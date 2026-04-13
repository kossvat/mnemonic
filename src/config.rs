use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub watchers: WatcherConfig,
    pub classifier: ClassifierConfig,
    pub storage: StorageConfig,
    pub output: OutputConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonConfig {
    pub pid_file: PathBuf,
    pub socket_path: PathBuf,
    pub log_file: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WatcherConfig {
    /// Directories to watch for file changes
    pub watch_paths: Vec<PathBuf>,
    /// File extensions to track
    pub extensions: Vec<String>,
    /// Paths to ignore
    pub ignore_patterns: Vec<String>,
    /// Debounce interval in milliseconds
    pub debounce_ms: u64,
    /// Enable conversation watcher (Claude Code JSONL sessions)
    #[serde(default = "default_true")]
    pub conversation_enabled: bool,
    /// Directory with Claude Code session JSONL files
    #[serde(default)]
    pub conversation_sessions_dir: Option<PathBuf>,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassifierConfig {
    /// Minimum importance score to save (0.0 - 1.0)
    pub importance_threshold: f32,
    /// Cosine similarity threshold for dedup
    pub dedup_threshold: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub db_path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputConfig {
    /// Write to Claude Code memory files
    pub memory_files_enabled: bool,
    pub memory_files_path: PathBuf,
    /// Write to Obsidian vault
    pub obsidian_enabled: bool,
    pub obsidian_path: PathBuf,
    /// Batch write interval in seconds
    pub batch_interval_secs: u64,
    /// Send to shared Memory API (for cross-agent access)
    #[serde(default)]
    pub memory_api_enabled: bool,
    #[serde(default)]
    pub memory_api_url: String,
    #[serde(default)]
    pub memory_api_key: String,
}

impl Default for Config {
    fn default() -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let data_dir = home.join(".mnemonic");
        let claude_memory = home.join(".claude/projects");

        Self {
            daemon: DaemonConfig {
                pid_file: data_dir.join("mnemonic.pid"),
                socket_path: data_dir.join("mnemonic.sock"),
                log_file: data_dir.join("daemon.log"),
            },
            watchers: WatcherConfig {
                watch_paths: vec![
                    PathBuf::from("."),        // current working dir
                    home.join(".claude-flow"), // sessions, insights
                ],
                extensions: vec![
                    "rs".into(),
                    "ts".into(),
                    "js".into(),
                    "py".into(),
                    "md".into(),
                    "toml".into(),
                    "json".into(),
                    "yaml".into(),
                ],
                ignore_patterns: vec![
                    "target/".into(),
                    "node_modules/".into(),
                    ".git/objects/".into(),
                    ".git/logs/".into(),
                    "*.lock".into(),
                ],
                debounce_ms: 500,
                conversation_enabled: true,
                conversation_sessions_dir: None, // defaults to ~/.claude/projects/
            },
            classifier: ClassifierConfig {
                importance_threshold: 0.4,
                dedup_threshold: 0.92,
            },
            storage: StorageConfig {
                db_path: data_dir.join("memory.db"),
            },
            output: OutputConfig {
                memory_files_enabled: true,
                memory_files_path: claude_memory,
                obsidian_enabled: false,
                obsidian_path: home.join("Documents/Obsidian/Vault"),
                batch_interval_secs: 5,
                memory_api_enabled: false,
                memory_api_url: String::new(),
                memory_api_key: String::new(),
            },
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_path = home.join(".config/mnemonic/config.toml");

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
