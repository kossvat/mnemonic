mod api;
mod classifier;
mod config;
mod daemon;
mod embedding;
mod event;
mod mcp;
mod output;
mod scoring;
mod storage;
mod watcher;

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use config::Config;
use daemon::Daemon;
use event::{EventSource, MemoryEntry, MemoryType};

fn init_logging(log_file: Option<&std::path::Path>) {
    if let Some(path) = log_file {
        // Daemon mode: log to file
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(file) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
        {
            tracing_subscriber::fmt()
                .with_env_filter(
                    EnvFilter::try_from_default_env()
                        .unwrap_or_else(|_| EnvFilter::new("mnemonic=info")),
                )
                .with_target(false)
                .with_ansi(false)
                .with_writer(std::sync::Mutex::new(file))
                .init();
            return;
        }
    }
    // Interactive mode: log to stderr
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("mnemonic=info")),
        )
        .with_target(false)
        .init();
}

#[derive(Parser)]
#[command(
    name = "mnemonic",
    version,
    about = "Background memory daemon for AI coding agents"
)]
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
    /// Manually save a memory
    Save {
        /// Memory title
        #[arg(short, long)]
        title: String,
        /// Memory content
        content: String,
        /// Type: decision, feedback, note
        #[arg(short = 'T', long, default_value = "note")]
        memory_type: String,
        /// Comma-separated tags
        #[arg(long, default_value = "")]
        tags: String,
    },
    /// Find semantically similar memories
    Similar {
        /// Search text
        text: String,
        /// Max results
        #[arg(short, long, default_value = "5")]
        limit: usize,
    },
    /// Generate context file with relevant memories (Whisper)
    Context {
        /// Optional topic to focus on
        #[arg(short, long)]
        topic: Option<String>,
        /// Max entries per section
        #[arg(short, long, default_value = "10")]
        limit: usize,
        /// Output path (default: project memory dir)
        #[arg(short, long)]
        output: Option<String>,
    },
    /// Export all memories to JSON (stdout)
    Export,
    /// Import memories from JSON file
    Import {
        /// Path to JSON file (or - for stdin)
        file: String,
    },
    /// Remove old low-importance memories
    Cleanup {
        /// Max age in days for low-importance notes (default: 30)
        #[arg(short, long, default_value = "30")]
        days: i64,
        /// Importance threshold — notes below this get cleaned (default: 0.5)
        #[arg(short, long, default_value = "0.5")]
        threshold: f32,
        /// Actually delete (without this flag, only shows what would be deleted)
        #[arg(long)]
        confirm: bool,
    },
    /// Diagnose common setup issues
    Doctor,
    /// JSON stats for widgets (daily counts, last activity, dedup)
    Stats {
        /// Output as JSON
        #[arg(long)]
        json: bool,
        /// Number of days for daily breakdown (default: 7)
        #[arg(short, long, default_value = "7")]
        days: usize,
    },
    /// Run as MCP server (JSON-RPC over stdio)
    Mcp,
    /// Generate default config file
    Init,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;

    // Daemon and foreground start: log to file. Everything else: log to stderr.
    match &cli.command {
        Commands::Start { .. } => init_logging(Some(&config.daemon.log_file)),
        Commands::Mcp | Commands::Stats { .. } => {} // stdout is structured output, no tracing
        _ => init_logging(None),
    }

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
        Commands::Save {
            title,
            content,
            memory_type,
            tags,
        } => {
            let mt = match memory_type.as_str() {
                "decision" => MemoryType::Decision,
                "feedback" => MemoryType::Feedback,
                "session_summary" => MemoryType::SessionSummary,
                _ => MemoryType::Note,
            };

            let tag_list: Vec<String> = tags
                .split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();

            let mut entry = MemoryEntry::new(&title, &content, mt.clone(), EventSource::Manual);
            entry.tags = tag_list;

            let st = storage::Storage::open(&config.storage.db_path)?;

            // Generate embedding, dedup check, and dynamic scoring
            let embedder = embedding::HashEmbedder::new();
            use embedding::Embedder;
            let embed_text = format!("{} {}", entry.title, entry.content);
            if let Ok(emb) = embedder.embed(&embed_text) {
                if let Ok(Some(sim)) = st.is_duplicate(&emb, config.classifier.dedup_threshold) {
                    println!("Skipped (duplicate, similarity={sim:.3}): {title}");
                    return Ok(());
                }
                // Dynamic importance scoring
                let scorer = scoring::ImportanceScorer::default();
                if let Ok(score) = scorer.score(
                    &emb,
                    &event::EventKind::Custom("manual".into()),
                    &mt,
                    &st.conn,
                ) {
                    entry.importance = score;
                    println!("Importance: {score:.2}");
                } else {
                    entry.importance = 0.7;
                }
                st.save_with_embedding(&entry, Some(&emb))?;
            } else {
                entry.importance = 0.7;
                st.save(&entry)?;
            }

            // Write to output sinks
            use storage::OutputSink;
            if config.output.memory_files_enabled {
                let sink = output::memory_files::MemoryFileSink::new(
                    config.output.memory_files_path.clone(),
                );
                sink.write(&entry)?;
            }
            if config.output.obsidian_enabled {
                let sink = output::obsidian::ObsidianSink::new(config.output.obsidian_path.clone());
                sink.write(&entry)?;
            }

            println!("Saved: [{}] {}", entry.memory_type, title);
        }
        Commands::Similar { text, limit } => {
            let st = storage::Storage::open(&config.storage.db_path)?;

            let embedder = embedding::HashEmbedder::new();
            use embedding::Embedder;
            let query_emb = embedder.embed(&text)?;
            let results = st.find_similar(&query_emb, limit)?;

            if results.is_empty() {
                println!("No similar memories found for: {text}");
            } else {
                println!("Top {} similar memories:\n", results.len());
                for (entry, sim) in &results {
                    println!(
                        "  [{:>10}] {} (similarity: {:.3}, importance: {:.1})",
                        entry.memory_type, entry.title, sim, entry.importance
                    );
                    if !entry.tags.is_empty() {
                        println!("             tags: {}", entry.tags.join(", "));
                    }
                    println!("             {}", entry.timestamp.format("%Y-%m-%d %H:%M"));
                    println!();
                }
            }
        }
        Commands::Context {
            topic,
            limit,
            output,
        } => {
            let st = storage::Storage::open(&config.storage.db_path)?;

            // Default output: project memory dir / CONTEXT.md
            let output_path = match output {
                Some(p) => std::path::PathBuf::from(p),
                None => {
                    let cwd = std::env::current_dir()?;
                    let encoded = urlencoding::encode(&cwd.to_string_lossy()).to_string();
                    config
                        .output
                        .memory_files_path
                        .join(format!("-{encoded}"))
                        .join("CONTEXT.md")
                }
            };

            let whisper = output::whisper::Whisper::new(output_path.clone());

            let content = match topic {
                Some(ref t) => whisper.generate_for_topic(&st, t, limit)?,
                None => whisper.generate(&st)?,
            };

            println!("{content}");
            println!("\n---\nWritten to: {}", output_path.display());
        }
        Commands::Export => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            let entries = st.export_all()?;
            let json = serde_json::to_string_pretty(&entries)?;
            println!("{json}");
        }
        Commands::Import { file } => {
            let content = if file == "-" {
                use std::io::Read;
                let mut buf = String::new();
                std::io::stdin().read_to_string(&mut buf)?;
                buf
            } else {
                std::fs::read_to_string(&file)?
            };

            let entries: Vec<serde_json::Value> = serde_json::from_str(&content)?;
            let st = storage::Storage::open(&config.storage.db_path)?;
            let (imported, skipped) = st.import_entries(&entries)?;
            println!("Imported: {imported}, skipped (duplicates): {skipped}");
        }
        Commands::Cleanup {
            days,
            threshold,
            confirm,
        } => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            if confirm {
                let deleted = st.cleanup(days, threshold)?;
                println!("Cleaned up {deleted} old low-importance memories");
                let stats = st.stats()?;
                println!("Remaining: {stats}");
            } else {
                // Dry run — just show stats
                let stats = st.stats()?;
                let db_size = st.db_size()?;
                println!("Current state:");
                println!("{stats}");
                println!("Database size: {:.1} KB", db_size as f64 / 1024.0);
                println!(
                    "\nWould clean: notes older than {days}d with importance < {threshold:.1}"
                );
                println!("Decisions and feedback are NEVER cleaned.");
                println!("\nRun with --confirm to actually delete.");
            }
        }
        Commands::Doctor => {
            println!("mnemonic doctor\n");
            let mut issues = 0;

            // Check daemon
            if let Some(pid) = Daemon::is_running(&config) {
                println!("✓ Daemon running (PID {pid})");
            } else {
                println!("✗ Daemon not running");
                println!("  → Run: mnemonic start -d");
                issues += 1;
            }

            // Check database
            if config.storage.db_path.exists() {
                let st = storage::Storage::open(&config.storage.db_path);
                match st {
                    Ok(st) => {
                        let count = st.count().unwrap_or(0);
                        let size = st.db_size().unwrap_or(0);
                        println!(
                            "✓ Database: {count} memories ({:.1} KB)",
                            size as f64 / 1024.0
                        );
                    }
                    Err(e) => {
                        println!("✗ Database error: {e}");
                        issues += 1;
                    }
                }
            } else {
                println!(
                    "✗ No database found at {}",
                    config.storage.db_path.display()
                );
                println!("  → Will be created on first run");
                issues += 1;
            }

            // Check config
            let home = dirs::home_dir().unwrap_or_default();
            let config_path = home.join(".config/mnemonic/config.toml");
            if config_path.exists() {
                println!("✓ Config: {}", config_path.display());
            } else {
                println!("⚠ No config file (using defaults)");
                println!("  → Run: mnemonic init");
            }

            // Check git
            let cwd = std::env::current_dir().unwrap_or_default();
            if cwd.join(".git").exists() {
                println!("✓ Git repository detected");
            } else {
                println!("⚠ No git repository in current directory");
                println!("  → Git watcher will be disabled");
            }

            // Check Claude Code
            if home.join(".claude").exists() {
                println!("✓ Claude Code detected");
            } else {
                println!("⚠ Claude Code not found (~/.claude)");
                println!("  → Memory files and MCP integration won't work");
            }

            // Check socket
            if config.daemon.socket_path.exists() {
                println!("✓ API socket: {}", config.daemon.socket_path.display());
            } else if Daemon::is_running(&config).is_some() {
                println!("✗ Daemon running but socket missing");
                issues += 1;
            }

            // Check Obsidian (only if enabled)
            if config.output.obsidian_enabled {
                if config.output.obsidian_path.exists() {
                    println!(
                        "✓ Obsidian vault: {}",
                        config.output.obsidian_path.display()
                    );
                } else {
                    println!(
                        "✗ Obsidian enabled but vault not found: {}",
                        config.output.obsidian_path.display()
                    );
                    println!("  → Disable in config or set correct path");
                    issues += 1;
                }
            } else {
                println!("- Obsidian: disabled");
            }

            if issues == 0 {
                println!("\nAll checks passed ✓");
            } else {
                println!("\n{issues} issue(s) found");
            }
        }
        Commands::Stats { json, days } => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            let stats = st.stats()?;
            let daily = st.daily_counts(days)?;
            let last_activity = st.last_activity()?;
            let db_size = st.db_size()?;
            let (saved, with_emb) = st.dedup_estimate()?;
            let is_running = Daemon::is_running(&config);

            if json {
                let daily_json: Vec<serde_json::Value> = daily
                    .iter()
                    .map(|(date, count)| {
                        serde_json::json!({"date": date, "count": count})
                    })
                    .collect();

                let by_type: serde_json::Map<String, serde_json::Value> = stats
                    .by_type
                    .iter()
                    .map(|(t, c)| (t.clone(), serde_json::json!(c)))
                    .collect();

                // Calculate hours since last activity
                let silent_hours = last_activity.as_ref().and_then(|ts| {
                    chrono::DateTime::parse_from_rfc3339(ts).ok().map(|dt| {
                        let now = chrono::Utc::now();
                        let diff = now - dt.with_timezone(&chrono::Utc);
                        diff.num_minutes() as f64 / 60.0
                    })
                });

                let output = serde_json::json!({
                    "total": stats.total,
                    "by_type": by_type,
                    "daily": daily_json,
                    "last_activity": last_activity,
                    "silent_hours": silent_hours,
                    "db_size_bytes": db_size,
                    "db_size_kb": db_size as f64 / 1024.0,
                    "saved_total": saved,
                    "with_embeddings": with_emb,
                    "daemon_running": is_running.is_some(),
                    "daemon_pid": is_running,
                });
                println!("{}", serde_json::to_string_pretty(&output)?);
            } else {
                println!("mnemonic stats ({days}-day view)\n");
                println!("{stats}");
                println!("Database: {:.1} KB", db_size as f64 / 1024.0);
                println!("Entries with embeddings: {with_emb}/{saved}");

                if let Some(ts) = &last_activity {
                    println!("Last activity: {ts}");
                }

                if !daily.is_empty() {
                    println!("\nDaily breakdown:");
                    let max_count = daily.iter().map(|(_, c)| *c).max().unwrap_or(1);
                    for (date, count) in &daily {
                        let bar_len = (*count as f64 / max_count as f64 * 20.0) as usize;
                        let bar: String = "█".repeat(bar_len);
                        println!("  {date} {bar} {count}");
                    }
                }

                if let Some(pid) = is_running {
                    println!("\nDaemon: running (PID {pid})");
                } else {
                    println!("\nDaemon: stopped");
                }
            }
        }
        Commands::Mcp => {
            let server = mcp::McpServer::new(config);
            server.run()?;
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
