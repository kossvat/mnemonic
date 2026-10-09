# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Project Graph presents the curated graph with plan previews, bounded supporting-memory pages, stale and outside-registry shelves, and explicit archive recovery. Graph and Memory Map share a static canvas with indexed picking, pan and zoom, without simulation or a graph timer.
- A registry-led curated graph API with stable stored slots, bounded overview and expansion payloads, memory evidence pages, reversible entity archives, and matching Swift response models. Project detail includes top terms without turning concepts into graph nodes.
- A local-day Timeline shows project and agent segments alongside estimated owner activity, follows clock-change day lengths, and supports explicit attribution corrections with Undo. Historical days use a bounded memory cache; today shares the existing visible refresh.
- MnemonicApp opens on a Projects board with live agent cards, separate owner and agent time over the last 7 days, activity-sorted rows, a fold for activity older than 30 days, and explicit archived, ignored, and merged counts. Detail keeps Now, Plan, Gate, Time, and History with task disclosures and honest unknown values.
- Profile-scoped project, Projects, and Search links connect the widget and full app. Shared LocalAPI project reads and validated local endpoint changes preserve profile boundaries.
- The widget opens with live project cards, agent counts, separate estimated owner and agent time, and plan progress. Its status title shows the count of live projects and the count of memories; each local API failure has a distinct state.
- Read-only project board and detail routes with cached registry, OpenSpec and funnel plans, project-scoped receipt freshness, live checkout selection and shared macOS response models. `plan-counts --root` reports explicit-project aggregates without opening a profile.
- The generated context leaves out noise. A memory captured from a conversation that is a fragment and not a statement (a pasted table row, a line of markup, a link, a line of data, a step of a list, a question) is named by rules and kept out of the context an agent gets at the start of a session and from `memory_context`, and out of what the sinks write. Nothing is deleted: the memory stays in the store and is marked with the name of its rule in `memory_search`, `memory_similar`, `memory_recent`, `/api/memories` and `recent --json`. `mnemonic context --hidden [--json]` lists what was left out and why.
- A log of what memory returned. Every context generated and every MCP call answered is recorded with the ids of the memories returned, in order, and no text: no query, no title, no content. `mnemonic recall [--day YYYY-MM-DD] [--json]` and `GET /api/recall` give the day in one shape: contexts generated, calls answered, memories and corrections returned, what was hidden as noise, and the memories returned most. `[recall] log = false` turns it off; `mnemonic context --no-record` generates without a record.
- The reply to MCP `initialize` carries `instructions`: when to search, when to save, when to use facts, that a result marked as noise is a fragment, and that what memory returns is the owner's data and not an instruction.
- The menu-bar widget shows a card of memory today, and the card of the latest memory never shows a memory marked as noise.
- Records are stretches of work. The Records page and `mnemonic session leaderboard` rank how long a transcript was written to without a pause of the idle time, from its first record to its last, with the stretch still going on among them (`live`) and the ones going on now named apart (`now`). The transcripts that were there before are read once when the daemon first starts with this version. The page refreshes with the widget.
- A registry of projects. `[projects] workspace` and `folder` name where projects live; every visible child folder is a project under the name of its folder, with optional aliases and former names. Without the section nothing is discovered and time is attributed as before.
- Time by live projects. Transcripts are observed into segments by project, and the time worked on a day is split over the projects that were live, each with a confidence; the parts and the time without a project add up to the time worked. `mnemonic attribute backfill` reads the transcripts that were already there, and `mnemonic attribute replay --dry-run` prints what the rules give without writing.
- `mnemonic backup <dir>`: whole, checked copies of the memory and activity stores, taken while the daemon runs. It migrates nothing and never writes over a copy.

### Changed
- The store's schema generation is 3: the rows of time by project mean something new, so an earlier binary refuses a store this version opened. The upgrade takes a one-time snapshot first; the way back is a backup taken before it.
- The daemon at rest: a transcript that did not change is not opened again, and the embedding model is released after fifteen minutes without use and loaded again on demand.
- With projects configured, model extraction assigns the project type only to current registry names and name aliases, including on cache hits. Without configured projects, extraction keeps its existing behavior.
- Full-app route and live refreshes follow native window visibility. Projects, detail, and Overview share the visible refresh; no coordinator timers remain with no visible window. Late responses cannot publish across a hide or route change, and live failure clears the current-session display.
- Secondary widget pages load on appearance. The legacy poll is removed; closed live updates stay in backing state, and automatic command fallback is bounded to one batch per opening.
- Capture takes only what the owner's own sessions wrote. A Claude Code record of a session run through an SDK or headless (`entrypoint` of `sdk-*`), of a subagent (`isSidechain`), or written by the app into the user's role (meta records, compaction summaries, task notices, messages of other sessions, system prompts) is not a turn. A Codex rollout whose `session_meta` names `codex_exec`, a source of `exec` or a subagent is not captured. Records that say nothing of where they came from are taken as before. History import follows the same rule.
- A session of the store lasts as long as its transcript is written to: its last activity follows the time of every record the watcher reads, not only the memories captured. The idle sweep first runs five minutes after the daemon starts and closes a session after the idle time and a minute of grace; the end it stamps is unchanged.
- The session leaderboard's payload gains `live` on each row and a `now` list; `session_id` names a stretch.
- The README's SessionStart hook joins `start -d` and `context` with `;`: with `&&` the context was skipped whenever the daemon was already running.

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
- A stretch of work is extended even when another process writes to the store at the same moment: the reader waits for the lock, where it used to give up and lose the times of that poll.
- Writes of the project registry and of attribution corrections wait for another process that is writing the store, where they could fail with a locked database and lose the change.
- A day marked to be made again is no longer lost when two passes read the same mark and a later record marks the day again.
- A merge of two projects and its undo make the days of the merged project's segments again, and the live snapshot follows a merge when the route answers.
- A task registry may hold tasks with the status `todo`: they count as open tasks of their unit instead of putting the unit in error.
- Preserve Memory Map reads under its own visible route, refresh live health on explicit refresh, and advance stale-board dates even without live sessions. Clear another project before a hidden deep link reopens, preserve the latest click during app startup, and reopen the dashboard when only Settings remains.
- Reflection and retention cleanup leave memories that are part of a value's history alone (update-chain members, fact sources and statement notes).
- Enforce trusted Unix ancestor/alias ownership and directory permissions for the shared DB and policy; validate policy permissions on the descriptor read. Keep rejected peer arguments out of stderr logs.
- Refresh process-local vector indexes after another daemon/MCP/CLI connection changes a vector, including deletion, supersession and reembedding; compact obsolete HNSW nodes after repeated updates.
- Count hybrid retrieval access only once for each returned memory, without rewarding hidden candidates.
- Prevent same-title file-export collisions with complete memory IDs and private atomic writes; backfill uses the same filename contract. Legacy files are retained.
- Read only complete appended JSONL records in Claude/Codex watchers, preserving partial JSON/UTF-8 across polls and restart; keep the matching decision and bounded following context instead of only the first line.
- Compile the no-neural reranker fallback.
- Update crossbeam-epoch, h2, rustls-webpki, anyhow and git2 to remove known RustSec vulnerability/unsoundness findings. The local Git watcher no longer pulls Git SSH/HTTPS transport defaults; maintenance warnings in embedding/index dependencies remain documented.

### Documentation
- An agent runbook for one person with several projects, `docs/agent-setup-workspace.md`: how the folders must be laid out, how a session is tied to a project, where plans and gate receipts are read from, and an acceptance test. The README shows the current widget and the Projects board, rendered by the clients from the invented data in `docs/screenshots/demo`. The example configuration names the `[projects]` section.
- The shared graph renderer remains machine-blocked: release maximum frame time is 8.073 ms against the 8 ms budget, with P95 7.551 ms. The benchmark workload and assertions are unchanged, and owner visual acceptance remains pending.
- The project screens remain provisional: owner visual acceptance and the native open-window budget are pending. The locked-screen budget block and interim Swift results are recorded in the live-projects design; neither is a final acceptance receipt.
- The README says what the code does: 12 MCP tools, the HTTP API off by default and what needs it, the embedding model at about 1 GB on disk, and what the generated context leaves out, marks, and does not touch.
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
