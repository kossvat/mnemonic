# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- A changed value is an update, not a duplicate. A save that states a different price, rate, term or date than a near-identical memory is kept and linked as its update instead of being dropped by the similarity check; versions, ports, hashes and times still deduplicate. Recall (`memory_search`, `memory_similar`, `memory_recent`, `memory_context` with a topic) marks a replaced memory with the one that holds now.
- Facts v2 and the `memory_fact_set` / `memory_facts` MCP tools. A fact is a slot (project, subject, predicate, qualifier) with a dated chain of values; the current one is derived. Newer values replace, repeats reconfirm, backdated values join the history, retractions and erasure (`fact forget`) are explicit. The old `facts` table is imported on open and left as it was. The `fact` CLI works on v2 (`current`, `set`, `retract`, `forget`, `audit`).
- A Key facts block in project digests: current values, what each replaced when recent, proposals marked.
- The store records its schema generation in `PRAGMA user_version` and takes a one-time snapshot before an upgrade.
- Opt-in `shared` CLI and restricted MCP for explicitly published project context and attributed, idempotent observations pending review. Separate DB, fixed agent/project policy, revisions/revocation, request/response limits and a durable pending quota; no new network listener or automatic private-memory export.
- `search` CLI alias for `query`.

### Fixed
- Reflection and retention cleanup leave memories that are part of a value's history alone (update-chain members, fact sources and statement notes).
- Enforce trusted Unix ancestor/alias ownership and directory permissions for the shared DB and policy; validate policy permissions on the descriptor read. Keep rejected peer arguments out of stderr logs.
- Refresh process-local vector indexes after another daemon/MCP/CLI connection changes a vector, including deletion, supersession and reembedding; compact obsolete HNSW nodes after repeated updates.
- Count hybrid retrieval access only once for each returned memory, without rewarding hidden candidates.
- Prevent same-title file-export collisions with complete memory IDs and private atomic writes; backfill uses the same filename contract. Legacy files are retained.
- Read only complete appended JSONL records in Claude/Codex watchers, preserving partial JSON/UTF-8 across polls and restart; keep the matching decision and bounded following context instead of only the first line.
- Compile the no-neural reranker fallback.
- Update crossbeam-epoch, h2, rustls-webpki, anyhow and git2 to remove known RustSec vulnerability/unsoundness findings. The local Git watcher no longer pulls Git SSH/HTTPS transport defaults; maintenance warnings in embedding/index dependencies remain documented.

### Documentation
- Clarify entry export/import limitations and the trusted-launcher boundary for shared context; document Mac-to-hub architecture and remaining ingestion/sink/recovery work.

## [0.1.0] - 2026-04-12

### Added
- Background daemon with file watcher (FSEvents/inotify) and git watcher
- Rule-based classifier for memory types (decision, feedback, note, security, session_summary)
- SQLite storage with FTS5 full-text search
- SimHash embeddings (256-dim) for semantic deduplication
- Dynamic importance scoring (frequency x 0.3 + recency x 0.3 + signal x 0.4)
- Claude Code memory file output (auto-detected project paths)
- Obsidian vault output (optional, disabled by default)
- Whisper context injection -- generates CONTEXT.md with prioritized memories
- MCP server with 6 tools (memory_search, memory_save, memory_recent, memory_similar, memory_context, memory_status)
- CLI with 14 commands (start, stop, status, query, similar, recent, save, context, export, import, cleanup, doctor, mcp, init)
- Export/import for backup and migration
- Memory cleanup with configurable TTL and importance threshold
- Doctor command for diagnosing setup issues
- Auto-start via Claude Code SessionStart hook
