use crate::profile;
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
    #[serde(default)]
    pub lint: LintConfig,
}

/// Contradiction lint — periodic pass that flags decisions semantically
/// reversed by newer ones (see src/lint.rs). Flag-only: verdicts land in
/// an audit table; memories are never mutated. Runs without an LLM too
/// (pairs stay 'candidate'), so it is safe to keep enabled by default.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LintConfig {
    #[serde(default = "default_lint_enabled")]
    pub enabled: bool,
    /// Seconds between lint passes.
    #[serde(default = "default_lint_interval_secs")]
    pub interval_secs: u64,
    /// Cosine similarity at which a decision pair becomes a candidate.
    #[serde(default = "default_lint_similarity")]
    pub similarity: f32,
}

impl Default for LintConfig {
    fn default() -> Self {
        Self {
            enabled: default_lint_enabled(),
            interval_secs: default_lint_interval_secs(),
            similarity: default_lint_similarity(),
        }
    }
}

fn default_lint_enabled() -> bool {
    true
}
fn default_lint_interval_secs() -> u64 {
    1800
}
fn default_lint_similarity() -> f32 {
    0.65
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
    /// Enable the git watcher for the daemon's working directory
    #[serde(default = "default_true")]
    pub git_enabled: bool,
    /// Capture transcripts only from sessions whose working directory is
    /// inside one of these project roots. Empty = every project. Required
    /// in an isolated profile before a transcript watcher may be enabled.
    #[serde(default)]
    pub project_roots: Vec<PathBuf>,
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
    /// A near duplicate that states a changed business value (price,
    /// commission, terms, deadline) is saved and linked as an update instead
    /// of dropped. `false` restores the plain similarity rule.
    #[serde(default = "default_true")]
    pub value_aware_dedup: bool,
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
    /// Sessions shorter than this (seconds) are dropped when they close
    /// instead of being kept. Filters phantom blips: some peripherals /
    /// utilities reset the system idle counter for an instant (observed
    /// as exactly-15-minute 0-second "sessions"), which pollutes session
    /// counts while adding no time. Deliberately tiny by default — blips
    /// measure 0-2s, while a genuine quick check-in (10-20s) must keep
    /// counting toward daily totals. 0 disables the filter.
    #[serde(default = "default_activity_min_session_secs")]
    pub min_session_secs: u64,
}

impl Default for ActivityConfig {
    fn default() -> Self {
        Self {
            enabled: default_activity_enabled(),
            sample_interval_secs: default_activity_sample_secs(),
            idle_threshold_secs: default_activity_idle_threshold_secs(),
            min_session_secs: default_activity_min_session_secs(),
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
fn default_activity_min_session_secs() -> u64 {
    // Phantom idle-counter blips are 0-2s; 5s catches them with margin
    // while never eating a real 10-20s check-in (Codex review point:
    // short honest work must keep counting toward totals).
    5
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
                git_enabled: true,
                project_roots: Vec::new(),
            },
            classifier: ClassifierConfig {
                importance_threshold: 0.4,
                dedup_threshold: 0.92,
                value_aware_dedup: true,
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
            lint: LintConfig::default(),
        }
    }
}

/// The working directory as the shell spells it. The OS reports the physical
/// path, but a shell standing in a symlinked checkout keeps the logical one in
/// `PWD`, and that is what an agent started there records as its folder.
fn working_dir_as_spelled() -> Result<PathBuf> {
    let physical = std::env::current_dir()?;
    Ok(std::env::var_os("PWD")
        .map(PathBuf::from)
        .filter(|pwd| {
            pwd.is_absolute()
                && std::fs::canonicalize(pwd).ok() == std::fs::canonicalize(&physical).ok()
        })
        .unwrap_or(physical))
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
        Self::load_for(profile::active()?.as_deref())
    }

    /// Config file for the default store, or for the isolated profile
    /// selected by `MNEMONIC_HOME`.
    pub fn config_path() -> Result<PathBuf> {
        match profile::active()? {
            Some(home) => Self::profile_config_path(&profile::prepare(&home)?),
            None => Ok(Self::config_path_for(None)),
        }
    }

    /// The profile's `config.toml`, refused when it is a symlink that leaves
    /// the profile: reading would import another store's settings, and
    /// `init` would write through it.
    fn profile_config_path(home: &Path) -> Result<PathBuf> {
        let config_path = Self::config_path_for(Some(home));
        profile::ensure_inside(home, "config.toml", &config_path)?;
        Ok(config_path)
    }

    fn config_path_for(profile_home: Option<&Path>) -> PathBuf {
        match profile_home {
            Some(home) => home.join("config.toml"),
            None => dirs::home_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join(".config/mnemonic/config.toml"),
        }
    }

    /// Defaults for `mnemonic init`: profile-scoped when `MNEMONIC_HOME` is set.
    pub fn default_for_env() -> Result<Self> {
        Ok(match profile::active()? {
            Some(home) => Self::profile_default(&profile::prepare(&home)?),
            None => Self::default(),
        })
    }

    /// Turn on transcript capture limited to `roots`. No roots = unchanged.
    pub fn with_project_roots(mut self, roots: Vec<PathBuf>) -> Result<Self> {
        if roots.is_empty() {
            return Ok(self);
        }
        let mut resolved: Vec<PathBuf> = Vec::with_capacity(roots.len());
        for root in roots {
            let real = root.canonicalize().map_err(|e| {
                anyhow::anyhow!("--project-root {} is not a directory: {e}", root.display())
            })?;
            // A file resolves too, but no session can run inside it: capture
            // would silently take nothing while consuming every record.
            anyhow::ensure!(
                real.is_dir(),
                "--project-root {} is not a directory",
                root.display()
            );
            // Keep the spelling the human gave as well, made absolute: a
            // session started through a symlinked checkout records that path,
            // not the target. `..` cannot be resolved without following
            // links, so such a spelling keeps only the real path.
            let spelled = if root.is_absolute() {
                root.clone()
            } else {
                working_dir_as_spelled()?.join(&root)
            };
            // Collecting the components drops every `./` along the way.
            let spelled = (!spelled
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)))
            .then(|| spelled.components().collect::<PathBuf>());
            for candidate in [Some(real), spelled].into_iter().flatten() {
                if !resolved.contains(&candidate) {
                    resolved.push(candidate);
                }
            }
        }
        self.watchers.project_roots = resolved;
        self.watchers.conversation_enabled = true;
        self.watchers.codex_enabled = true;
        Ok(self)
    }

    pub fn load_for(profile_home: Option<&Path>) -> Result<Self> {
        let Some(home) = profile_home else {
            let config_path = Self::config_path_for(None);
            return if config_path.exists() {
                let content = std::fs::read_to_string(&config_path)?;
                let config: Config = toml::from_str(&content)?;
                tighten_config_path(&config_path);
                Ok(config)
            } else {
                Ok(Self::default())
            };
        };

        let home = profile::prepare(home)?;
        let defaults = Self::profile_default(&home);
        let config_path = Self::profile_config_path(&home)?;
        let config = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)?;
            let mut merged = toml::Value::try_from(&defaults)?;
            profile::merge(&mut merged, toml::from_str(&content)?);
            tighten_owner_only_file(&config_path);
            merged.try_into()?
        } else {
            defaults
        };
        config.ensure_profile_isolated(&home)?;
        Ok(config)
    }

    /// Defaults for an isolated profile: all state inside `home`, and every
    /// watcher and sink that reads or writes the owner's global directories
    /// (`~/.claude/projects`, `~/.codex/sessions`, the Obsidian vault) off
    /// until the profile's own config turns it on with an explicit path.
    pub fn profile_default(home: &Path) -> Self {
        let base = Self::default();
        Self {
            daemon: DaemonConfig {
                pid_file: home.join("mnemonic.pid"),
                socket_path: home.join("mnemonic.sock"),
                log_file: home.join("daemon.log"),
            },
            watchers: WatcherConfig {
                watch_paths: Vec::new(),
                conversation_enabled: false,
                codex_enabled: false,
                git_enabled: false,
                ..base.watchers
            },
            storage: StorageConfig {
                db_path: home.join("memory.db"),
            },
            output: OutputConfig {
                memory_files_enabled: false,
                memory_files_path: home.join("memory-files"),
                obsidian_enabled: false,
                obsidian_path: home.join("obsidian"),
                memory_api_enabled: false,
                ..base.output
            },
            ui: UiConfig {
                token_file: home.join("auth.token"),
                ..base.ui
            },
            // The idle sampler is machine-wide; every profile would record the
            // same owner activity.
            activity: ActivityConfig {
                enabled: false,
                ..base.activity
            },
            ..base
        }
    }

    /// Refuse to write a generated file outside the active isolated profile.
    /// Config validation covers configured paths; this covers the concrete
    /// destination (for example a CONTEXT.md symlink planted under an
    /// in-profile folder). No-op for the default store.
    pub fn ensure_profile_output(path: &Path) -> Result<()> {
        match profile::active()? {
            Some(home) => profile::ensure_inside(&profile::prepare(&home)?, "output file", path),
            None => Ok(()),
        }
    }

    /// Would this config keep the isolated profile at canonical `home` sealed?
    pub fn check_profile(&self, home: &Path) -> Result<()> {
        self.ensure_profile_isolated(home)
    }

    fn ensure_profile_isolated(&self, home: &Path) -> Result<()> {
        let activity_db = self.activity_db_path();
        for (label, path) in [
            ("storage.db_path", &self.storage.db_path),
            ("activity.db", &activity_db),
            ("daemon.pid_file", &self.daemon.pid_file),
            ("daemon.socket_path", &self.daemon.socket_path),
            ("daemon.log_file", &self.daemon.log_file),
            ("ui.token_file", &self.ui.token_file),
        ] {
            profile::ensure_inside(home, label, path)?;
        }
        profile::ensure_socket_fits(&self.daemon.socket_path)?;
        // Sinks write memories out of the store; keep them in the profile.
        // Checked even when a sink is off: `context` writes CONTEXT.md under
        // memory_files_path regardless of the switch.
        profile::ensure_inside(
            home,
            "output.memory_files_path",
            &self.output.memory_files_path,
        )?;
        profile::ensure_inside(home, "output.obsidian_path", &self.output.obsidian_path)?;
        // No capture source may be derived from the daemon's working directory.
        if let Some(relative) = self.watchers.watch_paths.iter().find(|p| !p.is_absolute()) {
            anyhow::bail!(
                "watchers.watch_paths must be absolute in an isolated profile: {}",
                relative.display()
            );
        }
        // Project roots narrow transcript capture to sessions that ran inside
        // them. A root that swallows the home directory narrows nothing.
        let user_home = dirs::home_dir().unwrap_or_default();
        let user_home_real = user_home
            .canonicalize()
            .unwrap_or_else(|_| user_home.clone());
        for root in &self.watchers.project_roots {
            if !root.is_absolute()
                || root
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                anyhow::bail!(
                    "watchers.project_roots must be absolute paths without `..`: {}",
                    root.display()
                );
            }
            let resolved = root.canonicalize().unwrap_or_else(|_| root.clone());
            if user_home.starts_with(root) || user_home_real.starts_with(&resolved) {
                anyhow::bail!(
                    "watchers.project_roots entry {} contains the whole home directory",
                    root.display()
                );
            }
        }
        let scoped = !self.watchers.project_roots.is_empty();
        // Without project roots, naming the global transcript folder
        // explicitly is still every project.
        // The whole agent folder, not just its sessions root: Codex date
        // folders and `archived_sessions` hold every project too.
        for (label, dir, global) in [
            (
                "watchers.conversation_sessions_dir",
                &self.watchers.conversation_sessions_dir,
                user_home.join(".claude"),
            ),
            (
                "watchers.codex_sessions_dir",
                &self.watchers.codex_sessions_dir,
                user_home.join(".codex"),
            ),
        ] {
            let Some(dir) = dir else { continue };
            if !dir.is_absolute()
                || dir
                    .components()
                    .any(|c| matches!(c, std::path::Component::ParentDir))
            {
                anyhow::bail!(
                    "{label} must be an absolute path without `..` in an isolated profile: {}",
                    dir.display()
                );
            }
            // Compare real targets: a symlink to the global folder is the
            // global folder.
            let resolved = dir.canonicalize().unwrap_or_else(|_| dir.clone());
            let global = global.canonicalize().unwrap_or(global);
            if !scoped && (global.starts_with(&resolved) || resolved.starts_with(&global)) {
                anyhow::bail!(
                    "{label} {} is part of the global {}; set watchers.project_roots to capture from it in an isolated profile",
                    dir.display(),
                    global.display()
                );
            }
        }
        // Unscoped, the daemon would fall back to the owner's global
        // transcript folders and ingest every project.
        if !scoped {
            if self.watchers.conversation_enabled
                && self.watchers.conversation_sessions_dir.is_none()
            {
                anyhow::bail!(
                    "watchers.conversation_enabled needs watchers.project_roots (or a dedicated conversation_sessions_dir) in an isolated profile"
                );
            }
            if self.watchers.codex_enabled && self.watchers.codex_sessions_dir.is_none() {
                anyhow::bail!(
                    "watchers.codex_enabled needs watchers.project_roots (or a dedicated codex_sessions_dir) in an isolated profile"
                );
            }
        }
        Ok(())
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

        let tmp = crate::test_support::temp_dir("mnemonic-config-");
        let dir = tmp.path();
        let path = dir.join("config.toml");
        std::fs::write(&path, "memory_api_key = \"secret\"\n").unwrap();
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        tighten_config_path(&path);

        let dir_mode = std::fs::metadata(dir).unwrap().permissions().mode() & 0o777;
        let file_mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        assert_eq!(file_mode, 0o600);
    }

    /// Isolated `MNEMONIC_HOME` profiles (Unix paths and permissions).
    #[cfg(unix)]
    mod isolated_profile {
        use super::*;

        fn profile_root(tag: &str) -> PathBuf {
            // Not `temp_dir()`: on macOS it is ~50 bytes deep and a profile
            // socket under it would trip the Unix socket path limit.
            let id = uuid::Uuid::new_v4().simple().to_string();
            let dir = PathBuf::from("/tmp").join(format!("mn-{tag}-{}", &id[..8]));
            std::fs::create_dir_all(&dir).unwrap();
            dir.canonicalize().unwrap()
        }

        fn state_paths(config: &Config) -> Vec<PathBuf> {
            vec![
                config.storage.db_path.clone(),
                config.activity_db_path(),
                config.daemon.pid_file.clone(),
                config.daemon.socket_path.clone(),
                config.daemon.log_file.clone(),
                config.ui.token_file.clone(),
            ]
        }

        #[test]
        fn profile_without_config_keeps_all_state_inside_and_capture_off() {
            let root = profile_root("bare");
            let home = root.join("client");
            let config = Config::load_for(Some(&home)).unwrap();

            for path in state_paths(&config) {
                assert!(path.starts_with(&home), "{} escaped", path.display());
            }
            assert!(config.watchers.watch_paths.is_empty());
            assert!(!config.watchers.conversation_enabled);
            assert!(!config.watchers.codex_enabled);
            assert!(!config.watchers.git_enabled);
            assert!(!config.activity.enabled);
            assert!(!config.output.memory_files_enabled);
            assert!(!config.output.obsidian_enabled);
            assert!(!config.output.memory_api_enabled);
            assert!(config.output.memory_files_path.starts_with(&home));
            assert!(config.output.obsidian_path.starts_with(&home));
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn partial_profile_config_never_inherits_owner_paths() {
            let root = profile_root("partial");
            let home = root.join("client");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::write(
            home.join("config.toml"),
            "[ui]\nenabled = true\n\n[classifier]\nimportance_threshold = 0.5\ndedup_threshold = 0.9\n",
        )
        .unwrap();

            let config = Config::load_for(Some(&home)).unwrap();
            assert!(config.ui.enabled);
            assert_eq!(config.classifier.dedup_threshold, 0.9);
            for path in state_paths(&config) {
                assert!(path.starts_with(&home), "{} escaped", path.display());
            }
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn profile_config_pointing_at_another_store_is_refused() {
            let root = profile_root("escape");
            let home = root.join("client");
            let owner = root.join("owner");
            std::fs::create_dir_all(&home).unwrap();
            std::fs::create_dir_all(&owner).unwrap();
            for (section, key) in [
                ("storage", "db_path"),
                ("daemon", "socket_path"),
                ("ui", "token_file"),
            ] {
                std::fs::write(
                    home.join("config.toml"),
                    format!("[{section}]\n{key} = \"{}\"\n", owner.join("x").display()),
                )
                .unwrap();
                assert!(
                    Config::load_for(Some(&home)).is_err(),
                    "{section}.{key} outside the profile must be refused"
                );
            }
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn profile_watchers_need_an_explicit_directory() {
            let root = profile_root("watch");
            let home = root.join("client");
            std::fs::create_dir_all(&home).unwrap();
            let config_path = home.join("config.toml");

            std::fs::write(&config_path, "[watchers]\nconversation_enabled = true\n").unwrap();
            assert!(Config::load_for(Some(&home)).is_err());
            std::fs::write(&config_path, "[watchers]\ncodex_enabled = true\n").unwrap();
            assert!(Config::load_for(Some(&home)).is_err());

            let sessions = root.join("sessions");
            std::fs::write(
                &config_path,
                format!(
                    "[watchers]\nconversation_enabled = true\nconversation_sessions_dir = \"{}\"\n",
                    sessions.display()
                ),
            )
            .unwrap();
            let config = Config::load_for(Some(&home)).unwrap();
            assert_eq!(config.watchers.conversation_sessions_dir, Some(sessions));
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn copied_owner_config_is_refused_even_with_state_paths_fixed() {
            let root = profile_root("copied");
            let home = profile::prepare(&root.join("client")).unwrap();
            let config_path = home.join("config.toml");
            let user_home = dirs::home_dir().unwrap();

            // Owner's sinks and watchers, state paths already moved inside.
            let mut copied = Config::profile_default(&home);
            copied.output.memory_files_enabled = true;
            copied.output.memory_files_path = user_home.join(".claude/projects");
            copied.save(&config_path).unwrap();
            assert!(
                Config::load_for(Some(&home)).is_err(),
                "sink outside the profile"
            );

            let mut copied = Config::profile_default(&home);
            copied.watchers.watch_paths = vec![PathBuf::from(".")];
            copied.save(&config_path).unwrap();
            assert!(
                Config::load_for(Some(&home)).is_err(),
                "cwd-derived watch path"
            );

            let mut copied = Config::profile_default(&home);
            copied.watchers.conversation_enabled = true;
            copied.watchers.conversation_sessions_dir = Some(user_home.join(".claude/projects"));
            copied.save(&config_path).unwrap();
            assert!(
                Config::load_for(Some(&home)).is_err(),
                "global transcript folder"
            );

            // Enabling a sink without a path stays inside the profile.
            std::fs::write(&config_path, "[output]\nmemory_files_enabled = true\n").unwrap();
            let config = Config::load_for(Some(&home)).unwrap();
            assert!(config.output.memory_files_path.starts_with(&home));
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn symlinked_profile_files_are_refused() {
            let root = profile_root("links");
            let owner = root.join("owner");
            std::fs::create_dir_all(&owner).unwrap();
            std::fs::write(owner.join("config.toml"), "[ui]\nenabled = true\n").unwrap();

            // config.toml pointing at another store's config, live or dangling.
            for target in [owner.join("config.toml"), owner.join("missing.toml")] {
                let home = profile::prepare(&root.join(format!("c{}", target.exists()))).unwrap();
                std::os::unix::fs::symlink(&target, home.join("config.toml")).unwrap();
                assert!(
                    Config::load_for(Some(&home)).is_err(),
                    "{}",
                    target.display()
                );
            }

            // activity.db is derived, not configured, and must be checked too.
            let home = profile::prepare(&root.join("activity")).unwrap();
            std::os::unix::fs::symlink(owner.join("activity.db"), home.join("activity.db"))
                .unwrap();
            assert!(Config::load_for(Some(&home)).is_err());
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn disabled_sink_path_and_aliased_transcript_dirs_are_refused() {
            let root = profile_root("alias");
            let home = profile::prepare(&root.join("client")).unwrap();
            let config_path = home.join("config.toml");

            // `context` writes CONTEXT.md here even with the sink off.
            let mut config = Config::profile_default(&home);
            config.output.memory_files_path = root.join("elsewhere");
            config.save(&config_path).unwrap();
            assert!(Config::load_for(Some(&home)).is_err());

            // Unscoped, no part of the agents' global folders is a dedicated dir.
            let user_home = dirs::home_dir().unwrap();
            for dir in [
                user_home.join(".codex/sessions/2026"),
                user_home.join(".codex/archived_sessions"),
            ] {
                let mut config = Config::profile_default(&home);
                config.watchers.codex_enabled = true;
                config.watchers.codex_sessions_dir = Some(dir.clone());
                config.save(&config_path).unwrap();
                assert!(Config::load_for(Some(&home)).is_err(), "{}", dir.display());
            }

            let global = dirs::home_dir().unwrap().join(".claude/projects");
            let link = root.join("claude-link");
            std::os::unix::fs::symlink(&global, &link).unwrap();
            for dir in [
                global.join("some-project/.."),
                PathBuf::from("relative/sessions"),
                link,
            ] {
                if !global.exists() && dir.starts_with(&root) {
                    continue; // symlink case needs the real folder to resolve
                }
                let mut config = Config::profile_default(&home);
                config.watchers.conversation_enabled = true;
                config.watchers.conversation_sessions_dir = Some(dir.clone());
                config.save(&config_path).unwrap();
                assert!(Config::load_for(Some(&home)).is_err(), "{}", dir.display());
            }
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn project_roots_unlock_the_global_transcript_folders() {
            let root = profile_root("roots");
            let home = profile::prepare(&root.join("client")).unwrap();
            let config_path = home.join("config.toml");
            let project = root.join("code/demoapp");

            // Scoped: watchers may run against the default global folders.
            std::fs::write(
                &config_path,
                format!(
                    "[watchers]\nconversation_enabled = true\ncodex_enabled = true\nproject_roots = [\"{}\"]\n",
                    project.display()
                ),
            )
            .unwrap();
            let config = Config::load_for(Some(&home)).unwrap();
            assert_eq!(config.watchers.project_roots, vec![project.clone()]);
            assert!(config.watchers.conversation_sessions_dir.is_none());

            // A root that swallows the home directory is no scope at all.
            let user_home = dirs::home_dir().unwrap();
            for bad in [
                user_home.clone(),
                PathBuf::from("/"),
                PathBuf::from("relative/project"),
                project.join("../other"),
            ] {
                let mut config = Config::profile_default(&home);
                config.watchers.conversation_enabled = true;
                config.watchers.project_roots = vec![bad.clone()];
                config.save(&config_path).unwrap();
                assert!(Config::load_for(Some(&home)).is_err(), "{}", bad.display());
            }
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn two_profiles_share_no_state_path() {
            let root = profile_root("pair");
            let a = Config::load_for(Some(&root.join("a"))).unwrap();
            let b = Config::load_for(Some(&root.join("b"))).unwrap();
            let default_paths = state_paths(&Config::default());
            for path in state_paths(&a) {
                assert!(!state_paths(&b).contains(&path));
                assert!(!default_paths.contains(&path));
            }
            std::fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn profile_default_round_trips_through_save_and_load() {
            let root = profile_root("init");
            let home = profile::prepare(&root.join("client")).unwrap();
            Config::profile_default(&home)
                .save(&home.join("config.toml"))
                .unwrap();
            let config = Config::load_for(Some(&home)).unwrap();
            assert_eq!(config.storage.db_path, home.join("memory.db"));
            std::fs::remove_dir_all(root).unwrap();
        }
    }
}
