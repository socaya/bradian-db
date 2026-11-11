//! Snapshot export + restore (OPS-003).
//!
//! The export packages a partition's current manifest + every visible segment + its WAL
//! tail into a single on-disk directory tree:
//!
//! ```text
//! <export_root>/
//!   manifest.json       # copy of partition manifest at export time
//!   segments/<id>.derc  # one file per visible segment
//!   wal.log             # raw bytes of the current WAL (can be empty)
//!   SNAPSHOT.json       # metadata header: schema fingerprint, data_revision, ts
//! ```
//!
//! Restore reads the metadata header, lays the files back into the partition root, and the
//! partition can then be opened normally — the existing WAL replay + manifest-load path
//! takes over from there.
//!
//! The export is a thin, operator-friendly format. For wire transport use a tarball or
//! zstd-compressed stream on top.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::Path;

use dap_core::{DapError, DapResult};
use serde::{Deserialize, Serialize};

use crate::partition::Partition;
use crate::segment::SegmentReader;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SnapshotHeader {
    pub schema_fingerprint: String,
    pub data_revision: u64,
    pub exported_at_ms: i64,
    pub segment_count: usize,
    pub row_count: u64,
    pub dataset_id: String,
    pub partition_id: String,
}

/// Export the partition's current snapshot into `export_root`. Creates the directory if
/// it does not exist. Fails closed if `export_root` already contains a snapshot (the
/// operator must delete or rename first — we do not clobber).
pub fn export_snapshot(partition: &Partition, export_root: &Path) -> DapResult<SnapshotHeader> {
    if export_root.join("SNAPSHOT.json").exists() {
        return Err(DapError::AlreadyExists(format!(
            "export root {export_root:?} already contains a snapshot"
        )));
    }
    fs::create_dir_all(export_root)?;
    fs::create_dir_all(export_root.join("segments"))?;

    // Hold a snapshot handle so compaction cannot delete files while we copy.
    let _snap = partition.snapshot();

    let partition_dir = partition.dir();
    let manifest_src = partition_dir.join("manifest.json");
    if manifest_src.exists() {
        fs::copy(&manifest_src, export_root.join("manifest.json"))?;
    }
    let wal_src = partition_dir.join("wal.log");
    if wal_src.exists() {
        fs::copy(&wal_src, export_root.join("wal.log"))?;
    }

    let segments = partition.visible_segments();
    let mut row_count = 0u64;
    for seg in &segments {
        // Verify the segment is readable before copying — the export must be restorable.
        SegmentReader::open(&seg.path)?;
        let name = seg.path.file_name().ok_or_else(|| {
            DapError::invariant(format!("segment path has no file name: {:?}", seg.path))
        })?;
        fs::copy(&seg.path, export_root.join("segments").join(name))?;
        row_count += seg.row_count;
    }

    let data_revision = segments
        .iter()
        .map(|s| s.data_revision)
        .max()
        .unwrap_or(0);
    let header = SnapshotHeader {
        schema_fingerprint: partition.schema_fingerprint().to_string(),
        data_revision,
        exported_at_ms: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0),
        segment_count: segments.len(),
        row_count,
        dataset_id: partition_dataset_id(partition_dir).unwrap_or_default(),
        partition_id: partition_partition_id(partition_dir).unwrap_or_default(),
    };
    let json = serde_json::to_vec_pretty(&header)?;
    let mut f = File::create(export_root.join("SNAPSHOT.json"))?;
    f.write_all(&json)?;
    f.sync_all()?;
    Ok(header)
}

/// Restore a snapshot into `target_root`. `target_root` must be empty. After this call the
/// caller opens a `Partition` on `target_root/../..` (the `datasets/<d>/partitions/<p>`
/// path) with the same options they used at export time.
pub fn restore_snapshot(
    export_root: &Path,
    target_root: &Path,
) -> DapResult<SnapshotHeader> {
    if !export_root.join("SNAPSHOT.json").exists() {
        return Err(DapError::NotFound(format!(
            "no SNAPSHOT.json in {export_root:?}"
        )));
    }
    if target_root.exists()
        && fs::read_dir(target_root)?.next().is_some()
    {
        return Err(DapError::AlreadyExists(format!(
            "target root {target_root:?} is non-empty; refusing to overwrite"
        )));
    }
    fs::create_dir_all(target_root.join("segments"))?;

    let mut header_bytes = Vec::new();
    File::open(export_root.join("SNAPSHOT.json"))?.read_to_end(&mut header_bytes)?;
    let header: SnapshotHeader = serde_json::from_slice(&header_bytes)?;

    if export_root.join("manifest.json").exists() {
        fs::copy(
            export_root.join("manifest.json"),
            target_root.join("manifest.json"),
        )?;
    }
    if export_root.join("wal.log").exists() {
        fs::copy(export_root.join("wal.log"), target_root.join("wal.log"))?;
    }
    for entry in fs::read_dir(export_root.join("segments"))? {
        let entry = entry?;
        let dst = target_root.join("segments").join(entry.file_name());
        fs::copy(entry.path(), dst)?;
    }
    Ok(header)
}

fn partition_dataset_id(partition_dir: &Path) -> Option<String> {
    // partition_dir shape: <root>/datasets/<dataset_id>/partitions/<partition_id>
    let mut parts = partition_dir.components().rev();
    parts.next()?;
    parts.next()?;
    parts.next().map(|c| c.as_os_str().to_string_lossy().into_owned())
}

fn partition_partition_id(partition_dir: &Path) -> Option<String> {
    partition_dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::partition::{Partition, PartitionOptions};
    use dap_core::{Column, LogicalType, Schema, Value};
    use tempfile::tempdir;

    fn open_part(root: &Path) -> std::sync::Arc<Partition> {
        let schema = Schema::new(vec![
            Column::new("ou", LogicalType::Uid).required(),
            Column::new("value", LogicalType::F64),
        ])
        .unwrap();
        Partition::open(
            root,
            PartitionOptions {
                dataset_id: "d1".into(),
                partition_id: "p1".into(),
                schema,
                primary_key_columns: vec!["ou".into()],
                flush_rows: 1024,
                flush_bytes: 1 << 24,
            },
        )
        .unwrap()
    }

    fn write_one_segment(part: &Partition, ou: &str, value: f64) {
        part.upsert_row(
            vec![Value::Str(ou.into()), Value::F64(value)],
            1,
        )
        .unwrap();
        part.flush().unwrap();
    }

    #[test]
    fn export_and_restore_round_trip() {
        let src_dir = tempdir().unwrap();
        let export_dir = tempdir().unwrap();
        let export_root = export_dir.path().join("snap");
        let restored_dir = tempdir().unwrap();

        let part = open_part(src_dir.path());
        write_one_segment(&part, "ouAAAAAAAAA", 1.5);
        write_one_segment(&part, "ouBBBBBBBBB", 2.5);

        let header = export_snapshot(&part, &export_root).unwrap();
        assert_eq!(header.segment_count, 2);
        assert_eq!(header.row_count, 2);

        // Restore into a fresh target. The target_root must match a partition directory
        // layout, but restore_snapshot accepts any empty root for v1 and lays files there.
        let target_partition_root = restored_dir
            .path()
            .join("datasets")
            .join("d1")
            .join("partitions")
            .join("p1");
        restore_snapshot(&export_root, &target_partition_root).unwrap();

        // Re-open the partition at the restored root — the manifest + segments are back.
        let restored_part = Partition::open(
            restored_dir.path(),
            PartitionOptions {
                dataset_id: "d1".into(),
                partition_id: "p1".into(),
                schema: Schema::new(vec![
                    Column::new("ou", LogicalType::Uid).required(),
                    Column::new("value", LogicalType::F64),
                ])
                .unwrap(),
                primary_key_columns: vec!["ou".into()],
                flush_rows: 1024,
                flush_bytes: 1 << 24,
            },
        )
        .unwrap();
        assert_eq!(restored_part.visible_segments().len(), 2);
    }

    #[test]
    fn export_refuses_to_overwrite() {
        let src = tempdir().unwrap();
        let part = open_part(src.path());
        write_one_segment(&part, "ouAAAAAAAAA", 1.5);
        let export = tempdir().unwrap();
        let root = export.path().join("snap");
        export_snapshot(&part, &root).unwrap();
        let err = export_snapshot(&part, &root).expect_err("second export rejected");
        assert!(err.to_string().contains("already"));
    }

    #[test]
    fn restore_refuses_non_empty_target() {
        let src = tempdir().unwrap();
        let part = open_part(src.path());
        write_one_segment(&part, "ouAAAAAAAAA", 1.5);

        let export_dir = tempdir().unwrap();
        let export_root = export_dir.path().join("snap");
        export_snapshot(&part, &export_root).unwrap();

        // Pre-populate the target so restore must fail closed rather than clobber.
        let target_dir = tempdir().unwrap();
        fs::create_dir_all(target_dir.path()).unwrap();
        fs::write(target_dir.path().join("pre-existing"), b"do not delete").unwrap();

        let err = restore_snapshot(&export_root, target_dir.path())
            .expect_err("restore rejected into a non-empty target");
        assert!(err.to_string().contains("non-empty"));

        // Target contents are untouched.
        assert!(target_dir.path().join("pre-existing").exists());
    }

    #[test]
    fn restore_rejects_missing_snapshot_header() {
        let empty_export = tempdir().unwrap();
        let target = tempdir().unwrap();
        let err = restore_snapshot(empty_export.path(), &target.path().join("new"))
            .expect_err("restore rejected without SNAPSHOT.json");
        assert!(err.to_string().contains("SNAPSHOT.json"));
    }
}
