mod activity;
mod activity_worker;
mod api;
mod attribution;
mod attribution_worker;
mod classifier;
mod conclusions_generator;
mod config;
mod daemon;
mod decay;
mod digest;
mod dream;
mod dream_worker;
mod embedding;
mod eval;
mod event;
mod extraction_worker;
mod graph;
mod http;
mod journal;
mod lint;
mod mcp;
mod output;
mod reflection;
mod reranker;
mod retrieval;
mod scoring;
mod semantic_attribution;
mod storage;
mod watcher;

use anyhow::{Context as _, Result};
use clap::{Parser, Subcommand};
// Trait import so `LlmConclusionGenerator::generate_for_subject` is
// callable as a method in the `conclusion generate` handler.
use conclusions_generator::ConclusionGenerator as _;
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
            // Tighten perms to 0600 on every start. The log can contain
            // request paths, fingerprints, and error context — nothing
            // *should* be a raw secret, but on a shared system "world
            // readable" still leaks operational metadata.
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
            }
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
        /// Run in background (daemonize). Refused when a launchd
        /// LaunchAgent already manages the daemon (use `mnemonic
        /// restart` / `launchctl kickstart` instead) to avoid a
        /// dual-daemon race. Otherwise a fully supported background
        /// start.
        #[arg(short, long)]
        daemon: bool,
    },
    /// Stop a running daemon
    Stop,
    /// Restart the daemon using the active lifecycle owner
    Restart,
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
        /// Emit JSON (type, title, full content, timestamp) + total count.
        /// Used by the widget's "Latest Memory" card.
        #[arg(long)]
        json: bool,
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
    /// Delete a single memory by id. Cascade-cleans linked
    /// memory_entities / memory_peers / conclusion_sources rows via
    /// existing FK ON DELETE CASCADE — that's why this is exposed
    /// as a first-class subcommand instead of a raw SQL one-shot.
    /// Use the full UUID; no prefix matching here to keep
    /// destructive operations explicit.
    Forget {
        /// Memory id (full UUID).
        id: String,
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
    /// Query knowledge graph for an entity
    Graph {
        /// Entity name to look up
        entity: String,
    },
    /// List all known entities
    Entities {
        /// Max results
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Backfill graph from existing memories
    Backfill,
    /// Backfill Obsidian vault with all existing memories (idempotent — skips existing files)
    BackfillObsidian {
        /// Overwrite existing notes (default: skip)
        #[arg(long)]
        force: bool,
        /// Override vault path from config
        #[arg(long)]
        vault: Option<String>,
    },
    /// Re-embed all existing memories using the current embedder.
    /// Use after switching embedder (e.g. SimHash 256d → Neural MiniLM 384d).
    /// Requires daemon to be stopped to avoid dimension mismatch in HNSW.
    Reembed,
    /// Run as MCP server (JSON-RPC over stdio)
    Mcp,
    /// Generate default config file
    Init,
    /// Rebuild and install binary + restart widget
    Upgrade,
    /// Canonicalize existing entity names and merge variants
    /// (e.g. "Acme Devices Co.", "acme-devices", "client-acme-devices"
    /// → all collapse into `acme-devices`).
    DedupeGraph {
        /// Show what would be merged without writing
        #[arg(long)]
        dry_run: bool,
    },
    /// Backfill project links: scan every memory's title and link it to any
    /// EXISTING project entity it names (e.g. my-app, my-service). Repairs
    /// memories the hardcoded-list extractor missed, so attribution/Journal
    /// see all your projects. Run `attribute backfill` afterwards.
    BacklinkProjects,
    /// Inspect and manipulate the temporal `facts` table — assertions of
    /// the form `(subject, predicate, value)` with valid_from/valid_to
    /// timestamps. New facts supersede previous current ones with the
    /// same (subject, predicate); the chain is preserved.
    #[command(subcommand)]
    Fact(FactCommands),
    /// Manage peers (first-class identities: User, Claude, Codex,
    /// clients) and their sessions. Foundation for multi-agent
    /// attribution — today only populated via this CLI; the conversation
    /// watcher will auto-tag in a follow-up commit.
    #[command(subcommand)]
    Peer(PeerCommands),
    /// Inspect and add inductive `conclusions` — higher-level patterns
    /// induced from clusters of memories. Sits one layer above facts:
    /// where facts are atomic (subject/predicate/value), conclusions are
    /// claims like "user prefers low-overhead developer tooling".
    ///
    /// v1 is foundation only: storage + manual entry via this CLI.
    /// v2 will plug an LLM generator into the async extraction worker.
    #[command(subcommand)]
    Conclusion(ConclusionCommands),
    /// Inspect sessions — logical threads that group memories from one
    /// continuous interaction (a Claude Code JSONL session, a meeting,
    /// a workday). Today sessions are opened explicitly via `peer
    /// sessions` / future watcher wiring; this surface lets you list
    /// and read them.
    #[command(subcommand)]
    Session(SessionCommands),
    /// Dream consolidation — generate `session_summary` memories from
    /// closed sessions. v1 produces heuristic summaries (counts +
    /// anchors + top entities), no LLM. Idempotent: skips sessions
    /// that already have a summary linked via metadata.
    #[command(subcommand)]
    Dream(DreamCommands),
    /// Re-run the graph extractor over existing memories. Useful to
    /// backfill the LLM extractor's output across history. Active-only
    /// by default — superseded memories don't get their edges rebuilt.
    ///
    /// Refuses to run if the daemon is up (concurrent writes corrupt
    /// edge counts); pass --force to override, or `--dry-run` which is
    /// always safe.
    ///
    /// Pre-flight: if the LLM extractor is enabled in config, the
    /// Ollama endpoint is probed before processing starts. A dead
    /// backend would mean half the memories get rule-based fallback
    /// silently — this fails fast instead.
    Reextract {
        /// Only the last N days
        #[arg(long)]
        since_days: Option<i64>,
        /// Max memories to process (default: all)
        #[arg(long)]
        limit: Option<usize>,
        /// Show plan without writing
        #[arg(long)]
        dry_run: bool,
        /// Include superseded source memories (default: skip — they were
        /// rolled into canonicals already and shouldn't re-add edges).
        #[arg(long)]
        include_superseded: bool,
        /// Wipe the whole graph (entities + edges + links) before rebuilding,
        /// so updated extractor rules reclassify existing nodes and drop
        /// now-denied ones. Without it, reextract upserts (INSERT OR IGNORE)
        /// and stale entity types survive. Memories are never touched.
        #[arg(long)]
        clean_graph: bool,
        /// Run anyway even if the daemon is up (DANGEROUS — concurrent writes).
        #[arg(long)]
        force: bool,
        /// Drain the `pending_extractions` queue instead of walking all
        /// memories. Picks up the rows whose next_attempt_at is in the
        /// past, retries them, bumps backoff on failure (5m → 30m → 2h →
        /// 6h → 24h), drops the row on the 6th consecutive failure. Use
        /// this after Ollama was down: `mnemonic reextract --pending`
        /// recovers everything the daemon couldn't extract.
        #[arg(long)]
        pending: bool,
        /// With `--pending`: if `llm.enabled = false`, delete the queue
        /// rows instead of refusing. Without this flag the drain aborts
        /// loudly — silent drop was the bug the queue exists to prevent.
        #[arg(long)]
        discard_pending: bool,
    },
    /// Run the retrieval eval harness against the current DB.
    ///
    /// Reads a JSONL file of `{query, expected_ids?, expected_title_contains?}`
    /// rows, runs each query through `hybrid_search`, and prints
    /// recall@5 / recall@20 / MRR plus a per-query breakdown.
    ///
    /// Doesn't write to the DB. Doesn't need the daemon stopped.
    Eval {
        /// Path to the JSONL seed file. Defaults to ./tests/eval/queries.jsonl
        /// (relative to the source tree).
        #[arg(short, long, default_value = "tests/eval/queries.jsonl")]
        file: String,
        /// Output JSON instead of a human-readable table (for diff'ing
        /// runs / CI baselines).
        #[arg(long)]
        json: bool,
        /// Disable the 1-hop graph expansion stage (BM25+vector only).
        /// Use this to attribute recall gains to retrieval components.
        #[arg(long)]
        no_graph_hop: bool,
        /// Run a cross-encoder reranker over the top-30 fused candidates
        /// before truncating to top-5/top-20. First use downloads
        /// jina-reranker-v2-base-multilingual (~278MB) to
        /// ~/.fastembed_cache; subsequent runs use the cached weights.
        #[arg(long)]
        rerank: bool,
    },
    /// Reflection / consolidation: cluster near-duplicate memories and
    /// (with --apply) create a canonical memory that supersedes them.
    /// Source memories are NEVER deleted — just marked superseded.
    ///
    /// Default is dry-run (preview only); pass --apply to write.
    Reflect {
        /// Apply the plan (without this, dry-run only)
        #[arg(long)]
        apply: bool,
        /// Cosine threshold for clustering (default 0.85)
        #[arg(long, default_value = "0.85")]
        threshold: f32,
        /// Limit pool size (default 2000)
        #[arg(long)]
        limit: Option<usize>,
        /// Only consider memories from the last N days
        #[arg(long)]
        since_days: Option<i64>,
        /// Emit JSON instead of human-readable text
        #[arg(long)]
        json: bool,
    },
    /// Work-activity tracking — accurate daily "time worked" from input
    /// idle, with history graph. Reads the daemon's activity.db; no
    /// daemon connection required (works even while the daemon runs).
    Activity {
        #[command(subcommand)]
        cmd: Option<ActivityCommands>,
    },
    /// Project-time attribution — map work sessions to projects (by the
    /// project-linked memories in each session's window). Honest: weak signal
    /// → Unattributed, confidence reported, no fake precision.
    Attribute {
        #[command(subcommand)]
        cmd: AttributeCommands,
    },
    /// Daily journal — a readable digest of one day's work (summary, projects
    /// with hours + bullets, decisions, follow-ups). Deterministic; the same
    /// contract as `GET /api/journal`. Defaults to today (local).
    Journal {
        /// Local date YYYY-MM-DD; defaults to today.
        #[arg(long)]
        day: Option<String>,
        /// Emit the raw JSON contract instead of the human view.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum AttributeCommands {
    /// Recompute the last N local days (default 14).
    Backfill {
        #[arg(long, default_value = "14")]
        days: u32,
    },
    /// Recompute today only.
    Run,
    /// Preview SEMANTIC (vector) attribution for a day WITHOUT writing to the
    /// DB: shows before (graph links only) → after (graph + k-NN semantic
    /// match), per project, plus per-memory reasons. Lets you eyeball the
    /// impact before enabling the write path.
    Semantic {
        /// Local date YYYY-MM-DD; defaults to today.
        #[arg(long)]
        day: Option<String>,
    },
    /// Preview day-level CARRY-FORWARD for one day WITHOUT writing: shows
    /// before (per-session direct) → after (+ carry-forward of the dominant
    /// project's no-signal neighbours), the carried sessions, and what stays
    /// Unattributed. Guarded by dominance / window / cap — see `attribution`.
    Carry {
        /// Local date YYYY-MM-DD; defaults to today.
        #[arg(long)]
        day: Option<String>,
    },
}

#[derive(Subcommand)]
enum ActivityCommands {
    /// Today's worked total + session count.
    Today {
        /// Emit JSON instead of text.
        #[arg(long)]
        json: bool,
    },
    /// History graph — worked time per day for the last N days.
    Week {
        /// Number of days to show (default 7).
        #[arg(long, default_value = "7")]
        days: u32,
        /// Emit JSON instead of the bar chart.
        #[arg(long)]
        json: bool,
    },
    /// One-shot JSON payload for the widget's main screen: worked today,
    /// live session, week stats, today's detail + timeline blocks, and
    /// the 7-day chart series. JSON only (always).
    Summary,
    /// Detail for one day: total, sessions, longest, span, and the
    /// session-timeline blocks. Defaults to today. JSON only (always).
    Day {
        /// Local date YYYY-MM-DD (default: today).
        #[arg(long)]
        date: Option<String>,
    },
    /// Projects (graph project-entities) with memory counts + latest
    /// memories. Time fields are null until attribution ships. JSON only.
    Projects {
        /// No-op (always JSON); accepted so the widget can pass it
        /// uniformly with the other `--json` activity subcommands.
        #[arg(long)]
        json: bool,
    },
}

#[derive(Subcommand)]
enum FactCommands {
    /// Show the currently-valid facts for a subject. "Current" means the
    /// most recent assertion of each (subject, predicate) pair — older
    /// values are hidden unless you pass --history.
    Current {
        /// Subject entity (case-insensitive; gets lowercased)
        subject: String,
        /// Show every fact ever recorded, not just the current ones.
        #[arg(long)]
        history: bool,
    },
    /// Add a new fact. If a current fact already exists for this
    /// (subject, predicate), it gets superseded automatically — its
    /// valid_to is set to the new fact's valid_from.
    Add {
        /// Subject entity (canonical name, e.g. "inventory-labeler")
        subject: String,
        /// Predicate (relation, e.g. "has-price", "deadline", "version")
        predicate: String,
        /// Value (free-form string, e.g. "$6k", "2026-08-01", "v3")
        value: String,
        /// Memory id this fact was extracted from. Required for audit —
        /// use --source manual when adding by hand from the CLI.
        #[arg(long, default_value = "manual")]
        source: String,
        /// Confidence in [0.0, 1.0]. Default 1.0 for explicit assertions.
        /// Inductive conclusions (future) will use < 1.0.
        #[arg(long, default_value = "1.0")]
        confidence: f32,
    },
}

#[derive(Subcommand)]
enum PeerCommands {
    /// List all known peers, most recently active first.
    List {
        /// Max entries (default 50)
        #[arg(short, long, default_value = "50")]
        limit: usize,
    },
    /// Add or refresh a peer. If a peer with this name already exists,
    /// updates last_seen_at and returns the existing id.
    Add {
        /// Canonical name (lowercased on save). Examples: "user",
        /// "claude", "codex", "client-alpha".
        name: String,
        /// Display name with original casing for UI output.
        #[arg(long)]
        display: Option<String>,
        /// "human" | "agent" | "system" | anything you want — free-form.
        #[arg(long, default_value = "human")]
        kind: String,
    },
    /// Show a peer's sessions, newest first.
    Sessions {
        /// Peer name (canonical, lowercased on lookup)
        name: String,
        /// Max entries (default 20)
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Merge two peers — moves all memory_peers links + sessions from
    /// `src` into `dst`, then deletes `src`. Useful when a default
    /// rename created a duplicate identity (e.g. `user` → `user`).
    Merge {
        /// Source peer (will be deleted after merge)
        src: String,
        /// Destination peer (absorbs src's links and sessions)
        dst: String,
    },
}

#[derive(Subcommand)]
enum ConclusionCommands {
    /// List conclusions for a subject — currently-valid ones by default,
    /// or the full history (current + superseded) with --history.
    /// Use "_global" as the subject for non-entity-specific claims.
    List {
        /// Subject entity (case-insensitive). Use "_global" for non-entity
        /// claims.
        subject: String,
        /// Include superseded conclusions, not just current ones.
        #[arg(long)]
        history: bool,
        /// Show evidence: print the supporting memory ids alongside each
        /// conclusion.
        #[arg(long)]
        with_sources: bool,
    },
    /// Add a conclusion manually. Source memory ids are positional after
    /// the statement — at least one is recommended for traceability, but
    /// not required (v1 supports curated insights that may not map to a
    /// single memory).
    Add {
        /// Subject entity (canonical name; "_global" for non-entity).
        subject: String,
        /// Claim text — the conclusion itself.
        statement: String,
        /// Free-form category. Common values: pattern, preference, trend,
        /// observation. Default "pattern".
        #[arg(long, default_value = "pattern")]
        kind: String,
        /// Confidence in [0.0, 1.0]. Inductive claims are < 1.0 by nature;
        /// the default reflects "looks likely, not certain".
        #[arg(long, default_value = "0.6")]
        confidence: f32,
        /// Memory ids supporting this conclusion. Pass zero or more —
        /// each one is linked via `conclusion_sources` for evidence.
        #[arg(long = "source", value_name = "MEMORY_ID")]
        sources: Vec<String>,
    },
    /// LLM-driven generation: auto-mine 3-5 inductive conclusions
    /// about a subject from the most recent memories mentioning it.
    /// Uses the same Ollama backend as the graph extractor.
    ///
    /// Default is dry-run preview — review the LLM's claims before
    /// they land in the conclusions table. Pass `--apply` to save.
    /// `--limit N` controls how many recent memories are fed to the
    /// LLM as context (default 25; raise on stronger models, lower
    /// on slow ones).
    Generate {
        /// Subject entity (canonical name; case-insensitive lookup
        /// against the entities table).
        subject: String,
        /// Max memories to feed as context to the LLM. Newest first.
        /// Default 25 — fits comfortably in qwen2.5:3b's window.
        #[arg(long, default_value = "25")]
        limit: usize,
        /// Persist the generated conclusions. Default is dry-run
        /// preview to keep hallucinations from polluting retrieval
        /// without a human pass first.
        #[arg(long)]
        apply: bool,
    },
    /// Mark a conclusion as superseded by a newer one. The old row
    /// stays in the table (history is preserved) but
    /// `current_conclusions_for_subject` will no longer return it.
    /// Use when an LLM-generated claim refines a previous one and
    /// you want retrieval to pick the new version.
    ///
    /// Both ids accept full UUIDs or unambiguous prefixes (min 8
    /// chars, same contract as `session show`). The two ids MUST
    /// be different — storage enforces it.
    Supersede {
        /// Old conclusion id (will be marked superseded).
        old_id: String,
        /// New conclusion id that replaces the old one. Must
        /// already exist via `conclusion add` / `conclusion generate
        /// --apply`.
        new_id: String,
    },
    /// Delete a conclusion permanently. Cascades through the
    /// `conclusion_sources` link table (those rows are removed
    /// too); the source memories themselves stay intact.
    ///
    /// Use this for cleanup when `conclusion generate --apply`
    /// produced a duplicate or low-quality claim. Prefer
    /// `supersede` over delete when you want to preserve the
    /// history of how a claim evolved.
    ///
    /// Accepts full UUID or unambiguous prefix (min 8 chars).
    Delete {
        /// Conclusion id (full UUID or unambiguous 8+ char prefix).
        id: String,
    },
}

#[derive(Subcommand)]
enum SessionCommands {
    /// List sessions, newest first.
    ///
    /// Default scope (no flags): open sessions only, across all peers.
    /// We default to open-only because the full history of every peer
    /// can run into thousands of rows — surface what's live first,
    /// require an explicit `--peer name` to dive into closed history
    /// for one peer.
    ///
    /// Combinations:
    /// - `--open` alone — same as default (open across all peers)
    /// - `--peer name` — every session for that peer (open + closed)
    /// - `--peer name --open` — open sessions for that peer
    List {
        /// Show only sessions that are still open (no ended_at).
        /// Default behavior already filters to open across all peers;
        /// this flag becomes meaningful when combined with `--peer`.
        #[arg(long)]
        open: bool,
        /// Filter to one peer (canonical name, lowercased on lookup).
        /// Without this flag the listing stays open-only to avoid
        /// dumping every peer's full history.
        #[arg(long)]
        peer: Option<String>,
        /// Max entries (default 30).
        #[arg(short, long, default_value = "30")]
        limit: usize,
    },
    /// Show a session by id — metadata + every memory captured in it,
    /// oldest first, like a transcript. The id can be a prefix (first 8+
    /// chars) for convenience. Shorter prefixes are rejected loudly
    /// rather than silently grabbing the first match.
    Show {
        /// Session id (full UUID or unambiguous prefix, min 8 chars).
        id: String,
    },
}

#[derive(Subcommand)]
enum DreamCommands {
    /// Generate a session_summary memory for one session by id.
    /// Refuses if a summary already exists (regeneration is not
    /// implemented yet). Refuses OPEN sessions by default to avoid
    /// freezing a stale "ongoing" snapshot — pass `--allow-open`
    /// only when you intentionally want a snapshot of an active
    /// session. Useful for testing the summarizer output on a
    /// known session before batch runs.
    Run {
        /// Session id (full UUID or unambiguous prefix, min 8 chars).
        id: String,
        /// Permit snapshotting an open session. Default refuses
        /// because the metadata-link duplicate check would otherwise
        /// freeze the first snapshot forever even as the session
        /// keeps growing. Use only for testing or intentional
        /// snapshots.
        #[arg(long)]
        allow_open: bool,
        /// Use the LLM (Ollama) summarizer instead of the heuristic
        /// counts+anchors layout. Requires `[llm] enabled = true`
        /// in config. Produces prose narrative summaries (2-4 sentences).
        /// Costs one LLM call per invocation; falls back to heuristic
        /// on backend failure (caller can retry without --llm).
        #[arg(long)]
        llm: bool,
        /// Replace an existing canonical summary with a fresh one
        /// instead of skipping. Forgets the prior summary first.
        /// Common case: a session got a heuristic summary from
        /// the dream worker, you now want to upgrade it to the
        /// LLM version via `--llm --regenerate`.
        #[arg(long)]
        regenerate: bool,
    },
    /// Batch-summarize recently-closed sessions. Picks closed
    /// sessions ended in the last `--since-hours` hours, skips any
    /// that already have a summary, runs the heuristic summarizer
    /// on the rest. Save with --apply; default is dry-run preview.
    Batch {
        /// Look at sessions whose ended_at is within the last N
        /// hours. Default 24 — matches the typical "process last
        /// night's work" cadence.
        #[arg(long, default_value = "24")]
        since_hours: u64,
        /// Cap on sessions processed per run. Bounded so a fresh
        /// run on a long-uptime DB doesn't dump 10k summaries.
        #[arg(long, default_value = "50")]
        limit: usize,
        /// Without --apply, print what WOULD be summarized but
        /// don't save anything. The default is dry-run to keep
        /// dream runs reviewable.
        #[arg(long)]
        apply: bool,
        /// Use the LLM summarizer for each session in the batch.
        /// One LLM call per session — slower than heuristic, much
        /// better prose. Same `[llm] enabled` gating as `run --llm`.
        #[arg(long)]
        llm: bool,
        /// Process sessions even if they already have a canonical
        /// summary — forgets the old one and writes a fresh one.
        /// Pair with `--llm` to bulk-upgrade heuristic summaries
        /// to LLM prose. Without this flag, existing summaries are
        /// skipped (idempotent default).
        #[arg(long)]
        regenerate: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config = Config::load()?;

    // Merge the user's private [graph] vocabulary (projects/tech/people/deny)
    // into the rule extractor once, for every command in this process. The
    // public source ships only generic defaults; personal names live in the
    // local config file and never reach the open-source repo.
    graph::extractor::init_user_lists(&config.graph);
    // Same for the private alias map (real tool->project mappings used by
    // attribution): generic public defaults + the user's local [graph.aliases].
    semantic_attribution::init_user_aliases(&config.graph.aliases);

    // Daemon and foreground start: log to file. Everything else: log to stderr.
    match &cli.command {
        Commands::Start { .. } => init_logging(Some(&config.daemon.log_file)),
        Commands::Mcp | Commands::Stats { .. } => {} // stdout is structured output, no tracing
        _ => init_logging(None),
    }

    match cli.command {
        Commands::Start { daemon: bg } => {
            // Liveness-aware start: distinguish genuine running from
            // stale-PID-after-crash. Codex's review found that the
            // previous check refused start when PID file pointed at
            // a long-dead process, leaving launchd in a flap loop and
            // the widget showing "Stopped". Now we auto-clean stale
            // state and proceed; refuse only when something genuinely
            // alive (or hung but reachable) holds the slot.
            match daemon::Daemon::status_check(&config) {
                daemon::DaemonStatus::Stopped => {}
                daemon::DaemonStatus::StalePid { pid } => {
                    eprintln!("Cleaning up stale PID file ({pid} is dead); starting fresh daemon");
                    daemon::Daemon::cleanup_stale_state(&config);
                }
                daemon::DaemonStatus::Hung { pid } => {
                    eprintln!(
                        "mnemonic appears hung (PID {pid} alive but socket dead). \
                         Run `mnemonic stop` to clear it, then start again."
                    );
                    std::process::exit(1);
                }
                daemon::DaemonStatus::Running { pid } => {
                    eprintln!("mnemonic is already running (PID {pid})");
                    std::process::exit(1);
                }
            }

            if bg {
                // Single-owner guard: if a launchd LaunchAgent already
                // manages the daemon, a manual background start spawns a
                // second daemon that races launchd's KeepAlive — the
                // dual-daemon hang we burned hours on. Refuse and point
                // at the launchd-native restart. When NO LaunchAgent is
                // loaded (the common case for most installs / Linux),
                // `start -d` is a fully supported way to run in the
                // background — no warning.
                if daemon::Daemon::launchd_service_loaded() {
                    let uid = unsafe { libc::getuid() };
                    let label = daemon::Daemon::LAUNCHD_LABEL;
                    eprintln!(
                        "`mnemonic start -d` refused: the launchd service `{label}` is \
                         loaded and already manages the daemon.\n\
                         To (re)start it without racing launchd, run:\n\
                         \x20 launchctl kickstart -k gui/{uid}/{label}\n\
                         (or `mnemonic restart`, which does this for you)."
                    );
                    std::process::exit(1);
                }
                daemonize()?;
            } else {
                let d = Daemon::new(config);
                d.run().await?;
            }
        }
        Commands::Stop => {
            // Timeout-aware stop: SIGTERM → wait 5s → SIGKILL →
            // cleanup files. The previous version printed "Stopped"
            // immediately after SIGTERM without verifying, which is
            // how we ended up with the launchd PID-deadlock that
            // required a reboot threat to recover.
            match daemon::Daemon::stop_running_daemon(&config, 5) {
                Ok(daemon::StopOutcome::AlreadyStopped) => {
                    println!("mnemonic is not running");
                }
                Ok(daemon::StopOutcome::StaleCleaned { pid }) => {
                    println!("Cleaned up stale PID {pid} (process was already dead)");
                }
                Ok(daemon::StopOutcome::GracefulExit { pid }) => {
                    println!("Stopped mnemonic (PID {pid}, graceful)");
                }
                Ok(daemon::StopOutcome::ForcedExit { pid }) => {
                    println!("Force-killed mnemonic (PID {pid}) — SIGTERM ignored, SIGKILL worked");
                }
                Ok(daemon::StopOutcome::ForcedAndStuck { pid }) => {
                    eprintln!(
                        "Force-kill sent to PID {pid} but process still alive \
                         (likely uninterruptible kernel sleep). \
                         Reboot is the only reliable recovery. \
                         PID and socket files have been cleaned regardless."
                    );
                    std::process::exit(2);
                }
                Err(e) => {
                    eprintln!("mnemonic stop failed: {e}");
                    std::process::exit(1);
                }
            }
        }
        Commands::Restart => {
            restart_daemon(&config)?;
        }
        Commands::Status => {
            // Show the structured 4-state result so the user sees
            // hung daemons explicitly (was indistinguishable from
            // "running" in the previous check).
            let st = daemon::Daemon::status_check(&config);
            match &st {
                daemon::DaemonStatus::Stopped => println!("mnemonic is not running"),
                daemon::DaemonStatus::StalePid { pid } => println!(
                    "mnemonic has a stale PID file (PID {pid} is dead) — run `mnemonic stop` to clean"
                ),
                daemon::DaemonStatus::Hung { pid } => println!(
                    "mnemonic is HUNG (PID {pid} alive but API socket unresponsive) — run `mnemonic stop`"
                ),
                daemon::DaemonStatus::Running { pid } => {
                    println!("mnemonic is running (PID {pid})")
                }
            }

            if config.storage.db_path.exists() {
                let st = storage::Storage::open(&config.storage.db_path)?;
                let stats = st.stats()?;
                println!("\n{stats}");
                // Async extraction queue: memories saved but not yet
                // extracted by the background worker. Steady state is 0-N
                // depending on `extraction.worker_interval_secs`; a number
                // climbing into the hundreds means the worker is falling
                // behind (Ollama is slow, or async_enabled is off but the
                // queue still has stale rows).
                if let Ok(n) = st.extraction_queue_count()
                    && n > 0
                {
                    println!("  · {n} memories pending async extraction (worker will pick up)");
                }
                // Retry queue: memories whose LLM extraction failed and
                // are backing off. Non-zero usually means Ollama was
                // unreachable — run `mnemonic reextract --pending` once
                // the backend is healthy.
                if let Ok(n) = st.pending_extractions_count()
                    && n > 0
                {
                    println!(
                        "  ⏳ {n} memories waiting for LLM extraction retry \
                         (`mnemonic reextract --pending` to drain)"
                    );
                }
                // Temporal facts — assertions about subjects with valid_from /
                // valid_to. Surfaced once non-zero so the user notices the
                // table is being populated; deep inspection via
                // `mnemonic fact current <subject>`.
                if let Ok(n) = st.facts_count()
                    && n > 0
                {
                    println!(
                        "  ◆ {n} facts recorded (`mnemonic fact current <subject>` to inspect)"
                    );
                }
                // Inductive conclusions — one layer above facts. Today
                // only populated via `mnemonic conclusion add`; v2 will
                // wire the async extraction worker to generate them.
                if let Ok(n) = st.conclusions_count()
                    && n > 0
                {
                    println!(
                        "  ✦ {n} conclusion{} recorded (`mnemonic conclusion list <subject>` to inspect)",
                        if n == 1 { "" } else { "s" }
                    );
                }
                // Peers/sessions. Today only populated via `mnemonic peer
                // add`; the conversation watcher will auto-tag in a
                // follow-up commit. Surfaces ⚑ when any peer exists,
                // plus a separate warning when sessions are open
                // (potential leak — never ended).
                if let Ok(n) = st.peers_count()
                    && n > 0
                {
                    // Use the dedicated COUNT(*) helper, not
                    // `open_sessions(N).len()` — that one is capped at N
                    // and would lie ("1 open" when 12 are really open).
                    let open = st.open_sessions_count().unwrap_or(0);
                    let session_note = if open > 0 {
                        format!(", {open} session(s) open")
                    } else {
                        String::new()
                    };
                    println!(
                        "  ⚑ {n} peer{} known{session_note} (`mnemonic peer list`)",
                        if n == 1 { "" } else { "s" }
                    );
                }
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
        Commands::Recent { limit, json } => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            let results = st.recent(limit)?;

            if json {
                let total = st.stats().map(|s| s.total).unwrap_or(0);
                let items: Vec<_> = results
                    .iter()
                    .map(|e| {
                        serde_json::json!({
                            "id": e.id,
                            "type": e.memory_type.to_string(),
                            "title": e.title,
                            "content": e.content,
                            "timestamp": e.timestamp.to_rfc3339(),
                            "importance": e.importance,
                        })
                    })
                    .collect();
                println!("{}", serde_json::json!({ "total": total, "items": items }));
            } else if results.is_empty() {
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
            let embedder = embedding::create_embedder()?;
            let embed_text = format!("{} {}", entry.title, entry.content);
            if let Ok(emb) = embedder.embed(&embed_text) {
                // `?`: a dimension-mismatch error (model swapped without a reembed)
                // must abort the save, not fall through and write a mixed-dim vector.
                if let Some(sim) = st.is_duplicate(&emb, config.classifier.dedup_threshold)? {
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
            if config.output.memory_api_enabled && !config.output.memory_api_url.is_empty() {
                let sink = output::memory_api::MemoryApiSink::new(
                    config.output.memory_api_url.clone(),
                    config.output.memory_api_key.clone(),
                );
                sink.write(&entry)?;
            }

            println!("Saved: [{}] {}", entry.memory_type, title);
        }
        Commands::Similar { text, limit } => {
            let st = storage::Storage::open(&config.storage.db_path)?;

            let embedder = embedding::create_embedder()?;
            let query_emb = embedder.embed_query(&text)?;
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
        Commands::Forget { id } => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            // Storage already validates the id format implicitly via
            // the UPDATE/DELETE chain — pass-through. `forget_by_id`
            // returns true when a row was actually removed; false on
            // unknown id (better than erroring so retries are safe).
            let removed = st.forget_by_id(&id)?;
            if removed {
                println!("Forgot memory {}", &id[..8.min(id.len())]);
            } else {
                println!("No memory with id `{id}` (already gone or never existed)");
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

            // Daemon lifecycle — 4-state check so the user sees hung
            // / stale / running distinctly. Codex's review pointed
            // out that the old doctor lumped everything into "running
            // or not", missing exactly the failure modes that
            // produced the recent recovery dance.
            let status = daemon::Daemon::status_check(&config);
            match &status {
                daemon::DaemonStatus::Running { pid } => {
                    println!("✓ Daemon running (PID {pid}, socket responsive)");
                }
                daemon::DaemonStatus::Hung { pid } => {
                    println!("✗ Daemon HUNG (PID {pid} alive, socket dead)");
                    println!("  → Run: mnemonic stop  (will SIGTERM then SIGKILL after 5s)");
                    issues += 1;
                }
                daemon::DaemonStatus::StalePid { pid } => {
                    println!("✗ Stale PID file (PID {pid} is dead)");
                    println!("  → Run: mnemonic stop  (will clean up files)");
                    issues += 1;
                }
                daemon::DaemonStatus::Stopped => {
                    println!("✗ Daemon not running");
                    println!("  → Run: mnemonic start -d");
                    issues += 1;
                }
            }

            // PID file: exists / readable / parses
            let pid_path = &config.daemon.pid_file;
            if pid_path.exists() {
                match std::fs::read_to_string(pid_path) {
                    Ok(s) if s.trim().parse::<u32>().is_ok() => {
                        println!("✓ PID file: {} ({})", pid_path.display(), s.trim());
                    }
                    Ok(s) => {
                        println!("✗ PID file exists but unparseable: {:?}", s.trim());
                        issues += 1;
                    }
                    Err(e) => {
                        println!("✗ PID file unreadable: {e}");
                        issues += 1;
                    }
                }
            } else {
                println!("- PID file: not present (daemon stopped or never started)");
            }

            // Socket file: exists / reachable
            let socket_path = &config.daemon.socket_path;
            if socket_path.exists() {
                let probe_ok = daemon::Daemon::probe_socket_for_doctor(socket_path);
                if probe_ok {
                    println!(
                        "✓ API socket: {} (accepting connections)",
                        socket_path.display()
                    );
                } else {
                    println!(
                        "✗ API socket present but not accepting connections: {}",
                        socket_path.display()
                    );
                    issues += 1;
                }
            } else {
                println!("- API socket: not present");
            }

            // Dashboard HTTP port (if UI enabled)
            if config.ui.enabled {
                let port = config.ui.port;
                let bound = std::net::TcpStream::connect_timeout(
                    &format!("127.0.0.1:{port}").parse().unwrap(),
                    std::time::Duration::from_millis(300),
                )
                .is_ok();
                if bound {
                    println!("✓ Dashboard HTTP: 127.0.0.1:{port} (responding)");
                } else {
                    println!("✗ Dashboard HTTP: 127.0.0.1:{port} (no response)");
                    if matches!(status, daemon::DaemonStatus::Running { .. }) {
                        // Only count as an issue if the daemon claims
                        // healthy but the port doesn't bind.
                        issues += 1;
                    }
                }
            } else {
                println!("- Dashboard HTTP: disabled in config");
            }

            // launchctl service — only meaningful on macOS, and only
            // if user opted into the LaunchAgent. The plist path is
            // the conventional one we ship with the project.
            #[cfg(target_os = "macos")]
            {
                let label = "com.kossvat.mnemonic.daemon";
                let out = std::process::Command::new("launchctl").arg("list").output();
                match out {
                    Ok(o) if o.status.success() => {
                        let stdout = String::from_utf8_lossy(&o.stdout);
                        let line = stdout.lines().find(|l| l.contains(label));
                        match line {
                            Some(l) => {
                                // launchctl list format: PID Status Label
                                // PID == "-" means service loaded but not running
                                let cols: Vec<&str> = l.split_whitespace().collect();
                                if let (Some(pid), Some(status_code)) = (cols.first(), cols.get(1))
                                {
                                    println!(
                                        "✓ launchctl service `{label}`: PID={pid} last_exit={status_code}"
                                    );
                                } else {
                                    println!("✓ launchctl service `{label}` loaded");
                                }
                            }
                            None => {
                                println!("- launchctl service `{label}` not loaded");
                            }
                        }
                    }
                    _ => println!("- launchctl: not available"),
                }
            }

            // Binary location consistency check. Codex caught today's
            // foot-gun: two copies of `mnemonic` in different PATH dirs
            // (~/.local/bin and ~/.cargo/bin) drifted out of sync. The
            // shell resolved to the stale `.local/bin` copy after every
            // `cargo install` because `.local/bin` was higher in $PATH.
            // Surface the mismatch loudly so the user notices before
            // wasting another hour on "why is the new code not running".
            let current_exe = std::env::current_exe().ok();
            let shell_resolved = std::process::Command::new("which")
                .arg("mnemonic")
                .output()
                .ok()
                .and_then(|o| {
                    if o.status.success() {
                        String::from_utf8(o.stdout)
                            .ok()
                            .map(|s| std::path::PathBuf::from(s.trim()))
                    } else {
                        None
                    }
                });
            let launchctl_program = {
                #[cfg(target_os = "macos")]
                {
                    let label = "com.kossvat.mnemonic.daemon";
                    std::process::Command::new("launchctl")
                        .args([
                            "print",
                            &format!("gui/{}/{label}", unsafe { libc::getuid() }),
                        ])
                        .output()
                        .ok()
                        .and_then(|o| String::from_utf8(o.stdout).ok())
                        .and_then(|s| {
                            // Extract `program = /path/to/mnemonic` from
                            // launchctl print output. Format may evolve,
                            // so be forgiving — None means "couldn't
                            // determine", which we won't flag as error.
                            s.lines()
                                .find(|l| l.trim().starts_with("program ="))
                                .and_then(|l| l.split('=').nth(1))
                                .map(|p| std::path::PathBuf::from(p.trim()))
                        })
                }
                #[cfg(not(target_os = "macos"))]
                {
                    None::<std::path::PathBuf>
                }
            };

            // Report what we found. Show each path explicitly so the
            // user can copy-paste fixes; warn only when paths disagree.
            if let Some(exe) = &current_exe {
                println!("✓ This binary: {}", exe.display());
            }
            if let Some(sh) = &shell_resolved {
                println!("✓ Shell resolves `mnemonic` to: {}", sh.display());
            }
            if let Some(lc) = &launchctl_program {
                println!("✓ launchctl runs: {}", lc.display());
            }

            // Compare canonicalized paths (resolves symlinks); fall
            // back to lexical compare when canonicalize fails. The
            // resolved paths only need to match each other; absolute
            // sameness doesn't matter as long as a single binary is
            // the source of truth.
            let canon = |p: &std::path::Path| -> std::path::PathBuf {
                std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
            };
            if let (Some(exe), Some(sh)) = (&current_exe, &shell_resolved)
                && canon(exe) != canon(sh)
            {
                println!(
                    "⚠ Binary mismatch: shell `which mnemonic` points at a different file than \
                     the one you just invoked."
                );
                println!(
                    "  → Resync: cp '{}' '{}' && codesign --force --sign - '{}'",
                    exe.display(),
                    sh.display(),
                    sh.display()
                );
                issues += 1;
            }
            if let (Some(exe), Some(lc)) = (&current_exe, &launchctl_program)
                && canon(exe) != canon(lc)
            {
                println!(
                    "⚠ launchctl runs a different binary than this CLI. Daemon and CLI \
                     may behave inconsistently after upgrades."
                );
                println!(
                    "  → Resync: cp '{}' '{}' && codesign --force --sign - '{}'",
                    exe.display(),
                    lc.display(),
                    lc.display()
                );
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

            // (socket + dashboard HTTP checks moved to lifecycle section above)

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

            // Graph stats
            let (graph_entities, graph_edges) = st.graph_stats().unwrap_or((0, 0));
            let top_entities: Vec<serde_json::Value> = st.list_entities(5)
                .unwrap_or_default()
                .iter()
                .map(|(name, etype, count)| {
                    serde_json::json!({"name": name, "type": etype, "mentions": count})
                })
                .collect();

            if json {
                let daily_json: Vec<serde_json::Value> = daily
                    .iter()
                    .map(|(date, count)| serde_json::json!({"date": date, "count": count}))
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
                    "graph_entities": graph_entities,
                    "graph_edges": graph_edges,
                    "top_entities": top_entities,
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

                if graph_entities > 0 {
                    println!("\nKnowledge graph: {graph_entities} entities, {graph_edges} edges");
                }

                if let Some(pid) = is_running {
                    println!("\nDaemon: running (PID {pid})");
                } else {
                    println!("\nDaemon: stopped");
                }
            }
        }
        Commands::Graph { entity } => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            let result = st.graph_query(&entity)?;

            if !result.found {
                println!("Entity '{}' not found in graph.", entity);
                if let Some(canonical) = st.canonical_for_alias(&entity)? {
                    println!("'{}' is an alias of '{}'.", entity, canonical);
                    println!("\nTry: mnemonic graph {}", canonical);
                }
                println!("\nTip: run 'mnemonic entities' to see known entities,");
                println!("     or 'mnemonic backfill' to build graph from existing memories.");
            } else {
                let alias_suffix = if result.aliases.is_empty() {
                    String::new()
                } else {
                    format!(" (also: {})", result.aliases.join(", "))
                };
                println!(
                    "Entity: {}{} ({}, mentioned {} times)",
                    result.entity_name, alias_suffix, result.entity_type, result.mention_count
                );
                println!("First seen: {}", result.first_seen);
                println!("Last seen:  {}", result.last_seen);

                if !result.edges.is_empty() {
                    println!("\nConnections:");
                    for edge in &result.edges {
                        println!(
                            "  {} --{}→ {} (weight: {:.1})",
                            edge.source, edge.relation, edge.target, edge.weight
                        );
                    }
                }

                if !result.neighbors.is_empty() {
                    println!("\nConnected entities:");
                    for n in &result.neighbors {
                        println!(
                            "  {} ({}, {} mentions)",
                            n.name, n.entity_type, n.mention_count
                        );
                    }
                }

                if !result.memories.is_empty() {
                    println!("\nRelated memories:");
                    for m in &result.memories {
                        println!(
                            "  [{:>10}] {} (importance: {:.1})",
                            m.memory_type, m.title, m.importance
                        );
                        println!("             {}", m.timestamp);
                    }
                }
            }
        }
        Commands::Entities { limit } => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            let entities = st.list_entities(limit)?;
            let (entity_count, edge_count) = st.graph_stats()?;

            println!("Knowledge graph: {entity_count} entities, {edge_count} edges\n");

            if entities.is_empty() {
                println!(
                    "No entities yet. Run 'mnemonic backfill' to build from existing memories."
                );
            } else {
                for (name, etype, count) in &entities {
                    println!("  {name:20} ({etype:8}) — {count} mentions");
                }
            }
        }
        Commands::Backfill => {
            let st = storage::Storage::open(&config.storage.db_path)?;
            let all = st.recent(1000)?; // Get all memories
            let extractor = graph::extractor::RuleExtractor::new();
            use graph::extractor::EntityExtractor;

            let mut total_entities = 0;
            let mut total_edges = 0;

            for entry in &all {
                let result = extractor.extract(entry);
                st.replace_graph_and_reconcile_projects(entry, &result.entities, &result.edges)?;
                total_entities += result.entities.len();
                total_edges += result.edges.len();
            }

            let (entity_count, edge_count) = st.graph_stats()?;
            println!("Backfill complete:");
            println!("  Processed: {} memories", all.len());
            println!("  Extracted: {total_entities} entity mentions, {total_edges} edges");
            println!("  Graph now: {entity_count} unique entities, {edge_count} edges");
        }
        Commands::BackfillObsidian { force, vault } => {
            use crate::storage::OutputSink;
            let vault_path = match vault {
                Some(p) => {
                    if let Some(rest) = p.strip_prefix("~/") {
                        dirs::home_dir().unwrap_or_default().join(rest)
                    } else {
                        std::path::PathBuf::from(p)
                    }
                }
                None => {
                    if !config.output.obsidian_enabled {
                        eprintln!(
                            "Obsidian sync disabled in config. Pass --vault PATH or enable \
                             [output] obsidian_enabled = true in ~/.config/mnemonic/config.toml"
                        );
                        std::process::exit(1);
                    }
                    config.output.obsidian_path.clone()
                }
            };

            let st = storage::Storage::open(&config.storage.db_path)?;
            let all = st.recent(10_000)?;
            let sink = output::obsidian::ObsidianSink::new(vault_path.clone());

            let notes_dir = vault_path.join("Agents/Mnemonic/Notes");
            std::fs::create_dir_all(&notes_dir)?;

            let mut written = 0usize;
            let mut skipped = 0usize;
            for entry in &all {
                let date = entry.timestamp.format("%Y-%m-%d");
                let slug = output::obsidian::ObsidianSink::slug_for(&entry.title);
                let path = notes_dir.join(format!("{date}-{slug}.md"));
                if path.exists() && !force {
                    skipped += 1;
                    continue;
                }
                sink.write(entry)?;
                written += 1;
            }

            println!("Obsidian backfill complete:");
            println!("  Vault: {}", vault_path.display());
            println!("  Processed: {} memories", all.len());
            println!("  Wrote: {written}");
            println!("  Skipped (already exist): {skipped}");
            if !force && skipped > 0 {
                println!("  Pass --force to overwrite existing notes");
            }
        }
        Commands::Reembed => {
            // Safety check: refuse to run while daemon holds the DB. Mixing
            // dimensions in HNSW would crash the daemon on the next insert.
            if std::path::Path::new(&config.daemon.pid_file).exists() {
                eprintln!(
                    "Daemon is running (pid file exists at {}). Stop it first:\n  mnemonic stop",
                    config.daemon.pid_file.display()
                );
                std::process::exit(1);
            }

            // Open without HNSW — we may have mixed-dimension vectors mid-migration
            // which would panic rebuild_hnsw_index. HNSW is rebuilt fresh on daemon restart.
            let st = storage::Storage::open_no_hnsw(&config.storage.db_path)?;
            // All active rows (not a fixed 10k window): the startup dim-guard
            // checks every active row, so a partial reembed would keep the
            // daemon blocked. count() is an upper bound (includes superseded).
            let all = st.recent(st.count()?.max(1))?;
            if all.is_empty() {
                println!("No memories to re-embed.");
                return Ok(());
            }

            let embedder = embedding::create_embedder()?;
            let first = embedder.embed("probe")?;
            println!(
                "Re-embedding {} memories with {}-dim embedder...",
                all.len(),
                first.len()
            );

            let mut ok = 0usize;
            let mut err = 0usize;
            for (i, entry) in all.iter().enumerate() {
                let text = format!("{}\n{}", entry.title, entry.content);
                match embedder.embed(&text) {
                    Ok(emb) => {
                        st.update_embedding(&entry.id, &emb)?;
                        ok += 1;
                    }
                    Err(e) => {
                        eprintln!(
                            "  [{}/{}] {} — embed failed: {e}",
                            i + 1,
                            all.len(),
                            entry.title
                        );
                        err += 1;
                    }
                }
                if (i + 1) % 10 == 0 {
                    println!("  progress: {}/{}", i + 1, all.len());
                }
            }

            println!("\nRe-embed complete:");
            println!("  Success: {ok}");
            println!("  Failed:  {err}");
            println!(
                "\nRestart the daemon to rebuild HNSW with new embeddings:\n  mnemonic start -d"
            );
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
        Commands::Upgrade => {
            upgrade(&config)?;
        }
        Commands::DedupeGraph { dry_run } => {
            dedupe_graph(&config, dry_run)?;
        }
        Commands::BacklinkProjects => {
            let mem = storage::Storage::open(&config.storage.db_path)?;
            let scanned = mem.reconcile_all_projects()?;
            println!(
                "Reconciled project links across {scanned} memories (meta stripped, commits pinned to scope, notes alias-backlinked)."
            );
            println!("Run `mnemonic attribute backfill` to recompute attributed time.");
        }
        Commands::Fact(sub) => {
            run_fact_command(&config, sub)?;
        }
        Commands::Peer(sub) => {
            run_peer_command(&config, sub)?;
        }
        Commands::Conclusion(sub) => {
            run_conclusion_command(&config, sub)?;
        }
        Commands::Session(sub) => {
            run_session_command(&config, sub)?;
        }
        Commands::Dream(sub) => {
            run_dream_command(&config, sub)?;
        }
        Commands::Reextract {
            since_days,
            limit,
            dry_run,
            include_superseded,
            clean_graph,
            force,
            pending,
            discard_pending,
        } => {
            if pending {
                reextract_pending(&config, limit, dry_run, force, discard_pending)?;
            } else {
                reextract(
                    &config,
                    since_days,
                    limit,
                    dry_run,
                    include_superseded,
                    clean_graph,
                    force,
                )?;
            }
        }
        Commands::Eval {
            file,
            json,
            no_graph_hop,
            rerank,
        } => {
            run_eval(&config, &file, json, no_graph_hop, rerank)?;
        }
        Commands::Activity { cmd } => {
            run_activity(&config, cmd)?;
        }
        Commands::Attribute { cmd } => {
            let mem = storage::Storage::open(&config.storage.db_path)?;
            let act = activity::ActivityStore::open(&config.activity_db_path())?;
            let acfg = attribution::AttribCfg::default();
            match cmd {
                AttributeCommands::Backfill { days } => {
                    let n = attribution_worker::backfill(&act, &mem, days, &acfg);
                    println!("Attribution: recomputed {n} day(s).");
                }
                AttributeCommands::Run => {
                    let today = chrono::Local::now().date_naive();
                    attribution_worker::recompute_day(&act, &mem, today, &acfg)?;
                    println!("Attribution: recomputed today.");
                }
                AttributeCommands::Semantic { day } => {
                    let day = match day {
                        Some(s) => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                            .map_err(|_| anyhow::anyhow!("bad --day {s:?}, expected YYYY-MM-DD"))?,
                        None => chrono::Local::now().date_naive(),
                    };
                    semantic_dry_run(&mem, &act, day, &acfg)?;
                }
                AttributeCommands::Carry { day } => {
                    let day = match day {
                        Some(s) => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                            .map_err(|_| anyhow::anyhow!("bad --day {s:?}, expected YYYY-MM-DD"))?,
                        None => chrono::Local::now().date_naive(),
                    };
                    carry_dry_run(&mem, &act, day, &acfg)?;
                }
            }
        }
        Commands::Journal { day, json } => {
            let mem = storage::Storage::open(&config.storage.db_path)?;
            let act = activity::ActivityStore::open(&config.activity_db_path())?;
            let day = match day {
                Some(s) => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .map_err(|_| anyhow::anyhow!("bad --day {s:?}, expected YYYY-MM-DD"))?,
                None => chrono::Local::now().date_naive(),
            };
            let digest = journal::collect(&mem, &act, day)?;
            if json {
                println!("{}", serde_json::to_string(&digest)?);
            } else {
                print_journal(&digest);
            }
        }
        Commands::Reflect {
            apply,
            threshold,
            limit,
            since_days,
            json,
        } => {
            reflect(&config, apply, threshold, limit, since_days, json)?;
        }
    }

    Ok(())
}

/// Human-readable rendering of a day's journal for the CLI.
fn print_journal(d: &journal::JournalDay) {
    let hm = |secs: f64| -> String {
        let m = (secs / 60.0).round() as i64;
        if m >= 60 {
            format!("{}h {}m", m / 60, m % 60)
        } else {
            format!("{m}m")
        }
    };
    println!("📓 Journal — {}\n", d.day);
    println!("{}\n", d.summary);
    if !d.projects.is_empty() {
        println!("BY PROJECT");
        for p in &d.projects {
            let conf = p.confidence.as_deref().unwrap_or("-");
            println!("  • {} — {} ({})", p.name, hm(p.seconds), conf);
            for b in &p.bullets {
                println!("      - {b}");
            }
        }
        if d.unattributed_seconds > 0.5 {
            println!("  • Unattributed — {}", hm(d.unattributed_seconds));
        }
        println!();
    }
    if !d.decisions.is_empty() {
        println!("DECISIONS");
        for x in &d.decisions {
            println!("  • {}", x.title);
        }
        println!();
    }
    if !d.follow_ups.is_empty() {
        println!("FOLLOW-UPS");
        for x in &d.follow_ups {
            println!("  • {}", x.title);
        }
        println!();
    }
}

/// Read-only preview of day-level carry-forward for one local day. Mirrors the
/// per-session direct attribution `recompute_day` runs, then applies
/// `carry_forward_day`, and prints before → after + the carried sessions.
/// Writes NOTHING — the safety belt before wiring carry into the write path.
fn carry_dry_run(
    mem: &storage::Storage,
    act: &activity::ActivityStore,
    day: chrono::NaiveDate,
    acfg: &attribution::AttribCfg,
) -> anyhow::Result<()> {
    use attribution::{CarryCfg, carry_forward_day};
    const SIGNAL_PAD_MINUTES: i64 = 10;
    const MIN_PROJECT_MEMS: i64 = 2;

    let pad = chrono::Duration::minutes(SIGNAL_PAD_MINUTES);
    // Same per-session build the live recompute uses — one shared computation.
    let day_sessions = act.day_sessions_with_signals(day, acfg, pad, |s, e| {
        mem.project_signals_in_window(s, e, MIN_PROJECT_MEMS)
            .unwrap_or_default()
    })?;

    // The day's project-memory timestamps drive the carry window guard.
    let (day_start, day_end) = activity::local_day_bounds_utc(day)
        .ok_or_else(|| anyhow::anyhow!("could not resolve local day bounds for {day}"))?;
    let mem_times = mem.project_mem_times_in_window(day_start, day_end, MIN_PROJECT_MEMS)?;

    let ccfg = CarryCfg::default();
    let after = carry_forward_day(&day_sessions, &mem_times, &ccfg);
    // "before" = same call with no memory timestamps → carry can't fire, so it
    // is exactly the direct per-session aggregation (confidence by seconds).
    let before = carry_forward_day(&day_sessions, &[], &ccfg);

    let hm = |secs: f64| -> String {
        let m = (secs / 60.0).round() as i64;
        if m >= 60 {
            format!("{}h {:02}m", m / 60, m % 60)
        } else {
            format!("{m}m")
        }
    };
    // Attribution keys are entity-id UUIDs; show the project name for review.
    let name = |key: &str| -> String {
        mem.entity_name(key)
            .ok()
            .flatten()
            .unwrap_or_else(|| key.to_string())
    };

    println!("Carry-forward dry-run — {day}  (NO DB writes)");
    println!(
        "  guards: dominance {:.0}% · window {}m · cap {:.0}% of day\n",
        ccfg.dominance * 100.0,
        ccfg.window_minutes,
        ccfg.cap_fraction * 100.0
    );

    println!("before (direct only):");
    for (k, secs, c) in &before.per_project {
        println!("  {:<24} {:>9}  ({})", name(k), hm(*secs), c.as_str());
    }
    println!(
        "  {:<24} {:>9}",
        "Unattributed",
        hm(before.unattributed_seconds)
    );

    println!("\nafter (+ carry-forward):");
    for (k, secs, c) in &after.per_project {
        println!("  {:<24} {:>9}  ({})", name(k), hm(*secs), c.as_str());
    }
    println!(
        "  {:<24} {:>9}",
        "Unattributed",
        hm(after.unattributed_seconds)
    );

    match &after.day_project {
        Some(p) => {
            let carried: f64 = after.carried.iter().map(|c| c.seconds).sum();
            println!(
                "\ncarried {} session(s) → {} (low), {} total:",
                after.carried.len(),
                name(p),
                hm(carried)
            );
            for c in &after.carried {
                let st = c.start.with_timezone(&chrono::Local).format("%H:%M");
                let en = c.end.with_timezone(&chrono::Local).format("%H:%M");
                println!("  {st}–{en}  {}", hm(c.seconds));
            }
        }
        None => println!(
            "\nno carry-forward: day is multi-project or has no dominant single-project signal."
        ),
    }
    Ok(())
}

/// Read-only preview of semantic attribution for one local day. Computes the
/// current graph-only attribution ("before") and the graph+semantic hybrid
/// ("after") over the same session windows and prints the per-project delta
/// plus the semantic matches that moved time. Writes NOTHING.
fn semantic_dry_run(
    mem: &storage::Storage,
    act: &activity::ActivityStore,
    day: chrono::NaiveDate,
    acfg: &attribution::AttribCfg,
) -> Result<()> {
    use attribution::{ProjectSignal, attribute_session};
    use std::collections::{HashMap, HashSet};

    const MIN_PROJECT_MEMS: i64 = 2;
    let pad = chrono::Duration::minutes(10);
    let scfg = semantic_attribution::SemanticCfg::default();

    let pool = mem.project_reference_pool(MIN_PROJECT_MEMS)?;
    let significant: HashSet<String> = pool.iter().map(|p| p.project_key.clone()).collect();
    let mut name_of: HashMap<String, String> = pool
        .iter()
        .map(|p| (p.project_key.clone(), p.project_name.clone()))
        .collect();

    let sessions = act.sessions_on_local_date(day)?;
    let mut before: HashMap<String, f64> = HashMap::new();
    let mut after: HashMap<String, f64> = HashMap::new();
    let (mut unattr_before, mut unattr_after) = (0.0f64, 0.0f64);
    let mut reasons: Vec<String> = Vec::new();
    // Global dedup across sessions: the ±pad makes adjacent session windows
    // overlap, so the same memory can appear twice — show each reason once.
    let mut seen: HashSet<String> = HashSet::new();

    for (s, e) in &sessions {
        let (s, e) = (*s, *e);
        let secs = (e - s).num_milliseconds() as f64 / 1000.0;
        let (lo, hi) = (s - pad, e + pad);

        let wmems = mem
            .window_memories_with_embeddings(lo, hi)
            .unwrap_or_default();
        // Two signals over the SAME memories, canonicalized:
        //   graph  = hard-links only (the "before")
        //   hybrid = hard-links + k-NN semantic match (the "after")
        let mut graph_votes: HashMap<String, f64> = HashMap::new();
        let mut hybrid_votes: HashMap<String, f64> = HashMap::new();
        for wm in &wmems {
            let linked_canon: Vec<String> = wm
                .linked_projects
                .iter()
                .map(|n| semantic_attribution::canonical_project(n))
                .collect();
            let linked = linked_canon.iter().find(|k| significant.contains(*k));
            if let Some(k) = linked {
                *graph_votes.entry(k.clone()).or_insert(0.0) += 1.0;
                *hybrid_votes.entry(k.clone()).or_insert(0.0) += 1.0;
                continue;
            }
            // Unlinked → try semantic k-NN (hybrid only).
            let cls = semantic_attribution::knn_classify(&wm.embedding, &pool, &scfg);
            let when = wm.timestamp.with_timezone(&chrono::Local).format("%H:%M");
            let head = journal_headline(&wm.title, &wm.content);
            if let Some(pk) = cls.project_key {
                *hybrid_votes.entry(pk.clone()).or_insert(0.0) += 1.0;
                name_of
                    .entry(pk)
                    .or_insert_with(|| cls.project_name.clone().unwrap_or_default());
                if seen.insert(wm.id.clone()) && reasons.len() < 24 {
                    reasons.push(format!("  ✓ {when}  {} | {}", cls.reason, head));
                }
            } else if cls.score > 0.30 && seen.insert(wm.id.clone()) && reasons.len() < 24 {
                reasons.push(format!("  · {when}  {} | {}", cls.reason, head));
            }
        }
        let to_sig = |m: HashMap<String, f64>| -> Vec<ProjectSignal> {
            m.into_iter()
                .map(|(project_key, weight)| ProjectSignal {
                    project_key,
                    weight,
                })
                .collect()
        };
        let br = attribute_session(secs, &to_sig(graph_votes), acfg);
        for a in br.allocations {
            *before.entry(a.project_key).or_insert(0.0) += a.seconds;
        }
        unattr_before += br.unattributed_seconds;
        let after_sig: Vec<ProjectSignal> = to_sig(hybrid_votes);
        let af = attribute_session(secs, &after_sig, acfg);
        for a in af.allocations {
            *after.entry(a.project_key).or_insert(0.0) += a.seconds;
        }
        unattr_after += af.unattributed_seconds;
    }

    let hm = |s: f64| -> String {
        let m = (s / 60.0).round() as i64;
        if m >= 60 {
            format!("{}h{:02}m", m / 60, m % 60)
        } else {
            format!("{m}m")
        }
    };

    let n_projects = significant.len();
    println!("Semantic attribution dry-run — {day}  (NO DB writes)");
    println!(
        "k-NN over {} pooled memories · {} projects · {} sessions\n",
        pool.len(),
        n_projects,
        sessions.len()
    );
    let mut keys: Vec<String> = before.keys().chain(after.keys()).cloned().collect();
    keys.sort();
    keys.dedup();
    keys.sort_by(|a, b| {
        after
            .get(b)
            .unwrap_or(&0.0)
            .partial_cmp(after.get(a).unwrap_or(&0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    println!("{:<22} {:>8}  {:>8}   Δ", "project", "before", "after");
    for k in &keys {
        let b = *before.get(k).unwrap_or(&0.0);
        let a = *after.get(k).unwrap_or(&0.0);
        let nm = name_of.get(k).cloned().unwrap_or_else(|| k.clone());
        let delta = a - b;
        let sign = if delta >= 0.0 { "+" } else { "-" };
        println!(
            "{:<22} {:>8}  {:>8}   {sign}{}",
            nm.chars().take(22).collect::<String>(),
            hm(b),
            hm(a),
            hm(delta.abs())
        );
    }
    println!(
        "{:<22} {:>8}  {:>8}",
        "Unattributed",
        hm(unattr_before),
        hm(unattr_after)
    );
    if !reasons.is_empty() {
        println!("\nSemantic matches (why time moved):");
        for r in &reasons {
            println!("{r}");
        }
    }
    println!("\n(dry-run only — nothing written. Review, then enable the write path.)");
    Ok(())
}

/// First readable line of a memory for the dry-run reason list — prefers the
/// title, falls back to the content's first line when the title is generic.
fn journal_headline(title: &str, content: &str) -> String {
    let t = title.trim();
    let generic = t.is_empty()
        || t.eq_ignore_ascii_case("conversation decision")
        || t.eq_ignore_ascii_case("user correction");
    let src = if generic { content } else { title };
    let line = src.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(64)
        .collect()
}

/// Eval entry point. Loads the JSONL seed, runs hybrid_search per query,
/// prints aggregate + per-query metrics.
///
/// Strictly read-only: opts.touch_access = false routes the inner retrievers
/// through `search_no_touch` / `find_similar_no_touch` and skips the final
/// `touch_access` call. Running eval twice in a row must return identical
/// numbers; production decay scoring is unaffected by measurement.
/// `mnemonic activity` — read-only views over activity.db. Opens the
/// store directly (no daemon round-trip) so it works whether or not the
/// daemon is running. The sampler and these readers both go through
/// SQLite WAL, so concurrent reads are safe.
fn run_activity(config: &Config, cmd: Option<ActivityCommands>) -> Result<()> {
    use crate::activity::{ActivityStore, fmt_hm};

    let path = config.activity_db_path();

    // Default (no subcommand): today's total + the week graph, the
    // single most shareable view.
    let cmd = cmd.unwrap_or(ActivityCommands::Week {
        days: 7,
        json: false,
    });

    // Projects come from the MEMORY graph, not activity.db — handle it
    // before touching ActivityStore so it works on installs that have no
    // activity data yet. Output an object (parsed by stripping to the
    // first `{`, robust against tracing log lines before it).
    if let ActivityCommands::Projects { .. } = cmd {
        let mem = storage::Storage::open(&config.storage.db_path)?;
        // Attributed hours come from activity.db (if it exists yet).
        let time = if path.exists() {
            ActivityStore::open(&path)
                .ok()
                .and_then(|s| s.project_time().ok())
        } else {
            None
        };
        println!("{}", mem.projects_payload(12, 5, time.as_ref())?);
        return Ok(());
    }

    // The friendly "no data yet" hint is only for the human-facing views.
    // JSON consumers (the widget) always get valid JSON, even empty.
    let is_human = matches!(
        &cmd,
        ActivityCommands::Today { json: false } | ActivityCommands::Week { json: false, .. }
    );
    if !path.exists() && is_human {
        println!(
            "No activity data yet. The sampler writes to {} once the \
             daemon has run with [activity] enabled.",
            path.display()
        );
        return Ok(());
    }
    let store = ActivityStore::open(&path)?;

    match cmd {
        ActivityCommands::Today { json } => {
            let secs = store.seconds_on_local_day(0)?;
            let sessions = store.session_count_today()?;
            if json {
                println!(
                    "{}",
                    serde_json::json!({
                        "seconds": secs.round() as i64,
                        "human": fmt_hm(secs),
                        "sessions": sessions,
                    })
                );
            } else {
                println!("Today: {}  ({sessions} session(s))", fmt_hm(secs));
            }
        }
        ActivityCommands::Week { days, json } => {
            let totals = store.daily_totals(days)?;
            if json {
                let arr: Vec<_> = totals
                    .iter()
                    .map(|t| {
                        serde_json::json!({
                            "date": t.date,
                            "seconds": t.seconds.round() as i64,
                            "human": fmt_hm(t.seconds),
                        })
                    })
                    .collect();
                println!("{}", serde_json::to_string_pretty(&arr)?);
            } else {
                print_week_chart(&totals);
            }
        }
        ActivityCommands::Summary => {
            println!("{}", store.summary_value()?);
        }
        ActivityCommands::Day { date } => {
            let day = match date {
                Some(s) => chrono::NaiveDate::parse_from_str(&s, "%Y-%m-%d")
                    .with_context(|| format!("bad --date {s:?}, expected YYYY-MM-DD"))?,
                None => chrono::Local::now().date_naive(),
            };
            println!("{}", store.day_value(day)?);
        }
        // Projects is handled early (above) — it doesn't use ActivityStore.
        ActivityCommands::Projects { .. } => unreachable!(),
    }
    Ok(())
}

/// Render a compact horizontal bar chart of daily worked time —
/// terminal-friendly and easy to screenshot for sharing.
fn print_week_chart(totals: &[crate::activity::DailyTotal]) {
    use crate::activity::fmt_hm;

    let max = totals
        .iter()
        .map(|t| t.seconds)
        .fold(0.0_f64, f64::max)
        .max(1.0);
    let bar_width = 24usize;
    let total: f64 = totals.iter().map(|t| t.seconds).sum();

    println!("Worked — last {} days", totals.len());
    println!();
    for t in totals {
        // Weekday label from the YYYY-MM-DD date.
        let label = chrono::NaiveDate::parse_from_str(&t.date, "%Y-%m-%d")
            .map(|d| d.format("%a %m-%d").to_string())
            .unwrap_or_else(|_| t.date.clone());
        let filled = ((t.seconds / max) * bar_width as f64).round() as usize;
        let bar = "█".repeat(filled) + &"·".repeat(bar_width.saturating_sub(filled));
        println!("  {label}  {bar}  {}", fmt_hm(t.seconds));
    }
    println!();
    let days = totals.len().max(1) as f64;
    println!(
        "  total {}   ·   avg {}/day",
        fmt_hm(total),
        fmt_hm(total / days)
    );
}

fn run_eval(
    config: &Config,
    file: &str,
    json: bool,
    no_graph_hop: bool,
    rerank: bool,
) -> Result<()> {
    use crate::eval::{
        EvalSummary, QueryResult, aggregate, expected_relevant_count, is_hit, load_jsonl,
        recall_at_k, reciprocal_rank,
    };
    use crate::retrieval::{HybridOptions, hybrid_search, hybrid_search_with_rerank};

    let path = std::path::Path::new(file);
    let mut queries = load_jsonl(path)?;

    // Auto-merge a sibling "<name>.local.jsonl" (gitignored) if present, so
    // private, host-specific eval rows — real memory ids and personal topics
    // that must not ship in the public seed — are still measured on local
    // runs while the committed seed stays sanitized.
    if let Some(stem) = file.strip_suffix(".jsonl") {
        let local = std::path::PathBuf::from(format!("{stem}.local.jsonl"));
        if local.exists() {
            let mut extra = load_jsonl(&local)?;
            if !extra.is_empty() {
                if !json {
                    println!(
                        "(+{} local eval queries from {})",
                        extra.len(),
                        local.display()
                    );
                }
                queries.append(&mut extra);
            }
        }
    }

    if queries.is_empty() {
        anyhow::bail!(
            "no queries found in {} — expected at least one JSONL row",
            path.display()
        );
    }

    let storage = storage::Storage::open(&config.storage.db_path)?;
    let embedder = embedding::create_embedder()?;

    // Only initialize the reranker when the flag is set — first use
    // downloads ~280MB and that's a wait the user shouldn't pay if they're
    // running default eval.
    let reranker = if rerank {
        match crate::reranker::try_create_reranker()? {
            Some(r) => Some(r),
            None => {
                anyhow::bail!(
                    "--rerank requested but no reranker is available. \
                     Build with `--features neural` (default for binary) and \
                     ensure ~/.fastembed_cache is writable."
                );
            }
        }
    } else {
        None
    };

    let opts = HybridOptions {
        // Need at least 20 hits so recall@20 isn't truncated.
        limit: 20,
        per_retriever: 20,
        with_graph_hop: !no_graph_hop,
        // Pure read — re-running eval must not perturb production rankings.
        touch_access: false,
        rerank,
        ..HybridOptions::default()
    };

    let mut per_query: Vec<QueryResult> = Vec::with_capacity(queries.len());
    let mut skipped = 0usize;

    for q in &queries {
        if !q.has_expectations() {
            skipped += 1;
            if !json {
                eprintln!(
                    "skipping query without expected_ids/expected_title_contains: {:?}",
                    q.query
                );
            }
            continue;
        }

        let hits = if let Some(rr) = reranker.as_deref() {
            hybrid_search_with_rerank(&storage, &*embedder, Some(rr), &q.query, &opts)?
        } else {
            hybrid_search(&storage, &*embedder, &q.query, &opts)?
        };
        let relevance: Vec<bool> = hits
            .iter()
            .map(|h| is_hit(q, &h.entry.id, &h.entry.title))
            .collect();
        let total_rel = expected_relevant_count(q);
        let r5 = recall_at_k(&relevance, 5, total_rel);
        let r20 = recall_at_k(&relevance, 20, total_rel);
        let mrr_q = reciprocal_rank(&relevance);
        let top_ids: Vec<(String, String)> = hits
            .iter()
            .take(5)
            .map(|h| (h.entry.id.clone(), h.entry.title.clone()))
            .collect();
        per_query.push(QueryResult {
            query: q.query.clone(),
            tags: q.tags.clone(),
            returned: hits.len(),
            recall_at_5: r5,
            recall_at_20: r20,
            reciprocal_rank: mrr_q,
            top_ids,
        });
    }

    let mut summary: EvalSummary = aggregate(per_query);
    summary.skipped = skipped;

    if json {
        println!("{}", serde_json::to_string_pretty(&summary)?);
        return Ok(());
    }

    // Human-readable table.
    println!(
        "Eval: {} queries scored ({} skipped, no expectations)",
        summary.queries, summary.skipped
    );
    println!("  recall@5  = {:.3}", summary.mean_recall_at_5);
    println!("  recall@20 = {:.3}", summary.mean_recall_at_20);
    println!("  MRR       = {:.3}", summary.mrr);
    if no_graph_hop {
        println!("  (graph-hop disabled — BM25 + vector only)");
    }
    if rerank {
        println!(
            "  (cross-encoder rerank: jina-v2-base-multilingual, top-30 → top-{})",
            opts.limit
        );
    }
    println!();

    for r in &summary.per_query {
        let mark5 = if r.recall_at_5 >= 0.5 { "✓" } else { "·" };
        println!(
            "{} r@5={:.2} r@20={:.2} rr={:.2}  {} {:?}",
            mark5, r.recall_at_5, r.recall_at_20, r.reciprocal_rank, r.query, r.tags
        );
        if r.recall_at_5 < 0.2 && !r.top_ids.is_empty() {
            // Show what the retriever *did* return so the user can see why.
            for (id, title) in &r.top_ids {
                println!("      {}  {}", &id[..8.min(id.len())], title);
            }
        }
    }
    Ok(())
}

/// Reflection / consolidation entry. Always prints the plan; in apply
/// mode also writes canonical memories and marks sources superseded.
fn reflect(
    config: &Config,
    apply: bool,
    threshold: f32,
    limit: Option<usize>,
    since_days: Option<i64>,
    json: bool,
) -> Result<()> {
    use crate::reflection::{Mode, ReflectionOptions, run_reflection};

    if Daemon::is_running(config).is_some() && apply {
        eprintln!(
            "WARNING: daemon is running. Apply mode writes canonical memories \
             and supersedes sources; recommend `mnemonic stop` first to avoid \
             concurrent dedup/save contention."
        );
    }

    let storage = std::sync::Arc::new(storage::Storage::open(&config.storage.db_path)?);
    let opts = ReflectionOptions {
        mode: if apply { Mode::Apply } else { Mode::DryRun },
        threshold,
        limit,
        since_days,
    };

    let plan = run_reflection(&storage, config, &opts)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&plan)?);
        return Ok(());
    }

    println!(
        "Reflection run {} ({}): pool={}, threshold={:.2}, clusters={}",
        plan.run_id,
        if apply { "APPLIED" } else { "dry-run" },
        plan.pool_size,
        plan.threshold,
        plan.clusters.len()
    );
    for (i, c) in plan.clusters.iter().enumerate() {
        let short_ids: Vec<String> = c
            .source_ids
            .iter()
            .map(|id| id.get(..8).unwrap_or(id).to_string())
            .collect();
        let avg_cos: f32 = if c.cosines.is_empty() {
            0.0
        } else {
            c.cosines.iter().sum::<f32>() / c.cosines.len() as f32
        };
        println!(
            "  [{:>2}] {} ({} members, avg cos={:.3}){}",
            i + 1,
            c.draft_title,
            c.source_ids.len(),
            avg_cos,
            if c.applied {
                format!(
                    " → canonical {}",
                    c.canonical_id
                        .as_deref()
                        .unwrap_or("?")
                        .get(..8)
                        .unwrap_or("?")
                )
            } else {
                String::new()
            }
        );
        println!("       sources: {}", short_ids.join(", "));
    }
    if !apply {
        println!("\nDry-run: no writes. Re-run with --apply to consolidate.");
    }
    Ok(())
}

/// Dispatch for `mnemonic peer ...`. Thin CLI wrapper — semantics live
/// in `Storage::upsert_peer` / `list_peers` / `sessions_for_peer`.
fn run_peer_command(config: &Config, sub: PeerCommands) -> Result<()> {
    let storage = storage::Storage::open(&config.storage.db_path)?;
    match sub {
        PeerCommands::List { limit } => {
            let peers = storage.list_peers(limit)?;
            if peers.is_empty() {
                println!("No peers recorded yet. Use `mnemonic peer add <name>` to add one.");
                return Ok(());
            }
            println!(
                "{} peer{}",
                peers.len(),
                if peers.len() == 1 { "" } else { "s" }
            );
            for p in &peers {
                println!("  {} ({})  last_seen={}", p.label(), p.kind, p.last_seen_at);
            }
        }
        PeerCommands::Add {
            name,
            display,
            kind,
        } => {
            let id = storage.upsert_peer(&name, display.as_deref(), &kind)?;
            println!(
                "Peer {} ({})  id={}",
                name.to_lowercase(),
                kind,
                &id[..8.min(id.len())]
            );
        }
        PeerCommands::Sessions { name, limit } => {
            let peer = match storage.peer_by_name(&name)? {
                Some(p) => p,
                None => {
                    println!("No peer named `{}`", name.to_lowercase());
                    return Ok(());
                }
            };
            let sessions = storage.sessions_for_peer(&peer.id, limit)?;
            if sessions.is_empty() {
                println!("No sessions for `{}`", peer.label());
                return Ok(());
            }
            println!(
                "{} session{} for `{}`",
                sessions.len(),
                if sessions.len() == 1 { "" } else { "s" },
                peer.label()
            );
            for s in &sessions {
                let marker = if s.is_open() { "●" } else { "○" };
                let ends = s.ended_at.as_deref().unwrap_or("ongoing");
                let label = s.label.as_deref().unwrap_or("(no label)");
                println!(
                    "  {marker} {label}  ({} → {})  src={}",
                    s.started_at, ends, s.source
                );
            }
        }
        PeerCommands::Merge { src, dst } => {
            let moved = storage.merge_peers(&src, &dst)?;
            println!(
                "Merged peer `{}` → `{}` ({} memory_peers row{} re-pointed)",
                src.to_lowercase(),
                dst.to_lowercase(),
                moved,
                if moved == 1 { "" } else { "s" }
            );
        }
    }
    Ok(())
}

/// Dispatch for `mnemonic fact ...`. Thin wrapper — the real semantics
/// live in `Storage::add_fact` / `current_facts_for_subject` /
/// `facts_for_subject`.
fn run_fact_command(config: &Config, sub: FactCommands) -> Result<()> {
    let storage = storage::Storage::open(&config.storage.db_path)?;
    match sub {
        FactCommands::Current { subject, history } => {
            let rows = if history {
                storage.facts_for_subject(&subject)?
            } else {
                storage.current_facts_for_subject(&subject)?
            };
            if rows.is_empty() {
                println!(
                    "No {} facts for `{}`",
                    if history { "" } else { "current " },
                    subject.to_lowercase()
                );
                return Ok(());
            }
            println!(
                "{} fact{} for `{}`{}",
                rows.len(),
                if rows.len() == 1 { "" } else { "s" },
                subject.to_lowercase(),
                if history {
                    " (full history, newest first)"
                } else {
                    " (current)"
                }
            );
            for f in &rows {
                let marker = if f.is_current() { "●" } else { "○" };
                let ends = f.valid_to.as_deref().unwrap_or("present");
                let conf = if (f.confidence - 1.0).abs() > f32::EPSILON {
                    format!(" [conf {:.2}]", f.confidence)
                } else {
                    String::new()
                };
                println!(
                    "  {marker} {} = {}   ({} → {}){conf}   src={}",
                    f.predicate,
                    f.value,
                    f.valid_from,
                    ends,
                    &f.source_memory_id[..8.min(f.source_memory_id.len())],
                );
            }
        }
        FactCommands::Add {
            subject,
            predicate,
            value,
            source,
            confidence,
        } => {
            if !(0.0..=1.0).contains(&confidence) {
                anyhow::bail!("confidence must be in [0.0, 1.0], got {confidence}");
            }
            // Show what's being superseded (if anything) so the operator
            // sees the transition explicitly when running this by hand.
            if let Some(prev) = storage.latest_fact(&subject, &predicate)? {
                println!("Superseding previous: {} = {}", prev.predicate, prev.value);
            }
            let id = storage.add_fact(&subject, &predicate, &value, &source, confidence, None)?;
            println!(
                "Added fact id={} ({} {} = {})",
                &id[..8.min(id.len())],
                subject.to_lowercase(),
                predicate.to_lowercase(),
                value
            );
        }
    }
    Ok(())
}

/// Dispatch for `mnemonic conclusion ...`. Real semantics live in
/// `Storage::add_conclusion` / `current_conclusions_for_subject` /
/// `conclusions_for_subject` / `conclusion_sources`. Mirrors the fact
/// CLI shape so the two tables feel like one cohesive feature.
fn run_conclusion_command(config: &Config, sub: ConclusionCommands) -> Result<()> {
    let storage = storage::Storage::open(&config.storage.db_path)?;
    match sub {
        ConclusionCommands::List {
            subject,
            history,
            with_sources,
        } => {
            let rows = if history {
                storage.conclusions_for_subject(&subject)?
            } else {
                storage.current_conclusions_for_subject(&subject)?
            };
            if rows.is_empty() {
                println!(
                    "No {} conclusions for `{}`",
                    if history { "" } else { "current " },
                    subject.to_lowercase()
                );
                return Ok(());
            }
            println!(
                "{} conclusion{} for `{}`{}",
                rows.len(),
                if rows.len() == 1 { "" } else { "s" },
                subject.to_lowercase(),
                if history {
                    " (full history, newest first)"
                } else {
                    " (current)"
                }
            );
            for c in &rows {
                let marker = if c.is_current() { "●" } else { "○" };
                println!(
                    "  {marker} [{}] {}   (conf {:.2}, support {})   id={}",
                    c.kind,
                    c.statement,
                    c.confidence,
                    c.support_count,
                    &c.id[..8.min(c.id.len())],
                );
                if with_sources {
                    let sources = storage.conclusion_sources(&c.id)?;
                    for sid in &sources {
                        println!("      ← {}", &sid[..8.min(sid.len())]);
                    }
                }
            }
        }
        ConclusionCommands::Add {
            subject,
            statement,
            kind,
            confidence,
            sources,
        } => {
            // CLI gate is a fast-path; the real invariant is enforced in
            // Storage::add_conclusion (so the future LLM generator and
            // other callers hit the same check). Duplicate validation
            // here mostly gives a nicer error before opening the DB.
            if !(0.0..=1.0).contains(&confidence) {
                anyhow::bail!("confidence must be in [0.0, 1.0], got {confidence}");
            }
            let id = storage.add_conclusion(&subject, &kind, &statement, confidence, &sources)?;
            // Print the honest count: Storage dedups `sources` before
            // inserting, so `--source a --source a` saves 1 link, not 2.
            // Earlier we printed `sources.len()` and lied — Codex caught it.
            let actual = storage.conclusion_sources(&id)?.len();
            println!(
                "Added conclusion id={} ({} [{}] supported by {} source{})",
                &id[..8.min(id.len())],
                subject.to_lowercase(),
                kind,
                actual,
                if actual == 1 { "" } else { "s" },
            );
        }
        ConclusionCommands::Generate {
            subject,
            limit,
            apply,
        } => {
            // Refuse when LLM is off in config — generating without
            // a backend would just dump a confusing error from the
            // network call. Surface the config gate explicitly.
            if !config.llm.enabled {
                anyhow::bail!(
                    "LLM is disabled in config. Set `[llm] enabled = true` (and ensure \
                     ollama is running at the configured endpoint) to use \
                     `mnemonic conclusion generate`."
                );
            }
            let backend = crate::graph::extractor_llm::OllamaBackend::new(&config.llm)
                .context("failed to initialize Ollama backend")?;
            let generator =
                crate::conclusions_generator::LlmConclusionGenerator::new(Box::new(backend));
            let out = generator.generate_for_subject(&storage, &subject, limit)?;

            println!(
                "Generated {} conclusion{} for `{}` from {} source memor{}:",
                out.conclusions.len(),
                if out.conclusions.len() == 1 { "" } else { "s" },
                subject.to_lowercase(),
                out.source_memory_ids.len(),
                if out.source_memory_ids.len() == 1 {
                    "y"
                } else {
                    "ies"
                }
            );
            for c in &out.conclusions {
                println!(
                    "  · [{}] {}   (conf {:.2})",
                    c.kind, c.statement, c.confidence
                );
            }
            if !apply {
                println!("\n(dry-run — pass --apply to persist)");
                return Ok(());
            }

            let mut saved = 0usize;
            let mut skipped_invalid = 0usize;
            for c in &out.conclusions {
                match storage.add_conclusion(
                    &subject,
                    &c.kind,
                    &c.statement,
                    c.confidence,
                    &out.source_memory_ids,
                ) {
                    Ok(id) => {
                        println!(
                            "  ✓ saved {} → {}",
                            &id[..8.min(id.len())],
                            c.statement.chars().take(60).collect::<String>()
                        );
                        saved += 1;
                    }
                    Err(e) => {
                        // Storage rejects out-of-range confidence,
                        // NaN, empty statement, etc. Log and
                        // continue — don't bail on a single bad LLM
                        // output.
                        eprintln!("  ✗ rejected: {e}");
                        skipped_invalid += 1;
                    }
                }
            }
            println!(
                "\nDone: {saved} conclusion{} saved{}",
                if saved == 1 { "" } else { "s" },
                if skipped_invalid > 0 {
                    format!(", {skipped_invalid} skipped (storage validation)")
                } else {
                    String::new()
                }
            );
        }
        ConclusionCommands::Supersede { old_id, new_id } => {
            // Resolve both ids — supersede on prefixes is allowed
            // because both rows must already exist in the table, so
            // the ambiguity check catches typos before any state
            // change.
            let old_full = resolve_conclusion_id_prefix(&storage, &old_id)?;
            let new_full = resolve_conclusion_id_prefix(&storage, &new_id)?;
            // Preview what's being replaced so the user sees the
            // operation in concrete terms.
            let old = storage.conclusion_by_id(&old_full)?.ok_or_else(|| {
                anyhow::anyhow!("conclusion {old_full} not found (post-prefix resolve)")
            })?;
            let new = storage.conclusion_by_id(&new_full)?.ok_or_else(|| {
                anyhow::anyhow!("conclusion {new_full} not found (post-prefix resolve)")
            })?;
            println!(
                "Superseding:\n  ○ {} [{}] {}\n  ● {} [{}] {}",
                &old.id[..8.min(old.id.len())],
                old.kind,
                old.statement,
                &new.id[..8.min(new.id.len())],
                new.kind,
                new.statement
            );
            storage.supersede_conclusion(&old_full, &new_full)?;
            println!("✓ supersede applied");
        }
        ConclusionCommands::Delete { id } => {
            let full_id = resolve_conclusion_id_prefix(&storage, &id)?;
            // Preview the row we're about to drop — useful when the
            // CLI is run by hand and the user is verifying the
            // right id. Cheap query against a small table.
            match storage.conclusion_by_id(&full_id)? {
                Some(c) => {
                    println!(
                        "Deleting: {} [{}] {}",
                        &c.id[..8.min(c.id.len())],
                        c.kind,
                        c.statement
                    );
                }
                None => {
                    println!("No conclusion with id `{full_id}`");
                    return Ok(());
                }
            }
            let removed = storage.delete_conclusion(&full_id)?;
            if removed {
                println!("✓ deleted (source memories untouched)");
            } else {
                // Race: row existed during conclusion_by_id but was
                // gone by the time DELETE ran. Surface honestly.
                println!("(already gone — race or concurrent delete)");
            }
        }
    }
    Ok(())
}

/// Resolve a conclusion id prefix to a full id. Mirrors
/// `resolve_session_id_prefix`: full UUIDs pass through, prefixes
/// must be ≥ 8 chars and resolve to exactly one row. Ambiguous
/// matches error loudly so destructive operations stay explicit.
fn resolve_conclusion_id_prefix(storage: &storage::Storage, prefix: &str) -> Result<String> {
    if prefix.len() >= 36 {
        return Ok(prefix.to_string());
    }
    if prefix.len() < MIN_SESSION_PREFIX_LEN {
        anyhow::bail!(
            "Conclusion id prefix must be at least {MIN_SESSION_PREFIX_LEN} chars (got {})",
            prefix.len()
        );
    }
    let matches = storage.find_conclusion_ids_by_prefix(prefix)?;
    match matches.len() {
        0 => anyhow::bail!("No conclusion id starts with `{prefix}`"),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => anyhow::bail!("Prefix `{prefix}` matches {n} conclusions — please use a longer id"),
    }
}

/// Dispatch for `mnemonic session ...`. v1 is read-only; sessions are
/// opened/closed by the daemon (today: only via explicit code paths;
/// the watcher auto-tag wiring lands in the follow-up commit).
fn run_session_command(config: &Config, sub: SessionCommands) -> Result<()> {
    let storage = storage::Storage::open(&config.storage.db_path)?;
    match sub {
        SessionCommands::List { open, peer, limit } => {
            // Three filter combinations: peer-scoped (uses
            // `sessions_for_peer`), open-only or unrestricted (both
            // use `open_sessions` / a fresh query). Peer-scoped runs
            // Filter pushed into SQL so LIMIT N applies AFTER the
            // open/closed check. Codex caught that the previous
            // `sessions_for_peer(N).filter(is_open)` could return 0 when
            // the most recent N peer sessions were all closed, even
            // though older open ones existed.
            let sessions = if let Some(name) = &peer {
                let p = match storage.peer_by_name(name)? {
                    Some(p) => p,
                    None => {
                        println!("No peer named `{}`", name.to_lowercase());
                        return Ok(());
                    }
                };
                if open {
                    storage.open_sessions_for_peer(&p.id, limit)?
                } else {
                    // --peer without --open: show everything (open +
                    // closed) for that one peer. Single-peer scope is
                    // bounded enough that history is OK to surface.
                    storage.sessions_for_peer(&p.id, limit)?
                }
            } else {
                // No --peer scope: default to open-only across all peers
                // (with or without --open flag). Unfiltered history
                // across every peer would dump too much; require an
                // explicit --peer to dive into closed history.
                if !open {
                    println!(
                        "(showing open sessions only — pass --peer <name> to see closed history)"
                    );
                }
                storage.open_sessions(limit)?
            };

            if sessions.is_empty() {
                let scope = if let Some(name) = &peer {
                    format!("`{}`", name.to_lowercase())
                } else if open {
                    "open".into()
                } else {
                    "any".into()
                };
                println!("No sessions found ({scope})");
                return Ok(());
            }

            println!(
                "{} session{} (newest first)",
                sessions.len(),
                if sessions.len() == 1 { "" } else { "s" }
            );
            for s in &sessions {
                let marker = if s.is_open() { "●" } else { "○" };
                let ends = s.ended_at.as_deref().unwrap_or("ongoing");
                let label = s.label.as_deref().unwrap_or("(no label)");
                // Peer label resolved best-effort; if the peer row was
                // deleted (FK ON DELETE CASCADE removed the session too,
                // but just in case) fall back to the raw id.
                let peer_label = storage
                    .peer_by_id(&s.peer_id)
                    .ok()
                    .flatten()
                    .map(|p| p.label().to_string())
                    .unwrap_or_else(|| s.peer_id.clone());
                println!(
                    "  {marker} {} | {peer_label} | {label}  ({} → {})  src={}",
                    &s.id[..8.min(s.id.len())],
                    s.started_at,
                    ends,
                    s.source
                );
            }
        }
        SessionCommands::Show { id } => {
            // Resolve a prefix to a full id. UUIDs are 36 chars; if the
            // caller passed less, look it up by LIKE 'prefix%'. Reject
            // ambiguous prefixes loudly instead of grabbing the first
            // match (silent wrong-id is worse than "be more specific").
            let full_id = resolve_session_id_prefix(&storage, &id)?;
            let session = match storage.session_by_id(&full_id)? {
                Some(s) => s,
                None => {
                    println!("No session with id `{full_id}`");
                    return Ok(());
                }
            };

            let peer_label = storage
                .peer_by_id(&session.peer_id)
                .ok()
                .flatten()
                .map(|p| p.label().to_string())
                .unwrap_or_else(|| session.peer_id.clone());

            let marker = if session.is_open() { "●" } else { "○" };
            let ends = session.ended_at.as_deref().unwrap_or("ongoing");
            println!(
                "{marker} session {} | peer {peer_label} | {}\n  started: {}\n  ended:   {}\n  source:  {}",
                &session.id[..8.min(session.id.len())],
                session.label.as_deref().unwrap_or("(no label)"),
                session.started_at,
                ends,
                session.source,
            );

            let memories = storage.memories_for_session(&session.id)?;
            if memories.is_empty() {
                println!("\n(no memories linked to this session yet)");
                return Ok(());
            }
            println!(
                "\n{} memor{} in window (oldest first):",
                memories.len(),
                if memories.len() == 1 { "y" } else { "ies" }
            );
            for m in &memories {
                println!(
                    "  · {}  [{:?}]  {}",
                    m.timestamp.format("%Y-%m-%d %H:%M"),
                    m.memory_type,
                    m.title
                );
            }
        }
    }
    Ok(())
}

/// Dispatch for `mnemonic dream ...`. v1 uses the heuristic
/// summarizer; v2 will add LLM prose generation behind the same CLI.
fn run_dream_command(config: &Config, sub: DreamCommands) -> Result<()> {
    let storage = storage::Storage::open(&config.storage.db_path)?;
    match sub {
        DreamCommands::Run {
            id,
            allow_open,
            llm,
            regenerate,
        } => {
            let full_id = resolve_session_id_prefix(&storage, &id)?;
            // Idempotency vs. regeneration: by default an existing
            // canonical summary causes skip. With `--regenerate`, we
            // explicitly forget the old one and produce a fresh one
            // — the canonical "upgrade heuristic to LLM" path.
            if let Some(existing) = dream::summary_for_session(&storage, &full_id)? {
                if regenerate {
                    storage.forget_by_id(&existing.id)?;
                    println!(
                        "Forgot prior summary {} for session {} — regenerating.",
                        &existing.id[..8.min(existing.id.len())],
                        &full_id[..8.min(full_id.len())]
                    );
                } else {
                    println!(
                        "Session {} already has a summary (memory {}). Skipping. \
                         (Pass --regenerate to replace it.)",
                        &full_id[..8.min(full_id.len())],
                        &existing.id[..8.min(existing.id.len())]
                    );
                    return Ok(());
                }
            }
            // Pick the summarizer. v1 heuristic by default; --llm
            // routes through Ollama. The four-way matrix
            // (heuristic/llm × strict/allow-open) is dispatched here
            // rather than pushed into the dream module so the LLM
            // backend stays out of the storage layer's tests.
            let summary = if llm {
                if !config.llm.enabled {
                    anyhow::bail!(
                        "--llm requires `[llm] enabled = true` in config (and ollama running)."
                    );
                }
                let backend = crate::graph::extractor_llm::OllamaBackend::new(&config.llm)
                    .context("failed to initialize Ollama backend for --llm")?;
                if allow_open {
                    dream::summarize_session_llm_allowing_open(&storage, &full_id, &backend)?
                } else {
                    dream::summarize_session_llm(&storage, &full_id, &backend)?
                }
            } else if allow_open {
                dream::summarize_session_heuristic_allowing_open(&storage, &full_id)?
            } else {
                dream::summarize_session_heuristic(&storage, &full_id)?
            };
            // Need an embedding so the summary participates in
            // semantic retrieval the same way regular memories do.
            let embedder = crate::embedding::create_embedder()?;
            let emb = embedder
                .embed(&format!("{} {}", summary.title, summary.content))
                .ok();
            storage.save_with_embedding(&summary, emb.as_ref())?;
            println!(
                "Saved session summary {} ({} bytes content)",
                &summary.id[..8.min(summary.id.len())],
                summary.content.len()
            );
            println!(
                "\n--- preview ---\n{}\n{}\n",
                summary.title, summary.content
            );
        }
        DreamCommands::Batch {
            since_hours,
            limit,
            apply,
            llm,
            regenerate,
        } => {
            let sessions = storage.closed_sessions_since(since_hours, limit)?;
            if sessions.is_empty() {
                println!("No closed sessions in the last {since_hours}h.");
                return Ok(());
            }
            // Selection: --regenerate processes ALL closed sessions
            // (existing summaries will be replaced); default skips
            // already-summarized rows for idempotency. The regenerate
            // path tracks prior-summary ids per session so we can
            // forget them at apply-time.
            let mut to_summarize: Vec<(crate::storage::Session, Option<String>)> = Vec::new();
            let mut skipped = 0usize;
            for s in &sessions {
                match dream::summary_for_session(&storage, &s.id)? {
                    Some(existing) => {
                        if regenerate {
                            to_summarize.push((s.clone(), Some(existing.id)));
                        } else {
                            skipped += 1;
                        }
                    }
                    None => to_summarize.push((s.clone(), None)),
                }
            }
            let regenerate_count = to_summarize.iter().filter(|(_, p)| p.is_some()).count();
            println!(
                "Found {} closed session{} in the last {since_hours}h: \
                 {} already summarized (skipped: {}, regenerate: {}), {} pending fresh",
                sessions.len(),
                if sessions.len() == 1 { "" } else { "s" },
                skipped + regenerate_count,
                skipped,
                regenerate_count,
                to_summarize.len() - regenerate_count,
            );
            if to_summarize.is_empty() {
                return Ok(());
            }
            if !apply {
                println!("\nDry run (pass --apply to save). Pending sessions:");
                for (s, prior) in &to_summarize {
                    let regen_tag = match prior {
                        Some(_) => " [regenerate]",
                        None => "",
                    };
                    println!(
                        "  · {} | started {} | source {}{regen_tag}",
                        &s.id[..8.min(s.id.len())],
                        s.started_at,
                        s.source
                    );
                }
                return Ok(());
            }

            // Build the LLM backend once if --llm — N sessions
            // share the same connection. Heuristic path needs no
            // backend; we keep the Option out of band so the inner
            // loop branches cleanly.
            let llm_backend: Option<crate::graph::extractor_llm::OllamaBackend> = if llm {
                if !config.llm.enabled {
                    anyhow::bail!(
                        "--llm requires `[llm] enabled = true` in config (and ollama running)."
                    );
                }
                Some(
                    crate::graph::extractor_llm::OllamaBackend::new(&config.llm)
                        .context("failed to initialize Ollama backend for --llm")?,
                )
            } else {
                None
            };

            let embedder = crate::embedding::create_embedder()?;
            let mut saved = 0usize;
            let mut regenerated = 0usize;
            let mut empty = 0usize;
            for (s, prior) in &to_summarize {
                let summary_result = match &llm_backend {
                    Some(b) => dream::summarize_session_llm(&storage, &s.id, b),
                    None => dream::summarize_session_heuristic(&storage, &s.id),
                };
                match summary_result {
                    Ok(summary) => {
                        // Forget the prior summary AFTER we've
                        // successfully generated the new one — this
                        // way a failed generation doesn't leave the
                        // session in a half-state. The order is:
                        // generate → forget old → save new.
                        if let Some(old_id) = prior
                            && let Err(e) = storage.forget_by_id(old_id)
                        {
                            eprintln!(
                                "  ✗ regenerate: failed to forget prior {} for session {}: {e}",
                                &old_id[..8.min(old_id.len())],
                                &s.id[..8.min(s.id.len())]
                            );
                            continue;
                        }
                        let emb = embedder
                            .embed(&format!("{} {}", summary.title, summary.content))
                            .ok();
                        if let Err(e) = storage.save_with_embedding(&summary, emb.as_ref()) {
                            eprintln!("  ✗ save failed for session {}: {e}", &s.id[..8]);
                            continue;
                        }
                        let marker = if prior.is_some() { "↺" } else { "✓" };
                        println!(
                            "  {marker} {} → summary {}",
                            &s.id[..8.min(s.id.len())],
                            &summary.id[..8.min(summary.id.len())]
                        );
                        if prior.is_some() {
                            regenerated += 1;
                        } else {
                            saved += 1;
                        }
                    }
                    Err(e) => {
                        // Most common path: empty sessions (no
                        // memories linked). Count and continue.
                        let msg = e.to_string();
                        if msg.contains("no memories") {
                            empty += 1;
                        } else {
                            eprintln!("  ✗ session {}: {e}", &s.id[..8]);
                        }
                    }
                }
            }
            println!(
                "\nDone: {saved} new, {regenerated} regenerated, {empty} skipped (empty sessions)"
            );
        }
    }
    Ok(())
}

/// Minimum prefix length for short-id lookup. UUIDs have ~5 bits of
/// entropy per hex digit, so 8 chars = ~32 bits = collision-resistant
/// across realistic session counts. Anything shorter is too likely to
/// silently grab the wrong session when a near-collision exists. The
/// CLI doc explicitly promises 8+; enforced here so the contract is
/// real, not aspirational. Codex caught the gap.
const MIN_SESSION_PREFIX_LEN: usize = 8;

/// Resolve a session id prefix to a full id. Returns the input verbatim
/// if it's already a full UUID (36 chars). For prefixes, requires
/// `MIN_SESSION_PREFIX_LEN` chars and queries the sessions table with
/// LIKE 'prefix%' — errors on no match OR on multiple matches (caller
/// must disambiguate).
fn resolve_session_id_prefix(storage: &storage::Storage, prefix: &str) -> Result<String> {
    if prefix.len() >= 36 {
        return Ok(prefix.to_string());
    }
    if prefix.len() < MIN_SESSION_PREFIX_LEN {
        anyhow::bail!(
            "Session id prefix must be at least {MIN_SESSION_PREFIX_LEN} chars (got {})",
            prefix.len()
        );
    }
    let matches = storage.find_session_ids_by_prefix(prefix)?;
    match matches.len() {
        0 => anyhow::bail!("No session id starts with `{prefix}`"),
        1 => Ok(matches.into_iter().next().unwrap()),
        n => anyhow::bail!("Prefix `{prefix}` matches {n} sessions — please use a longer id"),
    }
}

/// Canonicalize all existing entity names and merge variants.
/// Idempotent — safe to re-run.
fn dedupe_graph(config: &Config, dry_run: bool) -> Result<()> {
    use crate::graph::canonical::canonicalize_name;

    if Daemon::is_running(config).is_some() {
        eprintln!(
            "WARNING: daemon is running. Dedupe writes to entities/edges tables \
             concurrently; recommend `mnemonic stop` first."
        );
    }

    let storage = storage::Storage::open(&config.storage.db_path)?;
    let names = storage.list_entity_names()?;
    println!("Loaded {} entities", names.len());

    // Group by canonical name.
    let mut groups: std::collections::BTreeMap<String, Vec<String>> =
        std::collections::BTreeMap::new();
    for n in &names {
        let canon = canonicalize_name(n);
        if canon.is_empty() {
            continue;
        }
        groups.entry(canon).or_default().push(n.clone());
    }

    // Plan: for each group with >1 distinct names, pick canonical (already
    // exists in BD as-is) or rename one variant to canonical.
    let mut plan: Vec<(String, Vec<String>)> = Vec::new();
    for (canonical, variants) in &groups {
        if variants.len() == 1 && &variants[0] == canonical {
            continue; // already canonical, no-op
        }
        plan.push((canonical.clone(), variants.clone()));
    }

    if plan.is_empty() {
        println!("Graph already canonical — nothing to merge.");
        return Ok(());
    }

    println!("{} entity groups need merging:", plan.len());
    for (canonical, variants) in &plan {
        println!("  {} ← {:?}", canonical, variants);
    }

    if dry_run {
        println!("\nDry-run: no changes written. Re-run without --dry-run to apply.");
        return Ok(());
    }

    let mut merged = 0usize;
    let mut renamed = 0usize;
    let mut total_edges = 0usize;
    let mut total_links = 0usize;

    for (canonical, variants) in plan {
        let canonical_exists = variants.iter().any(|v| v == &canonical);

        if !canonical_exists {
            // Promote first variant to canonical. After rename, others merge.
            let to_rename = variants.first().cloned().unwrap_or_default();
            if storage.rename_entity(&to_rename, &canonical)? {
                renamed += 1;
            }
        }

        for variant in &variants {
            if variant == &canonical {
                continue;
            }
            let report = storage.merge_entities(&canonical, variant)?;
            if report.alias_dropped {
                merged += 1;
                total_edges += report.edges_redirected;
                total_links += report.memory_links_redirected;
            }
        }
    }

    println!(
        "\nDone: {merged} aliases merged, {renamed} promoted, \
         {total_edges} edges redirected, {total_links} memory links redirected"
    );
    Ok(())
}

/// Quick sanity check that an Ollama-compatible endpoint is alive and
/// has the requested model pulled. Used as a pre-flight before reextract
/// so dead-backend silent fallbacks don't ruin a 30-minute job.
fn probe_ollama(endpoint: &str, model: &str) -> Result<()> {
    let base = endpoint.trim_end_matches('/');
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()?;
    let resp = client
        .get(format!("{base}/api/tags"))
        .send()
        .map_err(|e| anyhow::anyhow!("connect: {e}"))?;
    if !resp.status().is_success() {
        anyhow::bail!("HTTP {}", resp.status());
    }
    let body: serde_json::Value = resp.json()?;
    let has_model = body
        .get("models")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter().any(|m| {
                m.get("name")
                    .and_then(|n| n.as_str())
                    .map(|n| n == model || n.starts_with(&format!("{model}:")))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if !has_model {
        anyhow::bail!("model `{model}` not pulled — run `ollama pull {model}`");
    }
    Ok(())
}

/// Re-run the graph extractor over existing memories.
fn reextract(
    config: &Config,
    since_days: Option<i64>,
    limit: Option<usize>,
    dry_run: bool,
    include_superseded: bool,
    clean_graph: bool,
    force: bool,
) -> Result<()> {
    use crate::graph::extractor::{EntityExtractor, RuleExtractor};

    // Refuse to run while the daemon is up — concurrent writes inflate
    // mention_count and produce duplicate edges. dry_run is always safe.
    if !dry_run && !force && Daemon::is_running(config).is_some() {
        anyhow::bail!(
            "daemon is running. concurrent writes to entities/edges would \
             corrupt graph counts. Run `mnemonic stop` first, or pass \
             --force if you really want to."
        );
    }

    // --clean-graph wipes the ENTIRE graph, so it must rebuild every memory.
    // Combining it with --since-days/--limit would clear the whole graph but
    // only reextract the selected subset, leaving the rest with no links/edges
    // until a separate full reextract. Reject the combination up front.
    if clean_graph && (since_days.is_some() || limit.is_some()) {
        anyhow::bail!(
            "--clean-graph rebuilds the whole graph and can't be combined with \
             --since-days/--limit (other memories would be left unlinked). Run \
             --clean-graph on its own, or drop it to reextract just a subset."
        );
    }

    // Pre-flight: if LLM extractor is enabled, probe Ollama up-front. A
    // half-dead backend would silently downgrade to rule-only for the
    // affected memories and the user would only notice via grep'ing logs.
    if !dry_run && config.llm.enabled {
        eprintln!(
            "Pre-flight: probing LLM endpoint at {}...",
            config.llm.endpoint
        );
        match probe_ollama(&config.llm.endpoint, &config.llm.model) {
            Ok(()) => eprintln!("  → reachable, model `{}` available.", config.llm.model),
            Err(e) => anyhow::bail!(
                "Ollama backend not usable: {e}. Either run `ollama serve` + \
                 `ollama pull {}`, or set llm.enabled=false in config.",
                config.llm.model
            ),
        }
    }

    let storage = std::sync::Arc::new(storage::Storage::open(&config.storage.db_path)?);
    let ids = storage.list_memory_ids(since_days, limit, include_superseded)?;
    println!(
        "Planning reextract for {} memories{}{}",
        ids.len(),
        if include_superseded {
            " (incl. superseded)"
        } else {
            " (active only)"
        },
        if clean_graph {
            " · clean-graph mode"
        } else {
            ""
        }
    );

    if dry_run {
        for id in ids.iter().take(5) {
            if let Ok(Some(m)) = storage.get_by_id(id) {
                println!("  {} — {}", &id[..8], m.title);
            }
        }
        if ids.len() > 5 {
            println!("  ... and {} more", ids.len() - 5);
        }
        println!("\nDry-run: no extraction performed.");
        return Ok(());
    }

    // clean-graph: wipe the existing graph first so updated extractor rules
    // reclassify entity types and drop now-denied nodes. A plain reextract
    // upserts with INSERT OR IGNORE, so stale types and junk would survive.
    if clean_graph {
        let removed = storage.clear_graph()?;
        println!("clean-graph: wiped {removed} existing entities + their edges");
    }

    // Pick composite if LLM enabled; otherwise rule-based only.
    let extractor: Box<dyn EntityExtractor> = if config.llm.enabled {
        match crate::graph::extractor_llm::OllamaBackend::new(&config.llm) {
            Ok(backend) => {
                let llm = crate::graph::extractor_llm::LlmExtractor::new(
                    Box::new(backend),
                    storage.clone(),
                    &config.llm,
                );
                println!(
                    "Extractor: composite (rule + LLM via {} {})",
                    config.llm.endpoint, config.llm.model
                );
                Box::new(crate::graph::extractor_llm::CompositeExtractor::new(
                    Box::new(RuleExtractor::new()),
                    Box::new(llm),
                ))
            }
            Err(e) => {
                eprintln!("LLM backend failed: {e}. Continuing with rule-based only.");
                Box::new(RuleExtractor::new())
            }
        }
    } else {
        println!("Extractor: rule-based (set llm.enabled=true for richer graph)");
        Box::new(RuleExtractor::new())
    };

    let mut done = 0usize;
    let mut entities_added = 0usize;
    let mut edges_added = 0usize;
    let total = ids.len();

    for (idx, id) in ids.iter().enumerate() {
        let entry = match storage.get_by_id(id)? {
            Some(e) => e,
            None => continue,
        };
        let result = extractor.extract(&entry);
        let n_e = result.entities.len();
        let n_r = result.edges.len();
        if let Err(e) =
            storage.replace_graph_and_reconcile_projects(&entry, &result.entities, &result.edges)
        {
            eprintln!("replace_graph failed for {id}: {e}");
        } else {
            entities_added += n_e;
            edges_added += n_r;
        }
        done += 1;
        if (idx + 1) % 10 == 0 || idx + 1 == total {
            println!(
                "  [{}/{}] {:.0}%",
                idx + 1,
                total,
                (idx + 1) as f64 / total as f64 * 100.0
            );
        }
    }

    println!(
        "\nDone: {done} memories reprocessed, {entities_added} entity refs, {edges_added} edges \
         (deduplicated server-side by INSERT OR IGNORE / upsert)"
    );
    Ok(())
}

/// Drain the `pending_extractions` queue. Picks up memories whose
/// `next_attempt_at <= now`, re-runs the composite extractor (rule + LLM),
/// commits a `save_graph` on success and `drop_pending_extraction` to clear
/// the row. On failure (LLM still down, parse still bad), the LlmExtractor's
/// own error path re-enqueues via `enqueue_pending_extraction`; here we
/// only need to bump `mark_pending_attempted` so the backoff schedule
/// advances and we don't tight-loop on the same broken row.
fn reextract_pending(
    config: &Config,
    limit: Option<usize>,
    dry_run: bool,
    force: bool,
    discard: bool,
) -> Result<()> {
    use crate::graph::extractor::{EntityExtractor, RuleExtractor};

    if !dry_run && !force && Daemon::is_running(config).is_some() {
        anyhow::bail!(
            "daemon is running. Pending drain writes to entities/edges \
             concurrently; run `mnemonic stop` first, or pass --force if \
             you know what you're doing."
        );
    }

    let storage = std::sync::Arc::new(storage::Storage::open(&config.storage.db_path)?);
    let total_pending = storage.pending_extractions_count()?;
    let batch_cap = limit.unwrap_or(100);
    let due = storage.pending_due_for_retry(batch_cap)?;
    println!(
        "Pending queue: {total_pending} total, {} due now (batch cap {batch_cap})",
        due.len()
    );

    if due.is_empty() {
        if total_pending > 0 {
            println!("Nothing due yet — earliest entry is still in backoff window.");
        }
        return Ok(());
    }

    if dry_run {
        for id in due.iter().take(10) {
            if let Some((attempts, last_err, next_at)) = storage.pending_row(id)? {
                let title = storage
                    .get_by_id(id)?
                    .map(|m| m.title)
                    .unwrap_or_else(|| "<deleted>".into());
                println!(
                    "  {} — {} (attempts={attempts}, next={next_at}, err={})",
                    &id[..8.min(id.len())],
                    title,
                    last_err.as_deref().unwrap_or("?")
                );
            }
        }
        println!("\nDry-run: no retries performed.");
        return Ok(());
    }

    if config.llm.enabled {
        // Probe Ollama up-front. If it's still down, draining will just
        // bump every row's backoff with the same error — wasted writes.
        eprintln!(
            "Pre-flight: probing LLM endpoint at {}...",
            config.llm.endpoint
        );
        if let Err(e) = probe_ollama(&config.llm.endpoint, &config.llm.model) {
            anyhow::bail!(
                "Ollama backend still not usable: {e}. Pending drain aborted to \
                 avoid burning attempts on a known-down backend."
            );
        }
        eprintln!("  → reachable, draining.");
    }

    let extractor: Box<dyn EntityExtractor> = if config.llm.enabled {
        let backend = crate::graph::extractor_llm::OllamaBackend::new(&config.llm)?;
        let llm = crate::graph::extractor_llm::LlmExtractor::new(
            Box::new(backend),
            storage.clone(),
            &config.llm,
        );
        Box::new(crate::graph::extractor_llm::CompositeExtractor::new(
            Box::new(RuleExtractor::new()),
            Box::new(llm),
        ))
    } else if discard {
        // User explicitly asked to discard. Honor it loudly.
        println!(
            "LLM disabled + --discard-pending: dropping {} due pending rows.",
            due.len()
        );
        let mut dropped_n = 0usize;
        for id in &due {
            if storage.drop_pending_extraction(id).is_ok() {
                dropped_n += 1;
            }
        }
        println!("Dropped {dropped_n} rows.");
        return Ok(());
    } else {
        // Silent drop was the original bug the queue exists to prevent.
        // Refuse rather than throwing extractions away by surprise.
        anyhow::bail!(
            "{} memories are queued for LLM extraction but llm.enabled = false in config. \
             Either enable LLM (`llm.enabled = true` + `ollama serve` + the configured model), \
             or pass --discard-pending to delete the queue intentionally.",
            due.len()
        );
    };

    let mut succeeded = 0usize;
    let mut still_failing = 0usize;
    let mut dropped = 0usize;
    let total = due.len();

    for (idx, id) in due.iter().enumerate() {
        let entry = match storage.get_by_id(id)? {
            Some(e) => e,
            None => {
                // Memory was forgotten between enqueue and drain — drop the row.
                let _ = storage.drop_pending_extraction(id);
                continue;
            }
        };

        // Snapshot row pre-call. If the row is still present after extract,
        // the LLM either succeeded (we'll drop it below) or failed silently
        // and re-enqueued — bump attempts to advance backoff.
        let before = storage.pending_row(id)?;
        let result = extractor.extract(&entry);

        // The LlmExtractor itself calls drop_pending_extraction on success
        // and enqueue_pending_extraction on failure. So:
        //   - row gone     → success
        //   - row still in → failure (re-enqueued by extractor)
        let after = storage.pending_row(id)?;
        match (before.is_some(), after.is_some()) {
            (_, false) => {
                // Success path. Persist the graph so the work isn't wasted.
                if let Err(e) = storage.replace_graph_and_reconcile_projects(
                    &entry,
                    &result.entities,
                    &result.edges,
                ) {
                    eprintln!("replace_graph failed for {id}: {e}");
                } else {
                    succeeded += 1;
                }
            }
            (_, true) => {
                // Still failing. Bump attempts → false if we hit the cap.
                let err = after
                    .as_ref()
                    .and_then(|r| r.1.clone())
                    .unwrap_or_else(|| "unknown".into());
                match storage.mark_pending_attempted(id, &err)? {
                    true => still_failing += 1,
                    false => dropped += 1,
                }
            }
        }

        if (idx + 1) % 10 == 0 || idx + 1 == total {
            println!(
                "  [{}/{}] succeeded={succeeded} still_failing={still_failing} \
                 dropped_after_5_attempts={dropped}",
                idx + 1,
                total
            );
        }
    }

    println!(
        "\nDone: {succeeded} succeeded, {still_failing} re-queued with longer \
         backoff, {dropped} dropped (5 attempts exhausted)."
    );
    Ok(())
}

fn upgrade(config: &Config) -> Result<()> {
    // Locate the source tree. Either we're already inside it (Cargo.toml in
    // cwd), or the user has set MNEMONIC_SOURCE_DIR explicitly. No hardcoded
    // personal paths — the upgrade flow should work wherever the user keeps
    // the checkout.
    let source_dir = {
        let cwd = std::env::current_dir()?;
        if cwd.join("Cargo.toml").exists() {
            cwd
        } else if let Some(env_path) = std::env::var_os("MNEMONIC_SOURCE_DIR") {
            let p = std::path::PathBuf::from(env_path);
            if p.join("Cargo.toml").exists() {
                p
            } else {
                anyhow::bail!(
                    "MNEMONIC_SOURCE_DIR is set to {} but no Cargo.toml found there.",
                    p.display()
                );
            }
        } else {
            anyhow::bail!(
                "Cannot find mnemonic source. Run `mnemonic upgrade` from the source \
                 directory, or set MNEMONIC_SOURCE_DIR to point at the checkout root."
            );
        }
    };

    let home = dirs::home_dir().unwrap_or_default();

    println!("1/4  Building release binary...");
    let status = std::process::Command::new("cargo")
        .args(["install", "--path", "."])
        .current_dir(&source_dir)
        .status()?;
    if !status.success() {
        anyhow::bail!("cargo install failed");
    }
    println!("     Binary installed to ~/.cargo/bin/mnemonic");

    // Sync sibling binary location. Codex caught the foot-gun:
    // `~/.local/bin/mnemonic` was higher in $PATH than `~/.cargo/bin/`,
    // so `which mnemonic` resolved to the stale `.local/bin` copy after
    // every cargo install. Result: the user thought upgrade ran but the
    // shell kept invoking the old binary. Fix: copy + resign both
    // canonical locations on every upgrade.
    //
    // ad-hoc codesign (`codesign --force --sign -`) is required because
    // macOS AMFI (Apple Mobile File Integrity) refuses to exec a binary
    // whose embedded signature doesn't match the current inode. Plain
    // `cp` produces a new inode with stale signature → SIGKILL on exec.
    let cargo_bin = home.join(".cargo/bin/mnemonic");
    let local_bin = home.join(".local/bin/mnemonic");

    // CRITICAL: detect the symlink case BEFORE attempting any copy.
    // Codex P0 caught that `std::fs::copy(src, symlink_to_src)` follows
    // the symlink and TRUNCATES the source — so on the recommended
    // setup (~/.local/bin/mnemonic → ~/.cargo/bin/mnemonic) the next
    // upgrade would zero out the live binary that the daemon is
    // actively executing. Compare canonicalized targets; if they
    // resolve to the same path, treat as already-synced and only
    // codesign once.
    let local_meta = std::fs::symlink_metadata(&local_bin).ok();
    let local_is_symlink = local_meta
        .as_ref()
        .is_some_and(|m| m.file_type().is_symlink());
    let same_inode = match (
        std::fs::canonicalize(&cargo_bin),
        std::fs::canonicalize(&local_bin),
    ) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    };

    if same_inode {
        // .local/bin/mnemonic already resolves to .cargo/bin/mnemonic
        // (symlink, hard link, or identical canonical path). Skip the
        // copy that would truncate the live binary. Codesign the
        // canonical target once.
        println!(
            "     ~/.local/bin/mnemonic already resolves to the same target ({})",
            if local_is_symlink {
                "symlink"
            } else {
                "same path"
            }
        );
        let _ = std::process::Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&cargo_bin)
            .status();
        println!("     ✓ Canonical binary ad-hoc resigned");
    } else if local_bin.exists() {
        // Two genuinely distinct files. Safe to copy + resign.
        println!("     Syncing ~/.local/bin/mnemonic (was {})", {
            let meta = std::fs::metadata(&local_bin).ok();
            meta.and_then(|m| m.modified().ok())
                .and_then(|t| {
                    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| {
                        let secs = d.as_secs();
                        let now = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|n| n.as_secs())
                            .unwrap_or(secs);
                        let age = now.saturating_sub(secs);
                        if age < 60 {
                            format!("{age}s ago")
                        } else if age < 3600 {
                            format!("{}m ago", age / 60)
                        } else if age < 86400 {
                            format!("{}h ago", age / 3600)
                        } else {
                            format!("{}d ago", age / 86400)
                        }
                    })
                })
                .unwrap_or_else(|| "?".to_string())
        });
        if let Err(e) = std::fs::copy(&cargo_bin, &local_bin) {
            eprintln!("     warning: failed to copy to ~/.local/bin: {e}");
        } else {
            let _ = std::process::Command::new("codesign")
                .args(["--force", "--sign", "-"])
                .arg(&local_bin)
                .status();
            println!("     ✓ ~/.local/bin/mnemonic synced and ad-hoc resigned");
        }
        // Resign cargo_bin too — it just came out of `cargo install`
        // with a fresh inode whose embedded signature doesn't match.
        let _ = std::process::Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&cargo_bin)
            .status();
    } else {
        // Don't auto-create — user may not want a second copy. Just
        // surface the gap so they can decide. Still resign cargo_bin.
        println!(
            "     (~/.local/bin/mnemonic not present — skipping. \
             If PATH prefers .local/bin, create a symlink: \
             `ln -s ~/.cargo/bin/mnemonic ~/.local/bin/mnemonic`)"
        );
        let _ = std::process::Command::new("codesign")
            .args(["--force", "--sign", "-"])
            .arg(&cargo_bin)
            .status();
    }

    // Restart daemon. Codex P1 caught the launchd race: if the
    // LaunchAgent is loaded with KeepAlive=true and we stop+sleep+
    // start, launchd can race the manual start during the sleep
    // window, ending in the manual-daemon-vs-launchd-daemon hung
    // state we just spent hours debugging. Detect the loaded
    // service and use launchctl kickstart instead of `start -d`.
    println!("2/4  Restarting daemon...");
    let label = "com.kossvat.mnemonic.daemon";
    let launchctl_loaded = std::process::Command::new("launchctl")
        .arg("list")
        .output()
        .map(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout)
                    .lines()
                    .any(|l| l.contains(label))
        })
        .unwrap_or(false);

    if launchctl_loaded {
        // `kickstart -k` sends SIGTERM to the running instance and
        // then starts a fresh one — handles the lifecycle atomically
        // inside launchd, no race window.
        let kick = std::process::Command::new("launchctl")
            .args([
                "kickstart",
                "-k",
                &format!("gui/{}/{label}", unsafe { libc::getuid() }),
            ])
            .status();
        if kick.map(|s| s.success()).unwrap_or(false) {
            println!("     ✓ launchctl kickstart -k issued (launchd handles restart)");
        } else {
            // kickstart failed but launchd still owns the daemon — do
            // NOT spawn a manual `start -d` (it's now refused, and would
            // race launchd's KeepAlive anyway). Stop the current
            // instance and let launchd's KeepAlive respawn it cleanly.
            eprintln!(
                "     warning: launchctl kickstart failed; stopping and letting \
                 launchd KeepAlive respawn the daemon"
            );
            let _ = std::process::Command::new(&cargo_bin).arg("stop").status();
        }
    } else {
        // No launchd service active — use the manual path.
        let _ = std::process::Command::new(&cargo_bin).arg("stop").status();
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = std::process::Command::new(&cargo_bin)
            .args(["start", "-d"])
            .status();
    }
    match wait_for_daemon_ready(config, std::time::Duration::from_secs(30)) {
        Ok(pid) => println!("     ✓ daemon ready (PID {pid})"),
        Err(e) => eprintln!("     warning: daemon did not become ready after restart: {e}"),
    }

    // Rebuild widget if source exists
    let widget_dir = source_dir.join("clients/macos");
    if widget_dir.join("Package.swift").exists() {
        println!("3/4  Rebuilding widget...");

        // Kill old widget
        let _ = std::process::Command::new("pkill")
            .args(["-f", "MnemonicBar"])
            .status();
        std::thread::sleep(std::time::Duration::from_millis(500));

        let status = std::process::Command::new("swift")
            .args(["build"])
            .current_dir(&widget_dir)
            .status()?;
        if !status.success() {
            eprintln!("     Widget build failed, skipping");
        } else {
            println!("4/4  Launching widget...");
            let _ = std::process::Command::new(widget_dir.join(".build/debug/MnemonicBar"))
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .stdin(std::process::Stdio::null())
                .spawn();
            println!("     Widget launched");
        }
    } else {
        println!("3/4  Widget source not found, skipping");
        println!("4/4  Done");
    }

    println!("\nUpgrade complete!");
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

fn restart_daemon(config: &Config) -> Result<()> {
    if daemon::Daemon::launchd_service_loaded() {
        let label = daemon::Daemon::LAUNCHD_LABEL;
        let uid = unsafe { libc::getuid() };
        let status = std::process::Command::new("launchctl")
            .args(["kickstart", "-k", &format!("gui/{uid}/{label}")])
            .status()
            .context("launchctl kickstart failed to start")?;
        if !status.success() {
            anyhow::bail!("launchctl kickstart failed for {label}");
        }
        let pid = wait_for_daemon_ready(config, std::time::Duration::from_secs(30))?;
        println!("Restarted mnemonic via launchd ({label}, PID {pid})");
        return Ok(());
    }

    match daemon::Daemon::stop_running_daemon(config, 5)? {
        daemon::StopOutcome::AlreadyStopped => println!("mnemonic was not running; starting"),
        daemon::StopOutcome::StaleCleaned { pid } => {
            println!("Cleaned up stale PID {pid}; starting")
        }
        daemon::StopOutcome::GracefulExit { pid } => {
            println!("Stopped mnemonic (PID {pid}, graceful); starting")
        }
        daemon::StopOutcome::ForcedExit { pid } => {
            println!("Force-killed mnemonic (PID {pid}); starting")
        }
        daemon::StopOutcome::ForcedAndStuck { pid } => {
            anyhow::bail!(
                "force-kill sent to PID {pid} but process is still alive; reboot is the only reliable recovery"
            );
        }
    }
    daemonize()?;
    let pid = wait_for_daemon_ready(config, std::time::Duration::from_secs(30))?;
    println!("Daemon ready (PID {pid})");
    Ok(())
}

fn wait_for_daemon_ready(config: &Config, timeout: std::time::Duration) -> Result<u32> {
    let start = std::time::Instant::now();
    loop {
        match daemon::Daemon::status_check(config) {
            daemon::DaemonStatus::Running { pid } => return Ok(pid),
            daemon::DaemonStatus::Hung { pid } if start.elapsed() >= timeout => {
                anyhow::bail!("PID {pid} is alive but API socket is unresponsive")
            }
            daemon::DaemonStatus::Stopped | daemon::DaemonStatus::StalePid { .. } => {}
            daemon::DaemonStatus::Hung { .. } => {}
        }

        if start.elapsed() >= timeout {
            anyhow::bail!(
                "daemon did not become socket-responsive within {:?}",
                timeout
            );
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
}
