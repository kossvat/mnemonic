# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Secret redaction at capture, policy version 1. Text is redacted before it is classified, embedded or stored: `<private>` regions and recognized credentials (private keys, provider tokens, JWTs, bearer tokens, URL passwords, values under a credential name) become fixed markers. Every write path is covered, not only capture: MCP and CLI saves, import, facts, follow-ups, conclusions, the graph, peers and sessions, the shared store, and what a model generates (dream summaries, conclusions, consolidated memories, extracted graphs). A memory that was redacted says so in `metadata.redaction` (policy version and counts by class, never the text), and the `memory_save` reply and the `save` command report it.
- `mnemonic redact scan --db <PATH>`: a report-only scan of a closed copy of a store. It changes nothing and prints where the policy recognizes what class of thing (table, column, row number, counts), never the value. Exit codes: `0` clean, `2` findings, `1` incomplete or bad input.
- A changed value is an update, not a duplicate. A save that states a different price, rate, term or date than a near-identical memory is kept and linked as its update instead of being dropped by the similarity check; versions, ports, hashes and times still deduplicate. Recall (`memory_search`, `memory_similar`, `memory_recent`, `memory_context` with a topic) marks a replaced memory with the one that holds now.
- Facts v2 and the `memory_fact_set` / `memory_facts` MCP tools. A fact is a slot (project, subject, predicate, qualifier) with a dated chain of values; the current one is derived. Newer values replace, repeats reconfirm, backdated values join the history, retractions and erasure (`fact forget`) are explicit. The old `facts` table is imported on open and left as it was. The `fact` CLI works on v2 (`current`, `set`, `retract`, `forget`, `audit`).
- A Key facts block in project digests: current values, what each replaced when recent, proposals marked.
- The store records its schema generation in `PRAGMA user_version` and takes a one-time snapshot before an upgrade.
- Opt-in `shared` CLI and restricted MCP for explicitly published project context and attributed, idempotent observations pending review. Separate DB, fixed agent/project policy, revisions/revocation, request/response limits and a durable pending quota; no new network listener or automatic private-memory export.
- `search` CLI alias for `query`.

### Changed
- A name that holds a credential is refused with `SENSITIVE_CONTENT` instead of stored: ids, projects, subjects, keys, graph names, paths and the fields of a shared publication or observation. Errors say the code, not the input.
- Failed graph extractions are recorded as a fixed code (`BACKEND_FAILED`, `GENERATED_JSON_INVALID`, `STRUCTURAL_INPUT_REJECTED`, `STORAGE_FAILED`, `WORKER_FAILED`, `GENERATION_FAILED`) instead of the error's text. The extraction cache is kept per redaction policy version, so every memory is extracted once more after the upgrade; earlier cache rows are left where they are and not read.
- Search queries, `memory_similar` queries, context topics and texts sent to the daemon's `/embed` endpoint are redacted before they are embedded; a query that is a credential is searched by its marker. `/embed` answers a failure with `EMBEDDING_FAILED` and a bad request with fixed words.
- Error text is redacted before it is shown: MCP and HTTP error replies, and the command line, which prints an error and what caused it on one line.
- A save whose title and content read as a credential together (a title that ends in a credential name, a content that begins with a long value) has the value redacted in the content.
- Reflection consolidates a cluster with a member stored before the policy (its text is redacted in the consolidated memory) instead of skipping it. A cluster with a member filed under an id the policy refuses is left as it is.

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
- Secret redaction: what is recognized and what is not, which names are refused, that every writer has to be upgraded, that what was stored before is not rewritten, and how to scan a copy of a store.
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
