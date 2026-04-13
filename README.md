<p align="center">
  <img src="assets/logo.svg" alt="mnemonic" width="400"/>
</p>

<p align="center">
  <a href="https://github.com/kossvat/mnemonic/actions"><img src="https://github.com/kossvat/mnemonic/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/kossvat/mnemonic/releases"><img src="https://img.shields.io/github/v/release/kossvat/mnemonic?color=6366f1" alt="Release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue" alt="MIT License"></a>
</p>

<p align="center">
  Background memory daemon for AI coding agents.<br>
  Watches your project, captures decisions, and builds persistent memory — automatically.
</p>

---

## The Problem

AI coding agents lose context between sessions. You make architectural decisions, fix bugs, get corrected — and next session, the agent starts from scratch. Existing memory systems require manual saving, which means important context gets lost.

## The Solution

Mnemonic runs in the background and automatically captures:
- **Git commits** — classified by conventional commit type (feat → decision, fix → note)
- **File changes** — new files, dependency additions, significant modifications
- **User corrections** — when you override an agent's approach (highest priority)

Everything is deduplicated, scored for importance, and stored locally:
1. **SQLite** — with FTS5 full-text search and semantic embeddings
2. **Claude Code memory files** — agents see memories on session start
3. **Obsidian vault** (optional) — human-readable notes with tags and frontmatter

## Requirements

- **Rust 1.70+** — for building from source
- **Git** — for commit tracking (optional, works without it)
- **macOS or Linux** — Windows not yet supported

### Optional

- **Claude Code** — for MCP integration, memory files, and SessionStart hooks
- **Obsidian** — for vault output (disabled by default, works fine without it)

No external databases, no Docker, no API keys. Everything runs locally.

## Quick Start

### One-line install (from GitHub)

```bash
cargo install --git https://github.com/kossvat/mnemonic
```

### Or from source

```bash
git clone https://github.com/kossvat/mnemonic.git
cd mnemonic
cargo install --path .
```

### Setup

```bash
# Generate config (optional, sane defaults work out of the box)
mnemonic init

# Start daemon
mnemonic start -d

# Verify everything works
mnemonic doctor

# Check status
mnemonic status
```

### Use with Claude Code

Give Claude Code this repo link — it can set everything up automatically. Or manually:

**1. Auto-start daemon on session start**

Add to `.claude/settings.json` → `hooks.SessionStart`:

```json
{
  "type": "command",
  "command": "sh -c '~/.cargo/bin/mnemonic start -d 2>/dev/null && ~/.cargo/bin/mnemonic context 2>/dev/null || true'",
  "timeout": 5000
}
```

**2. Register MCP server** (gives Claude 6 memory tools)

Add to `~/.claude.json`:

```json
{
  "mcpServers": {
    "mnemonic": {
      "type": "stdio",
      "command": "~/.cargo/bin/mnemonic",
      "args": ["mcp"],
      "env": { "RUST_LOG": "error" }
    }
  }
}
```

MCP tools: `memory_search`, `memory_save`, `memory_recent`, `memory_similar`, `memory_context`, `memory_status`

## How It Works

```
                    ┌──────────────┐
                    │  File System │
                    │   (notify)   │
                    └──────┬───────┘
                           │ events
┌──────────┐       ┌──────▼───────┐       ┌─────────────┐
│   Git    │──────►│   Daemon     │──────►│  Classifier  │
│ (git2)   │       │  (tokio)     │       │  (rules)     │
└──────────┘       └──────┬───────┘       └──────┬───────┘
                          │                       │
                   ┌──────▼───────┐       ┌──────▼───────┐
                   │  Embedder    │       │   Scorer     │
                   │  (SimHash)   │       │  (dynamic)   │
                   └──────┬───────┘       └──────┬───────┘
                          │                       │
                          ▼ dedup check           ▼ importance
                   ┌──────────────┐
                   │   Storage    │
                   │  (SQLite +   │
                   │   FTS5)      │
                   └──────┬───────┘
                          │
              ┌───────────┼───────────┐
              ▼           ▼           ▼
        ┌──────────┐ ┌────────┐ ┌──────────┐
        │  Claude  │ │Obsidian│ │ Whisper  │
        │  Memory  │ │  Vault │ │ Context  │
        │  Files   │ │ (opt.) │ │ (.md)    │
        └──────────┘ └────────┘ └──────────┘
```

### Memory Flow

1. **Watch** — File watcher (FSEvents/inotify) and Git watcher (polling HEAD) emit events
2. **Batch** — Events collected in 5-second batches (urgent events bypass)
3. **Classify** — Rule-based classifier determines type and base importance
4. **Embed** — SimHash generates 256-dim embedding for dedup + similarity search
5. **Score** — Dynamic importance: `frequency × 0.3 + recency × 0.3 + signal × 0.4`
6. **Dedup** — Skip if cosine similarity > 0.92 with existing memory
7. **Store** — Write to SQLite (FTS5), Claude memory files, and optionally Obsidian vault

### Memory Types

| Type | Signal | Examples |
|------|--------|----------|
| `decision` | 0.7 | Architecture choices, tech selections |
| `feedback` | 1.0 | User corrections (always saved, never cleaned) |
| `note` | 0.4 | General observations, file changes |
| `session_summary` | 0.5 | Session start/end markers |
| `security` | 0.9 | Security-related changes |

### Importance Scoring

Dynamic formula considers three factors:

- **Frequency** (30%) — how often similar topics appear (patterns matter more)
- **Recency** (30%) — exponential decay, 24h half-life (recent topics = more relevant)
- **Signal** (40%) — event type strength (user correction > decision > note)

Memories below `importance_threshold` (default: 0.4) are discarded.

### Memory Cleanup

Database doesn't grow forever. Use `mnemonic cleanup` to remove old low-importance notes:

```bash
# Preview what would be cleaned
mnemonic cleanup --days 30 --threshold 0.5

# Actually clean
mnemonic cleanup --days 30 --threshold 0.5 --confirm
```

**Never cleaned:** decisions and feedback are kept permanently — they're too valuable to lose.

### Trait-Based Extensibility

Every component is a trait — swap implementations without changing the pipeline:

```rust
trait Watcher    // FileWatcher, GitWatcher, (future: ConversationWatcher)
trait Classifier // RuleClassifier, (future: LLM-based)
trait Embedder   // HashEmbedder, (future: NeuralEmbedder with MiniLM-L6-v2)
trait OutputSink // SQLite, MemoryFiles, Obsidian, (future: any)
```

## CLI Reference

```bash
# Daemon
mnemonic start [-d]          # Start daemon (foreground or -d for background)
mnemonic stop                # Stop running daemon
mnemonic status              # Show daemon status and memory stats
mnemonic doctor              # Diagnose setup issues

# Search & Browse
mnemonic query <text>        # Full-text search (FTS5)
mnemonic similar <text>      # Semantic similarity search
mnemonic recent [-l N]       # Show N most recent memories
mnemonic stats [--json]      # Stats with daily breakdown (JSON for widgets)

# Write
mnemonic save -t <title> <content> [-T type] [--tags a,b]  # Manual save
mnemonic context [-t topic]  # Generate context file (Whisper)

# Data Management
mnemonic export              # Export all memories as JSON (stdout)
mnemonic import <file>       # Import memories from JSON file (or - for stdin)
mnemonic cleanup [--days 30] [--threshold 0.5] [--confirm]  # Remove old notes

# Integration
mnemonic mcp                 # Run as MCP server (JSON-RPC over stdio)
mnemonic init                # Generate default config
```

## Configuration

Default config path: `~/.config/mnemonic/config.toml`

See [config.example.toml](config.example.toml) for all options.

Key settings:
- `classifier.importance_threshold` — minimum score to save (default: 0.4)
- `classifier.dedup_threshold` — cosine similarity for dedup (default: 0.92)
- `output.obsidian_enabled` — enable/disable Obsidian output (default: false)
- `output.memory_files_path` — where Claude Code memory files go

## Data Storage

All data stays local. No cloud, no API calls, no telemetry.

- **Database**: `~/.mnemonic/memory.db` (SQLite, auto-created)
- **Config**: `~/.config/mnemonic/config.toml`
- **PID file**: `~/.mnemonic/mnemonic.pid`
- **Log**: `~/.mnemonic/daemon.log`
- **Socket**: `~/.mnemonic/mnemonic.sock` (Unix domain socket for API)

### Backup & Migration

```bash
# Backup
mnemonic export > memories-backup.json

# Restore on new machine
mnemonic import memories-backup.json

# Duplicates are skipped automatically
```

## macOS Menu Bar Widget

Native SwiftUI widget for monitoring mnemonic from the menu bar.

```bash
cd clients/macos
swift build
.build/debug/MnemonicBar
```

Features: live stats, memory search, type filtering, expandable entries, quick save, daemon start/stop, context generation, activity alerts.

See [clients/macos/README.md](clients/macos/README.md) for details.

## Roadmap

- [x] File watcher (FSEvents/inotify via `notify`)
- [x] Git watcher (commit tracking via `git2`)
- [x] Rule-based classifier
- [x] SQLite + FTS5 storage
- [x] Claude Code memory file output
- [x] Obsidian vault output (optional)
- [x] Hash-based embeddings (SimHash, 256-dim)
- [x] Semantic deduplication
- [x] Dynamic importance scoring
- [x] Whisper (context injection)
- [x] MCP server (6 tools)
- [x] CLI (15 commands)
- [x] Auto-start via SessionStart hook
- [x] Export/import for backup and migration
- [x] Memory cleanup with TTL
- [x] Doctor diagnostics
- [x] macOS menu bar widget (SwiftUI)
- [ ] Neural embeddings (MiniLM-L6-v2 via candle)
- [ ] Conversation watcher (Claude Code session monitoring)
- [ ] Embedding backfill for old memories
- [ ] Web UI for browsing memories
- [ ] Linux tray widget
- [ ] Windows support

## Building

```bash
# Requires Rust 1.70+
cargo build --release

# Run tests
cargo test

# Install globally
cargo install --path .
```

Binary size: ~6MB (statically linked SQLite).

## License

MIT — see [LICENSE](LICENSE)
