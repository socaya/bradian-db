//! `dap-admin` — operator CLI. Minimal scaffold; real commands land as serving matures.
#![deny(unsafe_code)]

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "dap-admin", version, about = "DAP operator CLI")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print build/version info.
    Version,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Version => {
            println!("dap-admin {}", env!("CARGO_PKG_VERSION"));
        }
    }
    Ok(())
}
