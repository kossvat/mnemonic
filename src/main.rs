mod api;
mod classifier;
mod config;
mod daemon;
mod event;
mod output;
mod storage;
mod watcher;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use config::Config;
use daemon::Daemon;

#[derive(Parser)]
#[command(name = "mnemonic", version, about = "Background memory daemon for AI coding agents")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the daemon in foreground
    Start {
        /// Run in background (daemonize)
        #[arg(short, long)]
        daemon: bool,
    },
    /// Stop a running daemon
    Stop,
    /// Show daemon status and memory stats
    Status,
    /// Search memories
    Query {
        /// Search text
        text: String,
        /// Max results
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Show recent memories
    Recent {
        /// Number of entries
        #[arg(short, long, default_value = "10")]
        limit: usize,
    },
    /// Generate default config file
    Init,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    // Init logging
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("mnemonic=info")),
        )
        .with_target(false)
        .init();

    let config = Config::load()?;

    match cli.command {
        Commands::Start { daemon: bg } => {
            if let Some(pid) = Daemon::is_running(&config) {
                eprintln!("mnemonic is already running (PID {pid})");
                std::process::exit(1);
            }

            if bg {
                daemonize()?;
            } else {
                let d = Daemon::new(config);
                d.run().await?;
            }
        }
        Commands::Stop => {
            if let Some(pid) = Daemon::is_running(&config) {
                unsafe {
                    libc::kill(pid as i32, libc::SIGTERM);
                }
                println!("Stopped mnemonic (PID {pid})");
            } else {
                println!("mnemonic is not running");
            }
        }
        Commands::Status => {
            if let Some(pid) = Daemon::is_running(&config) {
                println!("mnemonic is running (PID {pid})");
            } else {
                println!("mnemonic is not running");
            }

            if config.storage.db_path.exists() {
                let st = storage::Storage::open(&config.storage.db_path)?;
                let stats = st.stats()?;
                println!("\n{stats}");
            } else {
                println!("\nNo database found yet.");
            }
        }
        Commands::Query { text, limit } => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            let results = st.search(&text, limit)?;

            if results.is_empty() {
                println!("No results for: {text}");
            } else {
                println!("Found {} results:\n", results.len());
                for entry in &results {
                    println!(
                        "  [{:>10}] {} (importance: {:.1})",
                        entry.memory_type, entry.title, entry.importance
                    );
                    if !entry.tags.is_empty() {
                        println!("             tags: {}", entry.tags.join(", "));
                    }
                    println!("             {}", entry.timestamp.format("%Y-%m-%d %H:%M"));
                    println!();
                }
            }
        }
        Commands::Recent { limit } => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            let results = st.recent(limit)?;

            if results.is_empty() {
                println!("No memories yet.");
            } else {
                println!("Recent {} memories:\n", results.len());
                for entry in &results {
                    println!(
                        "  [{:>10}] {} (importance: {:.1})",
                        entry.memory_type, entry.title, entry.importance
                    );
                    println!("             {}", entry.timestamp.format("%Y-%m-%d %H:%M"));
                }
            }
        }
        Commands::Init => {
            let home = dirs::home_dir().unwrap_or_default();
            let config_path = home.join(".config/mnemonic/config.toml");
            let default_config = Config::default();
            default_config.save(&config_path)?;
            println!("Config written to: {}", config_path.display());
        }
    }

    Ok(())
}

fn daemonize() -> Result<()> {
    let exe = std::env::current_exe()?;
    let child = std::process::Command::new(exe)
        .arg("start")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .spawn()?;

    println!("mnemonic started in background (PID {})", child.id());
    Ok(())
}
