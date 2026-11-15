//! Shared Bradian operator commands for bradian-db and the legacy bradian-admin CLI.
//!
//! The admin binary wraps the embedded engine for operator workflows. v1 commands cover
//! snapshot export/restore, metadata adapter runs, and a health probe. The full admin app
//! (OPS-004) is a browser UI and is intentionally out of this binary's scope.
#![deny(unsafe_code)]

mod local_sql;
mod local_sql_output;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "bradian-db", version, about = "Bradian Database Systems operator CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print build / version info.
    Version,
    /// Run native local relational SQL (no HTTP listener).
    Sql(local_sql::SqlArgs),
    /// Export a partition's snapshot to a directory.
    ExportSnapshot(ExportSnapshotArgs),
    /// Restore a partition snapshot from a directory into a fresh target.
    RestoreSnapshot(RestoreSnapshotArgs),
    /// Print the header of a snapshot without restoring it.
    InspectSnapshot(InspectSnapshotArgs),
    /// Apply a DHIS2 `/api/metadata` JSON bundle to a new catalog and summarise it.
    LoadDhis2Metadata(LoadDhis2Args),
    /// Pull a live DHIS2 `/api/metadata` bundle over HTTP and apply it.
    FetchDhis2Metadata(FetchDhis2Args),
    /// Compact every visible segment of a partition into a single merged segment.
    CompactPartition(CompactArgs),
    /// Start the HTTP engine (API + optional static UI).
    Serve(ServeArgs),
}

#[derive(Parser)]
struct ServeArgs {
    /// Analytics root directory.
    #[arg(long)]
    root: PathBuf,
    /// Bind address.
    #[arg(long, default_value = "127.0.0.1:8080")]
    bind: String,
    /// Optional directory with the built `bradian-ui` bundle. When set, non-API routes serve
    /// the UI.
    #[arg(long)]
    ui_dir: Option<PathBuf>,
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
    /// ADR 0081/0085 — rows per page in emitted `.derc` segments.
    /// Default (omit): one page per column (pre-ADR-0081 shape).
    /// Small values (8–128) improve BACE dict-pool amortization.
    /// Zero is rejected.
    #[arg(long)]
    max_rows_per_page: Option<usize>,
    /// ADR 0083/0085 — enable the v1.4 column-level shared dict
    /// format. Combined with a small `--max-rows-per-page`, cuts
    /// dict-heavy workload size by ~82% (ADR 0083 §3.5 item 10).
    #[arg(long)]
    use_column_dict: bool,
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
struct FetchDhis2Args {
    /// DHIS2 base URL (e.g. https://play.dhis2.org/dev).
    #[arg(long)]
    base_url: String,
    /// Basic-auth username (or omit when using --token).
    #[arg(long)]
    username: Option<String>,
    /// Basic-auth password. Read from the DAP_DHIS2_PASSWORD env var if missing.
    #[arg(long)]
    password: Option<String>,
    /// Alternate bearer token (preempts username/password when set).
    #[arg(long)]
    token: Option<String>,
    /// HTTP timeout (e.g. "30s", "5min").
    #[arg(long, default_value = "60s")]
    timeout: humantime::Duration,
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
    /// ADR 0081/0085 — rows per page in the compacted `.derc`
    /// segment. See `export-snapshot --max-rows-per-page`.
    #[arg(long)]
    max_rows_per_page: Option<usize>,
    /// ADR 0083/0085 — enable v1.4 column-dict format. See
    /// `export-snapshot --use-column-dict`.
    #[arg(long)]
    use_column_dict: bool,
}

pub fn run() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Sql(args) => local_sql::run(args)?,
        Cmd::Version => {
            println!("bradian-db {}", env!("CARGO_PKG_VERSION"));
        }
        Cmd::ExportSnapshot(a) => do_export(a)?,
        Cmd::RestoreSnapshot(a) => do_restore(a)?,
        Cmd::InspectSnapshot(a) => do_inspect(a)?,
        Cmd::LoadDhis2Metadata(a) => do_load_dhis2(a)?,
        Cmd::FetchDhis2Metadata(a) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(do_fetch_dhis2(a))?;
        }
        Cmd::CompactPartition(a) => do_compact(a)?,
        Cmd::Serve(a) => {
            let runtime = tokio::runtime::Runtime::new()?;
            runtime.block_on(do_serve(a))?;
        }
    }
    Ok(())
}

async fn do_serve(a: ServeArgs) -> anyhow::Result<()> {
    use std::sync::Arc;
    // The bootstrap-only HTTP API is never permitted to listen remotely.
    let addr: std::net::SocketAddr = a.bind.parse()?;
    anyhow::ensure!(addr.ip().is_loopback(),
        "remote bind disabled until Bradian TLS and production authentication are implemented");
    let bootstrap_token = std::env::var("BRADIAN_API_TOKEN")
        .map_err(|_| anyhow::anyhow!("BRADIAN_API_TOKEN required for API startup"))?;
    let catalog = Arc::new(bradian_catalog::Catalog::new());
    let engine = Arc::new(bradian_serving::AnalyticsEngine::new(a.root, catalog));
    let state = bradian_serving::ServerState::new(engine);
    state.register_bootstrap_admin(&bootstrap_token).map_err(anyhow::Error::msg)?;
    let app = match a.ui_dir {
        Some(dir) => bradian_serving::routes::router_with_static_ui(state, dir),
        None => bradian_serving::router(state),
    };
    let listener = tokio::net::TcpListener::bind(&a.bind).await?;
    tracing::info!(addr = %a.bind, "dap engine listening");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn do_fetch_dhis2(a: FetchDhis2Args) -> anyhow::Result<()> {
    let auth = if let Some(token) = a.token {
        bradian_catalog::Dhis2Auth::Bearer(token)
    } else {
        let username = a
            .username
            .ok_or_else(|| anyhow::anyhow!("--username or --token required"))?;
        let password = a
            .password
            .or_else(|| std::env::var("DAP_DHIS2_PASSWORD").ok())
            .ok_or_else(|| {
                anyhow::anyhow!("--password or DAP_DHIS2_PASSWORD env var required")
            })?;
        bradian_catalog::Dhis2Auth::Basic { username, password }
    };
    let client =
        bradian_catalog::Dhis2HttpClient::new(a.base_url, auth)?.with_timeout(a.timeout.into());
    let catalog = bradian_catalog::Catalog::new();
    let report = client.load_metadata(&catalog).await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn do_export(a: ExportSnapshotArgs) -> anyhow::Result<()> {
    let schema = parse_schema(&a.schema)?;
    let pk = split_csv(&a.primary_key);
    if let Some(0) = a.max_rows_per_page {
        anyhow::bail!("--max-rows-per-page must be > 0");
    }
    let opts = bradian_storage::PartitionOptions {
        dataset_id: a.dataset,
        partition_id: a.partition,
        schema,
        primary_key_columns: pk,
        flush_rows: 1024,
        flush_bytes: 16 * 1024 * 1024,
        max_rows_per_page: a.max_rows_per_page,
        use_column_dict: a.use_column_dict,
    };
    let part = bradian_storage::Partition::open(&a.root, opts)?;
    let header = bradian_storage::export_snapshot(&part, &a.out)?;
    println!("{}", serde_json::to_string_pretty(&header)?);
    Ok(())
}

fn do_restore(a: RestoreSnapshotArgs) -> anyhow::Result<()> {
    let header = bradian_storage::restore_snapshot(&a.export_dir, &a.target_root)?;
    println!("{}", serde_json::to_string_pretty(&header)?);
    Ok(())
}

fn do_inspect(a: InspectSnapshotArgs) -> anyhow::Result<()> {
    let path = a.export_dir.join("SNAPSHOT.json");
    let bytes = std::fs::read(path)?;
    let header: bradian_storage::SnapshotHeader = serde_json::from_slice(&bytes)?;
    println!("{}", serde_json::to_string_pretty(&header)?);
    Ok(())
}

fn do_load_dhis2(a: LoadDhis2Args) -> anyhow::Result<()> {
    let bytes = std::fs::read(&a.file)?;
    let catalog = bradian_catalog::Catalog::new();
    let count = bradian_catalog::apply_dhis2_metadata_json(&catalog, &bytes)?;
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
    if let Some(0) = a.max_rows_per_page {
        anyhow::bail!("--max-rows-per-page must be > 0");
    }
    let opts = bradian_storage::PartitionOptions {
        dataset_id: a.dataset,
        partition_id: a.partition,
        schema,
        primary_key_columns: pk,
        flush_rows: 1024,
        flush_bytes: 16 * 1024 * 1024,
        max_rows_per_page: a.max_rows_per_page,
        use_column_dict: a.use_column_dict,
    };
    let part = bradian_storage::Partition::open(&a.root, opts)?;
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

fn parse_schema(s: &str) -> anyhow::Result<bradian_core::Schema> {
    let mut cols = Vec::new();
    for part in s.split(',') {
        let part = part.trim();
        let (name, ty) = part
            .split_once(':')
            .ok_or_else(|| anyhow::anyhow!("column '{part}' needs name:type"))?;
        let logical = parse_type(ty.trim())?;
        cols.push(bradian_core::Column::new(name.trim(), logical));
    }
    Ok(bradian_core::Schema::new(cols)?)
}

fn parse_type(s: &str) -> anyhow::Result<bradian_core::LogicalType> {
    Ok(match s {
        "bool" => bradian_core::LogicalType::Bool,
        "i32" => bradian_core::LogicalType::I32,
        "i64" => bradian_core::LogicalType::I64,
        "f64" => bradian_core::LogicalType::F64,
        "utf8" => bradian_core::LogicalType::Utf8,
        "uid" => bradian_core::LogicalType::Uid,
        "period" => bradian_core::LogicalType::Period,
        "timestamp" => bradian_core::LogicalType::Timestamp,
        decimal if decimal.starts_with("decimal:") => {
            let (_, rest) = decimal.split_at("decimal:".len());
            let (p, s) = rest
                .split_once(',')
                .ok_or_else(|| anyhow::anyhow!("decimal needs precision,scale"))?;
            bradian_core::LogicalType::Decimal {
                precision: p.trim().parse()?,
                scale: s.trim().parse()?,
            }
        }
        other => anyhow::bail!("unsupported type '{other}'"),
    })
}
