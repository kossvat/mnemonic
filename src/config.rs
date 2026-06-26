use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::env;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub watchers: WatcherConfig,
    pub classifier: ClassifierConfig,
    pub storage: StorageConfig,
    pub output: OutputConfig,
    #[serde(default)]
    pub llm: LlmConfig,
    #[serde(default)]
    pub ui: UiConfig,
    #[serde(default)]
    pub extraction: ExtractionConfig,
    #[serde(default)]
    pub peers: PeersConfig,
    #[serde(default)]
    pub sessions: SessionsConfig,
    #[serde(default)]
    pub dream: DreamConfig,
    #[serde(default)]
    pub activity: ActivityConfig,
    #[serde(default)]
    pub graph: GraphConfig,
}

/// User-specific graph extraction vocabulary, merged ON TOP of the generic
/// built-in defaults at startup. Keep PRIVATE project / persona / client and
/// person names HERE — this config file is local and never committed, unlike
/// the open-source extractor defaults which stay generic. Example:
///
/// ```toml
/// [graph]
/// projects = ["my-app", "client-x"]
/// tech     = ["my-render-tool", "internal-cli"]
/// people   = ["alice", "bob"]
/// deny     = ["sprint", "standup"]
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GraphConfig {
    /// Extra names always typed as Project (added to built-in defaults).
    #[serde(default)]
    pub projects: Vec<String>,
    /// Extra names always typed as Tech.
    #[serde(default)]
    pub tech: Vec<String>,
    /// Extra names always typed as Person.
    #[serde(default)]
    pub people: Vec<String>,
    /// Extra generic terms to drop (never become graph entities).
    #[serde(default)]
    pub deny: Vec<String>,
    /// Map of canonical project -> alias terms (tools / sub-brands) that should
    /// also attribute to that project. Private; local-only. Example:
    ///   [graph.aliases]
    ///   "my-app" = ["some-tool", "some-brand"]
    #[serde(default)]
    pub aliases: std::collections::HashMap<String, Vec<String>>,
}

/// Peer attribution. When `auto_tag = true` (default), the daemon links
/// every new memory to a "user" peer (you) as the speaker; for memories
/// from the conversation watcher (Claude Code JSONL) it ALSO links the
/// configured agent peer as a `participant` (neutral wrt turn direction,
/// covers both user-message memories where the agent is addressee and
/// assistant-summary memories where the agent is speaker). Both peers
/// are upserted at daemon startup so attribution works on a brand-new
/// install.
///
/// Configure the names if you don't want the defaults "user" / "claude":
///
/// ```toml
/// [peers]
/// auto_tag = true
/// user_name = "alice"
/// user_display = "Alice"
/// agent_name = "claude"
/// agent_display = "Claude"
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeersConfig {
    #[serde(default = "default_auto_tag_peers")]
    pub auto_tag: bool,
    #[serde(default = "default_user_peer_name")]
    pub user_name: String,
    #[serde(default = "default_user_peer_display")]
    pub user_display: String,
    #[serde(default = "default_agent_peer_name")]
    pub agent_name: String,
    #[serde(default = "default_agent_peer_display")]
    pub agent_display: String,
    /// Peer name for Codex-sourced memories (kept distinct from the Claude
    /// agent peer so the graph attributes Codex turns correctly).
    #[serde(default = "default_codex_peer_name")]
    pub codex_agent_name: String,
    #[serde(default = "default_codex_peer_display")]
    pub codex_agent_display: String,
}

impl Default for PeersConfig {
    fn default() -> Self {
        Self {
            auto_tag: default_auto_tag_peers(),
            user_name: default_user_peer_name(),
            user_display: default_user_peer_display(),
            agent_name: default_agent_peer_name(),
            agent_display: default_agent_peer_display(),
            codex_agent_name: default_codex_peer_name(),
            codex_agent_display: default_codex_peer_display(),
        }
    }
}

/// Session boundary detection settings — used by the daemon's
/// SessionTracker when grouping conversation-watcher memories into
/// logical sessions keyed by JSONL file path.
///
/// A session represents one continuous thread of activity in a JSONL
/// file. When the daemon observes a memory and the file has been idle
/// longer than `idle_timeout_secs`, the previous session is closed and
/// a new one opened. New JSONL paths always start fresh sessions.
///
/// Default 30 minutes balances "two queries 10 min apart belong to one
/// session" with "a workday isn't one giant session" — Claude Code
/// sessions typically gap by minutes during active work and hours
/// between work blocks.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionsConfig {
    #[serde(default = "default_session_idle_timeout_secs")]
    pub idle_timeout_secs: u64,
}

impl Default for SessionsConfig {
    fn default() -> Self {
        Self {
            idle_timeout_secs: default_session_idle_timeout_secs(),
        }
    }
}

fn default_session_idle_timeout_secs() -> u64 {
    1800
}

fn default_auto_tag_peers() -> bool {
    true
}
fn default_user_peer_name() -> String {
    // Generic default so a freshly-installed daemon attributes your work to a
    // neutral "user" peer. Override with your own handle in config.toml:
    //   [peers]
    //   user_name = "alice"
    //   user_display = "Alice"
    "user".into()
}
fn default_user_peer_display() -> String {
    "You".into()
}
fn default_agent_peer_name() -> String {
    "claude".into()
}
fn default_agent_peer_display() -> String {
    "Claude".into()
}
fn default_codex_peer_name() -> String {
    "codex".into()
}
fn default_codex_peer_display() -> String {
    "Codex".into()
}

/// Async entity extraction settings. When `async_enabled = true` (the
/// default), the daemon's save path commits each memory to SQLite
/// immediately and pushes the row into `extraction_queue`; a background
/// worker drains the queue and runs the rule-based + optional LLM
/// extractor without blocking ingestion. Set false to fall back to the
/// pre-async behavior where extraction runs synchronously on save (legacy
/// path, kept for tests and emergency rollback).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExtractionConfig {
    #[serde(default = "default_async_enabled")]
    pub async_enabled: bool,
    /// How often (seconds) the worker polls the queue.
    #[serde(default = "default_worker_interval_secs")]
    pub worker_interval_secs: u64,
    /// Max memories processed per worker tick. Bounded so a sudden burst
    /// of saves doesn't lock up the LLM connection for minutes.
    #[serde(default = "default_worker_batch_size")]
    pub worker_batch_size: usize,
}

impl Default for ExtractionConfig {
    fn default() -> Self {
        Self {
            async_enabled: default_async_enabled(),
            worker_interval_secs: default_worker_interval_secs(),
            worker_batch_size: default_worker_batch_size(),
        }
    }
}

fn default_async_enabled() -> bool {
    true
}
fn default_worker_interval_secs() -> u64 {
    2
}
fn default_worker_batch_size() -> usize {
    5
}

/// Dream-consolidation worker settings. The daemon runs a periodic
/// task that summarizes recently-closed sessions, producing
/// `session_summary` memories so retrieval can surface high-level
/// "what happened in that session" results without rereading every
/// atomic memory.
///
/// Defaults: enabled, polls hourly, looks back 24h, heuristic
/// summarizer (no LLM). User must opt into LLM via `use_llm = true`
/// — auto-LLM-by-default would be surprising and could rack up
/// Ollama calls on long-uptime daemons. Setting `enabled = false`
/// disables the worker entirely; manual `mnemonic dream batch`
/// always works.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamConfig {
    #[serde(default = "default_dream_enabled")]
    pub enabled: bool,
    /// How often (seconds) the worker scans for unsummarized
    /// closed sessions. Default 3600 (hourly).
    #[serde(default = "default_dream_interval_secs")]
    pub interval_secs: u64,
    /// Look-back window. Sessions whose `ended_at` is older than
    /// this are out of scope — keeps the worker bounded on
    /// long-uptime DBs. Default 24h.
    #[serde(default = "default_dream_since_hours")]
    pub since_hours: u64,
    /// Cap on sessions summarized per tick. Bounded so a burst
    /// of session closures doesn't lock up the LLM connection.
    #[serde(default = "default_dream_batch_limit")]
    pub batch_limit: usize,
    /// Use the LLM summarizer instead of the heuristic. Requires
    /// `[llm] enabled = true`. Off by default — the heuristic
    /// summarizer is cheap and deterministic; users opt into LLM
    /// when they want prose narrative output.
    #[serde(default = "default_dream_use_llm")]
    pub use_llm: bool,
}

impl Default for DreamConfig {
    fn default() -> Self {
        Self {
            enabled: default_dream_enabled(),
            interval_secs: default_dream_interval_secs(),
            since_hours: default_dream_since_hours(),
            batch_limit: default_dream_batch_limit(),
            use_llm: default_dream_use_llm(),
        }
    }
}

fn default_dream_enabled() -> bool {
    // Off by default — Codex caught that an auto-enabled worker
    // running heuristic summarizer would freeze a heuristic
    // summary on every closed session, then later `dream run
    // --llm` would skip via the metadata-link idempotency and
    // leave the user with cheap heuristic prose instead of the
    // LLM upgrade they explicitly asked for. The `--regenerate`
    // CLI flag exists for that path, but the safer default is
    // "no automatic summarization unless you opt in". Users who
    // want the cron set `[dream] enabled = true` AND optionally
    // `use_llm = true` together.
    false
}
fn default_dream_interval_secs() -> u64 {
    3600 // hourly
}
fn default_dream_since_hours() -> u64 {
    24
}
fn default_dream_batch_limit() -> usize {
    50
}
fn default_dream_use_llm() -> bool {
    false
}

/// HTTP dashboard API. Off by default — daemon never opens a port unless
/// you explicitly opt in via `[ui] enabled = true`. Bound to 127.0.0.1
/// only; auth via token file under ~/.mnemonic/auth.token.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_ui_port")]
    pub port: u16,
    /// Path to a file containing the API auth token. Auto-generated on
    /// first start if missing. Sent as `X-Mnemonic-Token` header by the UI.
    #[serde(default = "default_ui_token_file")]
    pub token_file: PathBuf,
    /// Allowed CORS origins for the dashboard frontend. Defaults cover Vite
    /// dev (5173) and a future bundled build on 3737.
    #[serde(default = "default_ui_origins")]
    pub cors_origins: Vec<String>,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_ui_port(),
            token_file: default_ui_token_file(),
            cors_origins: default_ui_origins(),
        }
    }
}

fn default_ui_port() -> u16 {
    3737
}
fn default_ui_token_file() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".mnemonic/auth.token")
}
fn default_ui_origins() -> Vec<String> {
    vec![
        "http://localhost:5173".into(),
        "http://127.0.0.1:5173".into(),
        "http://localhost:3737".into(),
        "http://127.0.0.1:3737".into(),
    ]
}

/// LLM-backed entity/relation extractor. Off by default — pure rule-based
/// extraction still runs. When enabled, an Ollama-compatible JSON endpoint
/// is called for each new memory and merged with rule-based output.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LlmConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Base URL of the Ollama-compatible API (e.g. http://localhost:11434).
    #[serde(default = "default_ollama_url")]
    pub endpoint: String,
    /// Model id. Small instruction-tuned models work best for extraction.
    #[serde(default = "default_llm_model")]
    pub model: String,
    /// HTTP request timeout (seconds). Local models are fast; remote may need more.
    #[serde(default = "default_llm_timeout")]
    pub timeout_secs: u64,
    /// Skip the LLM call if memory content is shorter than this. Avoids
    /// burning tokens on commit hashes or one-word events.
    #[serde(default = "default_llm_min_chars")]
    pub min_chars: usize,
}

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_ollama_url(),
            model: default_llm_model(),
            timeout_secs: default_llm_timeout(),
            min_chars: default_llm_min_chars(),
        }
    }
}

fn default_ollama_url() -> String {
    "http://localhost:11434".into()
}
fn default_llm_model() -> String {
    "qwen2.5:3b".into()
}
fn default_llm_timeout() -> u64 {
    30
}
fn default_llm_min_chars() -> usize {
    40
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
    /// Enable Codex watcher (Codex CLI rollout transcripts)
    #[serde(default = "default_true")]
    pub codex_enabled: bool,
    /// Directory with Codex rollout JSONL sessions (defaults to ~/.codex/sessions)
    #[serde(default)]
    pub codex_sessions_dir: Option<PathBuf>,
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

/// Work-activity tracking: accurate daily "time worked" derived from
/// input idle time. Stored in its own `activity.db` (next to
/// `memory.db`) so high-frequency samples never touch the memory store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActivityConfig {
    /// Master switch for the activity sampler.
    #[serde(default = "default_activity_enabled")]
    pub enabled: bool,
    /// How often (seconds) to sample idle time. Second-granular idle
    /// means there's no point going below ~5s.
    #[serde(default = "default_activity_sample_secs")]
    pub sample_interval_secs: u64,
    /// Inactivity gap (seconds) that ends a work session. Short pauses
    /// under this still count as continuous work; cross it and the
    /// session closes at the last input ("you stepped away").
    #[serde(default = "default_activity_idle_threshold_secs")]
    pub idle_threshold_secs: u64,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        Self {
            enabled: default_activity_enabled(),
            sample_interval_secs: default_activity_sample_secs(),
            idle_threshold_secs: default_activity_idle_threshold_secs(),
        }
    }
}

fn default_activity_enabled() -> bool {
    true
}
fn default_activity_sample_secs() -> u64 {
    30
}
fn default_activity_idle_threshold_secs() -> u64 {
    180 // 3 minutes
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
                codex_enabled: true,
                codex_sessions_dir: None, // defaults to ~/.codex/sessions/
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
            llm: LlmConfig::default(),
            ui: UiConfig::default(),
            extraction: ExtractionConfig::default(),
            peers: PeersConfig::default(),
            sessions: SessionsConfig::default(),
            dream: DreamConfig::default(),
            activity: ActivityConfig::default(),
            graph: GraphConfig::default(),
        }
    }
}

impl Config {
    /// Path to the activity DB — always co-located with `memory.db`
    /// (same directory, `activity.db`). Kept as a derived path rather
    /// than a config field so the two stores can't drift apart.
    pub fn activity_db_path(&self) -> PathBuf {
        self.storage
            .db_path
            .parent()
            .map(|p| p.join("activity.db"))
            .unwrap_or_else(|| PathBuf::from("activity.db"))
    }

    pub fn load() -> Result<Self> {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let config_path = home.join(".config/mnemonic/config.toml");

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let config: Config = toml::from_str(&content)?;
            tighten_config_path(&config_path);
            Ok(config)
        } else {
            Ok(Self::default())
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
            tighten_owner_only_dir(parent);
        }
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        // The config may hold an optional memory_api_key; keep it owner-only.
        tighten_owner_only_file(path);
        Ok(())
    }

    /// Resolve the Memory API key from the environment first, then config.
    /// Keeps secrets out of config files when `MNEMONIC_MEMORY_API_KEY` is set.
    pub fn get_memory_api_key(&self) -> String {
        resolve_memory_api_key(
            env::var("MNEMONIC_MEMORY_API_KEY")
                .ok()
                .filter(|v| !v.trim().is_empty()),
            &self.output.memory_api_key,
        )
    }
}

fn resolve_memory_api_key(env_value: Option<String>, config_value: &str) -> String {
    env_value.unwrap_or_else(|| config_value.to_string())
}

fn tighten_config_path(path: &Path) {
    if let Some(parent) = path.parent() {
        tighten_owner_only_dir(parent);
    }
    tighten_owner_only_file(path);
}

#[cfg(unix)]
fn tighten_owner_only_dir(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
}

#[cfg(not(unix))]
fn tighten_owner_only_dir(_path: &Path) {}

#[cfg(unix)]
fn tighten_owner_only_file(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn tighten_owner_only_file(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_memory_api_key_wins_over_config_value() {
        let resolved = resolve_memory_api_key(Some("from-env".into()), "from-config");
        assert_eq!(resolved, "from-env");
    }

    #[test]
    fn config_memory_api_key_is_fallback() {
        let resolved = resolve_memory_api_key(None, "from-config");
        assert_eq!(resolved, "from-config");
    }

    #[test]
    fn blank_env_memory_api_key_is_ignored_by_public_getter_path() {
        let mut cfg = Config::default();
        cfg.output.memory_api_key = "from-config".into();

        let blank: Option<String> = Some(String::new()).filter(|v| !v.trim().is_empty());
        let resolved = resolve_memory_api_key(blank, &cfg.output.memory_api_key);
        assert_eq!(resolved, "from-config");
    }

    #[cfg(unix)]
    #[test]
    fn tighten_config_path_locks_existing_file_and_parent() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("mnemonic-config-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "memory_api_key = \"secret\"\n").unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        tighten_config_path(&path);

        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);

        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_dir(&dir);
    }
}
