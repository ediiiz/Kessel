//! `kessel-sim`: serve one profile ZIP as a simulated TLS-PSK appliance.

#![forbid(unsafe_code)]

use std::net::IpAddr;
use std::path::PathBuf;

use anyhow::Context as _;
use clap::{Parser, ValueEnum};
use kessel_sim::{AccessCase, Options, Simulator};
use tracing_subscriber::EnvFilter;

/// Environment variable that sets the log filter (default `info`).
const LOG_ENV: &str = "KESSEL_SIM_LOG";

#[derive(Debug, Clone, Copy, ValueEnum)]
enum AccessWords {
    /// `READ`, `READWRITE`, as real appliances send them.
    Upper,
    /// `read`, `readWrite`, as the profile XML spells them.
    Camel,
}

/// A local Home Connect appliance simulator for Kessel development.
#[derive(Debug, Parser)]
#[command(version, about)]
struct Cli {
    /// The appliance profile ZIP to serve (TLS-PSK profiles only).
    #[arg(long)]
    profile: PathBuf,
    /// The address to listen on.
    #[arg(long, default_value = "127.0.0.1")]
    bind: IpAddr,
    /// The port to listen on.
    #[arg(long, default_value_t = 8443)]
    port: u16,
    /// Seed for every random choice (ids, description changes).
    #[arg(long, default_value_t = 1)]
    seed: u64,
    /// How `access` words are spelled in description changes.
    #[arg(long, value_enum, default_value = "upper")]
    access_words: AccessWords,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_env(LOG_ENV).unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let profile = hc_profile::Profile::from_zip_path(&cli.profile)
        .with_context(|| format!("reading {}", cli.profile.display()))?;
    let options = Options {
        bind: cli.bind,
        port: cli.port,
        seed: cli.seed,
        access_case: match cli.access_words {
            AccessWords::Upper => AccessCase::Upper,
            AccessWords::Camel => AccessCase::Camel,
        },
        ..Options::default()
    };
    if !options.bind.is_loopback() {
        tracing::warn!(bind = %options.bind, "listening on a non-loopback address");
    }
    let simulator = Simulator::bind(&profile, &options).await?;
    let model = profile.model();
    tracing::info!(
        addr = %simulator.local_addr()?,
        device_type = %profile.identity().device_type,
        entities = model.entities().len(),
        programs = model.programs().len(),
        "simulator listening at wss://<addr>/homeconnect"
    );
    simulator.run().await?;
    Ok(())
}
