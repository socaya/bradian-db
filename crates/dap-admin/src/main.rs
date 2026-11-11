//! `dap-admin` — operator CLI.
//!
//! The admin binary wraps the embedded engine for operator workflows. v1 commands cover
//! snapshot export/restore, metadata adapter runs, and a health probe. The full admin app
//! (OPS-004) is a browser UI and is intentionally out of this binary's scope.
#![deny(unsafe_code)]

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dap-admin", version, about = "DAP operator CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print build / version info.
    Version,
    /// Export a partition's snapshot to a directory.
    ExportSnapshot(ExportSnapshotArgs),
    /// Restore a partition snapshot from a directory into a fresh target.
    RestoreSnapshot(RestoreSnapshotArgs),
    /// Print the header of a snapshot without restoring it.
    InspectSnapshot(InspectSnapshotArgs),
    /// Apply a DHIS2 `/api/metadata` JSON bundle to a new catalog and summarise it.
    LoadDhis2Metadata(LoadDhis2Args),
    /// Compact every visible segment of a partition into a single merged segment.
    CompactPartition(CompactArgs),
}

#[derive(Parser)]
struct ExportSnapshotArgs {
    /// Analytics root directory (the parent of `datasets/`).
    #[arg(long)]
    root: PathBuf,
    /// Dataset ID.
    #[arg(long)]
    dataset: String,
    /// Partition ID.
    #[arg(long)]
    partition: String,
    /// Target export directory (created if missing; refuses to overwrite).
    #[arg(long)]
    out: PathBuf,
    /// Primary-key column names, comma-separated.
    #[arg(long)]
    primary_key: String,
    /// Columns in the partition schema as `name:type`, comma-separated. Types match
    /// `LogicalType`: `bool`, `i32`, `i64`, `f64`, `decimal:P,S`, `utf8`, `uid`, `period`,
    /// `timestamp`.
    #[arg(long)]
    schema: String,
}

#[derive(Parser)]
struct RestoreSnapshotArgs {
    #[arg(long)]
    export_dir: PathBuf,
    #[arg(long)]
    target_root: PathBuf,
}

#[derive(Parser)]
struct InspectSnapshotArgs {
    #[arg(long)]
    export_dir: PathBuf,
}

#[derive(Parser)]
struct LoadDhis2Args {
    /// Path to a DHIS2 `/api/metadata` JSON bundle on disk.
    #[arg(long)]
    file: PathBuf,
}

#[derive(Parser)]
struct CompactArgs {
    #[arg(long)]
    root: PathBuf,
    #[arg(long)]
    dataset: String,
    #[arg(long)]
    partition: String,
    #[arg(long)]
    primary_key: String,
    #[arg(long)]
    schema: String,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Version => {
            println!("dap-admin {}", env!("CARGO_PKG_VERSION"));
        }
        Cmd::ExportSnapshot(a) => do_export(a)?,
        Cmd::RestoreSnapshot(a) => do_restore(a)?,
        Cmd::InspectSnapshot(a) => do_inspect(a)?,
        Cmd::LoadDhis2Metadata(a) => do_load_dhis2(a)?,
        Cmd::CompactPartition(a) => do_compact(a)?,
    }
    Ok(())
}

fn do_export(a: ExportSnapshotArgs) -> anyhow::Result<()> {
    let schema = parse_schema(&a.schema)?;
    let pk = split_csv(&a.primary_key);
    let opts = dap_storage::PartitionOptions {
        dataset_id: a.dataset,
        partition_id: a.partition,
        schema,
        primary_key_columns: pk,
        flush_rows: 1024,
        flush_bytes: 16 * 1024 * 1024,
    };
    let part = dap_storage::Partition::open(&a.root, opts)?;
    let header = dap_storage::export_snapshot(&part, &a.out)?;
    println!("{}", serde_json::to_string_pretty(&header)?);
    Ok(())
}

fn do_restore(a: RestoreSnapshotArgs) -> anyhow::Result<()> {
    let header = dap_storage::restore_snapshot(&a.export_dir, &a.target_root)?;
    println!("{}", serde_json::to_string_pretty(&header)?);
    Ok(())
}

fn do_inspect(a: InspectSnapshotArgs) -> anyhow::Result<()> {
    let path = a.export_dir.join("SNAPSHOT.json");
    let bytes = std::fs::read(path)?;
    let header: dap_storage::SnapshotHeader = serde_json::from_slice(&bytes)?;
    println!("{}", serde_json::to_string_pretty(&header)?);
    Ok(())
}

fn do_load_dhis2(a: LoadDhis2Args) -> anyhow::Result<()> {
    let bytes = std::fs::read(&a.file)?;
    let catalog = dap_catalog::Catalog::new();
    let count = dap_catalog::apply_dhis2_metadata_json(&catalog, &bytes)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "applied_objects": count,
            "catalog_revision": catalog.revision().to_string(),
        }))?
    );
    Ok(())
}

fn do_compact(a: CompactArgs) -> anyhow::Result<()> {
    let schema = parse_schema(&a.schema)?;
    let pk = split_csv(&a.primary_key);
    let opts = dap_storage::PartitionOptions {
        dataset_id: a.dataset,
        partition_id: a.partition,
        schema,
        primary_key_columns: pk,
        flush_rows: 1024,
        flush_bytes: 16 * 1024 * 1024,
    };
    let part = dap_storage::Partition::open(&a.root, opts)?;
    match part.compact_all_segments()? {
        Some(seg) => println!(
            "{}",
            serde_json::to_string_pretty(&seg)?
        ),
        None => println!("{{\"compacted\": false}}"),
    }
    Ok(())
}

fn split_csv(s: &str) -> Vec<String> {
    s.split(',').map(|p| p.trim().to_string()).collect()
}

fn parse_schema(s: &str) -> anyhow::Result<dap_core::Schema> {
    let mut cols = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        let (name, ty) = part
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("column '{part}' needs name:type"))?;
        let logical = parse_type(ty.trim())?;
        cols.push(dap_core::Column::new(name.trim(), logical));
    }
    Ok(dap_core::Schema::new(cols)?)
}

fn parse_type(s: &str) -> anyhow::Result<dap_core::LogicalType> {
    Ok(match s {
        "bool" => dap_core::LogicalType::Bool,
        "i32" => dap_core::LogicalType::I32,
        "i64" => dap_core::LogicalType::I64,
        "f64" => dap_core::LogicalType::F64,
        "utf8" => dap_core::LogicalType::Utf8,
        "uid" => dap_core::LogicalType::Uid,
        "period" => dap_core::LogicalType::Period,
        "timestamp" => dap_core::LogicalType::Timestamp,
        decimal if decimal.starts_with("decimal:") => {
            let (_, rest) = decimal.split_at("decimal:".len());
            let (p, s) = rest
                .split_once(',')
                .ok_or_else(|| anyhow::anyhow!("decimal needs precision,scale"))?;
            dap_core::LogicalType::Decimal {
                precision: p.trim().parse()?,
                scale: s.trim().parse()?,
            }
        }
        other => anyhow::bail!("unsupported type '{other}'"),
    })
}
