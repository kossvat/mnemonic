# Shared context: implementation and architecture audit

Status: implemented and verified locally; no hub deployment or private-data migration is implied.

## Architecture decision

Keep private capture and shared publication separate inside Mnemonic. The existing
daemon captures development context on the owner's machine. A dedicated shared
store holds only deliberately published project knowledge and attributed agent
observations. It never opens the private DB, loads private config/vocabulary,
loads an embedding model, or runs the private memory/Obsidian/API output sinks.

Use one authoritative shared store on an always-on host. Mac clients and remote
workers read the same project revision through a restricted MCP connection.
Do not rsync a live SQLite database, expose the owner dashboard token, or use
append-only export/import as two-way synchronization. There is no new public
HTTP listener in this implementation.

```text
Owner's Mac: private Mnemonic, Claude/Codex, repositories
  | explicitly publish reviewed project brief / decisions / constraints
  v
Dedicated hub service: shared.db
  | published context (project + key + revision + source)
  v
Remote agent workers: fixed project and agent identity
  | observations with stable request_id and source
  v
Hub observations inbox -> owner review -> explicit new publication
```

This creates a common working context without giving all agents identical access.
A development handoff can publish keys such as `brief`, `constraints`, `current-work`
and `glossary`. Use source references containing a repository identity,
commit and document path, or a memory ID safe to disclose. Absolute local paths
are not portable project identities. The `source` string is provenance supplied
by the publisher/observer; Mnemonic does not fetch it or attest that its claim is true.

An agent result is an observation, not an owner decision, verified fact, or
permission to execute commands. `reviewed` acknowledges that a human/trusted
curator processed it; it does not automatically publish the text. Keep code in
git, and keep records owned by other systems in those systems.
There is no task lease/dispatcher or automatic agent wake-up in this first version.

## Implemented interface

All shared commands require an explicit absolute `--db` path before the subcommand.
The parent directory must already exist. On Unix, each traversed directory and
alias route must be owned by the service UID or root and must not allow group/world
write. Sticky ancestors may contain a trusted child (for private temporary
stores), but cannot directly contain the DB/sidecars or policy. Policy ownership
and permissions are checked on the same NOFOLLOW descriptor that is read.
Root/service-UID mutation, mount administration and ACL grants remain trusted
deployment concerns and must be reviewed before attaching workers.
A schema/application marker refuses an unrelated nonempty SQLite DB, including
the owner's memory.db. Existing broad file permissions, symlink/hardlink database
files and unsafe sidecars are rejected. This does not replace OS isolation.

Owner workflow, using example paths and reviewed non-sensitive input:

```bash
mnemonic shared --db /srv/mnemonic/shared.db publish \
  --project example-project --key brief --title "Current project brief" \
  --file /srv/mnemonic/reviewed/brief.txt --source "git:example@abc123:docs/brief.md"

mnemonic shared --db /srv/mnemonic/shared.db context --project example-project
mnemonic shared --db /srv/mnemonic/shared.db inbox --project example-project
mnemonic shared --db /srv/mnemonic/shared.db review \
  --project example-project --id OBSERVATION_ID --status reviewed
mnemonic shared --db /srv/mnemonic/shared.db revoke --project example-project --key brief
```

Publication updates a stable `(project_id, key)`; identical retries do not advance
revision. Changed content or source advances the project's revision. Revocation
removes the live text and advances revision; repeated revocation is a no-op.
Use one trusted publishing workflow per project: owner writes are serialized but
currently last-writer-wins. Compare-and-swap revision checks are future work before
allowing independent publishers to edit the same keys concurrently.
Context revision and rows come from a consistent SQLite snapshot. Context is
bounded and may be truncated; it is not a complete deletion manifest. Re-read
individual keys with `shared_get` after revision changes; null invalidates that key.
Revocation cannot recall text already placed in a model session or external cache.
Offline replication and deletion propagation to backups are not implemented.

Restricted policy file (owned by the service/administrator; not writable by agents):

```toml
version = 1
project_id = "example-project"
agent_id = "researcher"
allow_observations = true
max_observations_per_session = 20
```

Start the restricted server through a trusted launcher:

```bash
mnemonic shared --db /srv/mnemonic/shared.db serve \
  --policy /etc/mnemonic/policies/researcher.toml
```

The exposed MCP tools are `shared_context`, `shared_search`, `shared_get` and,
only when enabled, `shared_observe`. Tool arguments cannot choose project,
writer, trust or approval status. Owner publish/revoke/review/inbox and legacy
memory tools are unavailable through this server, including direct JSON-RPC
method calls. Observations enter only the fixed project's inbox with a fixed
writer, receipt ID, source, server time and `pending` status.

Retries use the same `request_id` and exact payload. Reusing that ID with different
content fails. There is a durable cap of 100 pending observations per project
and writer, checked after retry deduplication. The session cap counts distinct
accepted request IDs, so identical retries remain possible at that cap. These
caps are queue protection, not a full multi-user billing/rate-limit system.

Requests are newline-delimited MCP/JSON-RPC with 64 KiB frames; wire responses
are capped at 256 KiB, accounting for escaped tool text. Unknown arguments are
rejected; decode errors use generic stderr messages so peer text or terminal
control bytes are not copied into transport logs. Standard request `_meta` is accepted without granting authority.
Published content is limited to 32 KiB, titles to 240 bytes, references to 2048
bytes. Search is literal Unicode-lowercase substring matching within SQL-scoped
project rows, not semantic/vector search. This keeps the hub usable without
neural dependencies; its retrieval strategy can evolve independently.

Protocol baseline: [MCP tools](https://modelcontextprotocol.io/specification/2024-11-05/server/tools)
and [lifecycle](https://modelcontextprotocol.io/specification/2024-11-05/basic/lifecycle).

## Mac to server transport boundary

For a small initial deployment, use SSH stdio with a dedicated key per worker and
a server-side forced command selecting a fixed DB and policy. Disable shell,
PTY and port/agent forwarding for worker keys. The remote service and policy
must be outside worker-controlled containers/users; workers must not read the
hub DB or invoke owner commands. Do not use the owner's general SSH key.

A typical client launches `ssh -T -o BatchMode=yes mnemonic-researcher-hub`
where the host alias chooses the dedicated key and the server's authorized-key
policy forces the exact `shared ... serve` command. No command is accepted from
the model. Server authentication and transport encryption are provided by SSH;
the MCP policy supplies scope, not authentication. Configure and verify these
boundaries before attaching real agents. No SSH accounts/keys/configuration were
created by implementing this feature.

Changing a policy requires new sessions; the process snapshots policy at startup.
Revoking an agent requires revoking transport access and terminating its existing
session. Never present `--policy` as protection from a process with the same
OS user and arbitrary shell access to the hub. A deployment that serves several
tenants needs per-tenant scoping; a single project-scoped policy does not
authorize reading every tenant's records.

## Core audit and improvements

| Area | Finding | Action |
|---|---|---|
| Shared context | Full owner MCP has no agent/project ACL | New separate published store and fixed-policy restricted MCP; owner MCP remains owner-trusted |
| Provenance | External input could be saved as user feedback/decision | New agent path accepts only quarantined observations; private legacy MCP is not silently reclassified |
| Multiple processes | Long-lived HNSW cache missed inserts/updates by other connections | SQLite vector revision/change log; refresh changed IDs before query/dedup, consistent snapshot and dimension checks; compact obsolete nodes after repeated updates |
| Retrieval quality | Hybrid search rewarded intermediate candidates and counted winners multiple times | Touch final returned memories once; preserve side-effect-free evaluation |
| File outputs | Date/title filenames collided and writes were non-atomic | Full safe ID suffix, byte cap, private atomic replacement; legacy files retained |
| Small builds | No-neural reranker fallback referenced a macro as a value | Fix cfg-gated import and fallback so lightweight builds compile |
| CLI compatibility | Workspace adapters used `search`, binary accepted only `query` | Add `search` alias without removing `query` |
| Capture | Incomplete JSONL tail could be skipped permanently; decision content could collapse to an opening line | Shared complete-line reader for both transcript watchers, restart-safe byte boundaries and bounded excerpt starting at the matching decision |

The vector change log is a local cache-coherence mechanism, not a cross-machine
replication protocol. Its per-ID deletion markers are retained for long-lived
readers. Future maintenance can compact them only with a safe epoch/rebuild
strategy; deleting them arbitrarily breaks readers that have not caught up.

## Dependency audit

`cargo audit 0.22.1` fetched RustSec on 2026-09-05 (advisory DB commit
`5a0ebedfe8bdd2e295b171f4162f8c977bcad9a5`, last update 2026-09-02).
The original lockfile had five vulnerability advisories and three unsoundness
warnings. Presence of an affected dependency does not establish exploitability
through Mnemonic. Targeted updates remove those findings:

| Dependency | Before -> after | Advisory |
|---|---|---|
| crossbeam-epoch | 0.9.18 -> 0.9.20 | [RUSTSEC-2026-0204](https://rustsec.org/advisories/RUSTSEC-2026-0204.html) |
| h2 | 0.4.13 -> 0.4.16 | [RUSTSEC-2026-0258](https://rustsec.org/advisories/RUSTSEC-2026-0258.html) |
| rustls-webpki | 0.103.11 -> 0.103.13 | [0104](https://rustsec.org/advisories/RUSTSEC-2026-0104.html), [0098](https://rustsec.org/advisories/RUSTSEC-2026-0098.html), [0099](https://rustsec.org/advisories/RUSTSEC-2026-0099.html) |
| anyhow | 1.0.102 -> 1.0.103 | [RUSTSEC-2026-0190](https://rustsec.org/advisories/RUSTSEC-2026-0190.html) |
| git2 | 0.20.4 -> 0.21.0 | [0183](https://rustsec.org/advisories/RUSTSEC-2026-0183.html), [0184](https://rustsec.org/advisories/RUSTSEC-2026-0184.html) |

The git2 upgrade also resolves libgit2-sys to 0.18.8+1.9.7. Its default features
no longer enable SSH/HTTPS Git transports; Mnemonic's watcher only reads the local
repository and needs neither. No application package version bump was made.

Final lockfile scan: **0 vulnerability advisories, 0 unsoundness warnings**.
Four maintenance warnings remain: `bincode`, `core2`, `number_prefix`, `paste`;
`core2 0.4.0` is also yanked. These require planned upgrades/replacements in the
embedding/index dependency chains, not suppression of the audit. No ignored
advisories were added. `bincode` is used by HNSW dump loading, which Mnemonic does
not invoke; `core2` arrives through the image/AV1 dependency chain while Mnemonic
uses text embeddings. `number_prefix` formats download progress and `paste` is a
compile-time macro dependency. Plan those upgrades with retrieval/performance
evaluation rather than assuming every maintenance warning is an exposed endpoint.
Cargo audit does not replace reachability analysis,
runtime security testing or platform-specific validation.

## Remaining architecture work, in priority order

1. **Durable ingestion and original timestamps — implemented (2026-09-20).**
   Both transcript watchers now commit accepted events and byte cursors together
   in SQLite; the RAM channel carries only wake hints. Processing commits the
   derived memory, terminal receipt and async extraction enqueue together.
   Original source time survives replay. Raw turns have a seven-day observation
   TTL; `status` and `stats --json` expose the replay floor and pending count.
   Source identities and deletion suppressions remain after raw text expires.
   The verifier covers abrupt process exits around both commits, byte boundaries,
   rotation/truncation, timestamps, TTL and forgotten-record resurrection.
   Host gate and deployment acceptance remain separate from sandbox verification.
2. **Unified private write policy and sink delivery.** Existing owner sinks can
   still export raw content and fail best-effort. Add structured sensitivity,
   destination policy, transactional outbox/receipts and operational visibility.
   The new shared service bypasses those sinks; it does not retroactively fix or
   delete existing copies in Obsidian or a configured remote Memory API sink.
   A failed delivery to a remote sink is logged and does not fail the local
   save; a successful local save is therefore not evidence of remote replication.
3. **Lifecycle and recovery.** Export/import is entry interchange only: it does
   not preserve every relation, revision and deletion. Add versioned whole-store
   backup/restore verification, retention and explicit deletion semantics.
4. **Retrieval observability.** Surface partial failures from FTS/vector/graph
   instead of silently returning a weaker result. Expand the small seed eval
   with anonymized RU/EN facts, corrections, project-scoped queries and stale
   knowledge cases. Do not automatically send private eval data to public CI.
5. **Module boundaries.** Large storage/main modules mix schema, queries,
   administration and orchestration. Extract by responsibility behind existing
   contracts after regression coverage; avoid a broad rewrite during a data
   migration. Build new surfaces in bounded modules, as shared context does.

### Durable-ingress contract and limits

The private store adds only `ingest_events`, `ingest_cursors` and
`consumer_receipts`, with indexes and a late-installed memory-delete trigger.
The trigger is installed after existing table-rebuild migrations, alongside the
follow-up trigger's lifecycle boundary. It clears the raw payload and changes
linked receipts to `forgotten` for every memory-delete path. Receipts are not
foreign-key-cascaded away with the memory. This does not change the follow-up
lifecycle or turn the whole memory store into an event log.

Identity is namespaced by watcher. A conversation record is keyed by its `uuid`
ALONE: Claude Code mints that per message and copies it verbatim when /compact
or a resume carries earlier turns into a new transcript, while rewriting
`sessionId`, so a transcript-scoped key would re-ingest every copied turn (and
an ingress correction bypasses semantic dedup, so it would land in the store
twice). In practice many uuids reappear across transcripts under different
session ids, and no uuid binds two different captured texts. A Codex
record keeps its rollout scope plus the message `id` when supplied, because
Codex ids are only known to be unique within one rollout. The fallback uses
transcript filename, generation and byte position, never timestamp. Cursor
fingerprints use descriptor identity, a bounded prefix and a suffix at the
acknowledged offset; replacement, shortening and changed fingerprints bump the
generation and resume at zero. Changes completely overwritten between polls
cannot be recovered. A truncate/regrow that preserves those fingerprints is
not distinguishable from append. Message IDs still deduplicate across generation
changes, and conversation ones across transcripts; position-only records have
identity only within their generation.

A tick reads its namespace's cursors in ONE query (`ingest_cursors_with_prefix`,
keyed on the serialized opening of the stream key) and polls every transcript
against that snapshot, instead of one lookup and one lock of the shared
connection per transcript. The mutex is released before any filesystem work.
A failed bulk read fails the tick, never becomes an empty snapshot (which would
make every transcript look new and re-read it from byte zero). A stale snapshot
is safe because `append_ingest` still re-reads the cursor inside its write
transaction and refuses a mismatch; each committed cursor is recorded back into
the snapshot, so a second path sharing a stream within the same tick behaves
exactly as it did with fresh lookups. Idle tick cost: the marker check plus one
bulk read, independent of the number of transcripts.

A poll reads and hashes the bounded prefix ONCE: the cursor check hands its hash
to the checkpoint that follows, which recomputes only when the prefix actually
moved. Hashing the same bytes of the same descriptor twice was half the
steady-state read volume of an idle transcript, inline on the runtime, every
tick.

Legacy JSON offsets are imported once and left untouched. First adoption skips
complete history in previously unknown files, retaining partial tails. A durable
bootstrap marker ensures files created during later downtime are read from zero.
Adoption is per file: a transcript that cannot be opened, read or fingerprinted
is retried on later ticks while every healthy stream keeps being polled, and it
is never settled without a cursor, because a cursor-less file is read from byte
zero and would replay its whole history the moment it becomes readable. Only a
vanished path settles. The marker is written once nothing is left to retry, so
a still-unadopted transcript resumes rather than being taken for a new session;
its reason is logged once, not every tick. Both error paths of a pass are
part of the contract: a store failure partway through keeps the candidates the
pass never reached in the retry set, and the pass is recorded before the marker
write is attempted, so a transient SQLite error can neither drop files into a
byte-zero replay nor let the next tick adopt a session started in between as
history.

An append during a poll is not a rewrite. The snapshot is bounded by the length
read before parsing, so a turn that lands mid-poll belongs to the next one;
refusing the batch for it would stop the cursor of a continuously written
transcript from advancing at all. A poll is refused only when the file shrank,
the bytes it just consumed no longer match the file, or the resumed cursor's own
fingerprints no longer match.
The reader acknowledges only complete newline-delimited UTF-8 records. Source
timestamps are stored verbatim as `source_at`; valid RFC3339 timestamps become
Event/MemoryEntry time. Missing or invalid source time uses observation time,
while retaining the original invalid value for local inspection.

The payload carries the bounded capture EVENT and nothing else. The unabridged
turn is not stored: nothing ever read it back, so it was a second copy of
private text without a reader. Rows written before that are migrated in place
(the field is stripped, identities, timestamps, receipts and cursors untouched,
and the schema version deliberately NOT bumped, which would strand them behind
the version filter). What the event itself holds is bounded twice for a
decision: text before the decision line is dropped and the tail is capped. A
correction is not bounded, because the whole message IS the memory.

Downgrade note: a build from BEFORE this change requires the removed field and
cannot read a payload written after it. The supported rollback target is the
pre-ingress binary, which does not read these tables at all; rolling back to an
interim ingress build is not supported. In practice the exposure is one tick,
since a terminal receipt clears the payload immediately.

A terminal receipt drops the payload in the SAME transaction, for every outcome
including the skipped ones, and rows that reached a receipt under an older build
are cleared on open. A row still pending keeps its payload; it is the only kind
that replays. That is what gives a skipped turn a deletion path at
all: it produced no memory, so `forget` has no id to aim at and previously only
the TTL would ever have cleared its text. The source-key tombstone and the
receipt stay, so replay is still idempotent.

The fixed seven-day TTL is based on `observed_at`, including unprocessed events.
Expired raw payloads become NULL and unprocessed events get `expired` receipts;
small source-key tombstones, timestamps and receipts remain for idempotency.
Selection and processing both enforce the floor, including expiry between read
and commit. Cleanup runs on store open and daemon ticks. This is logical
retention, not forensic erasure of external filesystem snapshots, source JSONL,
or WAL pages held by other readers. Entry exports and sinks receive only derived
memories. Automatic `.bak` snapshots scrub ingress payloads before publication;
they cannot restore pending raw ingress. Whole-store backup/restore remains the
separate recovery work above.

Durable outcomes are `saved`, `filtered`, `duplicate`, `low_importance`, `expired`
and `forgotten`. Only a newly committed memory invokes the existing downstream
sinks. Peer/session attribution, synchronous graph extraction and sink delivery
remain post-commit best effort; a transactional outbox for those effects is not
part of this change. The async extraction enqueue is included in the memory
transaction, and for that reason the post-commit path does NOT enqueue an
ingress entry a second time: the worker can finish and dequeue the first job
while attribution runs, and a later `INSERT OR IGNORE` would then create a
duplicate job. Non-ingress events still enqueue post-commit as before. File/git/manual ingestion retains its existing behavior. Added
Rust interfaces are the `ingest` module and Storage ingress methods; CLI
`stats --json` adds an `ingress` object with `raw_ttl_days`, `replay_since`,
`oldest_pending_seq` and `pending`.

The machine verifier is `cargo test --release ingress_` (also run with
`--no-default-features`). Child processes exit without unwinding immediately
before/after the ingest commit and before/after the processing commit; reopening
must preserve accepted events and create one memory and one extraction job.
Additional tests cover concurrent consumers, transaction write failures, legacy
migration and rebuild-trigger survival, both parsers, identical timestamps,
UTF-8 splits, rotation/truncation, TTL races, export/backup exclusion and the real
daemon processor's terminal outcomes, per-file adoption of an unreadable
transcript, both bootstrap error paths under an injected store failure, a
forked transcript re-offering the turns it copied, prefix-hash reuse skipped
when the prefix grows, an idle tick costing one bulk cursor read regardless of
file count, a stale snapshot refused by the cursor CAS, a failed bulk read
failing the tick, fresh-lookup semantics preserved within one tick, a payload that holds the event alone under the real
decision bounds, a terminal receipt dropping the payload for saved and skipped
alike, in-place migration of a row written before the turn text was dropped, an
append arriving mid-poll, a memory id that stays citable by its
first eight characters, and a single extraction enqueue when a worker takes the
transactional job during attribution. Full project gates are still required on
the host before committing or deployment. Sandbox verification uses the same
release clippy/test feature matrix, with three explicit test exclusions:
`activity_worker::tests::idle_read_returns_a_sane_value_on_macos` (CoreGraphics
hang), `shared::filesystem::tests::macos_system_var_alias_is_supported_for_private_temporary_directories`
(`/var/tmp` write denied), and
`daemon::lifecycle_tests::status_check_running_when_pid_alive_and_socket_accepts`
(Unix bind denied). The existing embedding client tests
`daemon_embedder_happy_path`, `dim_mismatch_latches_and_never_falls_back` and
`bad_request_hard_fails_without_fallback` also return early when bind is denied;
their reported passes are not socket-transport evidence. Existing ignored tests
for local reranker initialization and Ollama extraction remain ignored.

## Verification and rollout

Compatibility: `shared` and the Rust `shared` module are additive public interfaces;
existing owner commands/configuration remain available and `query` gains the
`search` alias. Private storage adds cache-coherence tables/triggers on open, not
a cross-machine data migration. Newly exported note filenames include the full
memory ID; old filenames are retained, so backfill can create an additional file
for a legacy export. A changed title/date also creates a new filename. Cleanup
of previous copies requires a separately reviewed migration.

Use temporary stores and synthetic transcripts, never inject test clients into
the real memory. Required checks include cross-project negative tests, fixed
writer enforcement, owner-tool rejection, private-DB refusal, UTF-8 framing,
retry conflicts/quotas, revocation in an existing session, wire-size boundaries,
cross-connection vector freshness, access-count correctness and file collisions.

Local results, Rust 1.94.1 on macOS arm64, including the follow-up directory/alias
checks and generic stderr rejection:

- `cargo fmt --all --check`: passed.
- `cargo clippy --offline --locked --release --all-targets -- -D warnings`: passed.
- Same release clippy with `--no-default-features`: passed.
- `cargo test --offline --locked --release`: 700 passed, 3 ignored test executions
  across library, binary and integration targets (some unit suites run in both targets).
- Same full release tests with `--no-default-features`: 700 passed, 3 ignored.
- Both feature sets include a real compiled CLI -> stdio MCP -> SQLite round trip.
- Ignored cases remain the existing reranker model-load smoke (in two targets)
  and Ollama extraction-quality evaluation; model quality was not re-evaluated.
- `cargo audit --no-fetch --json` against the freshly fetched DB: exit 0, with
  the maintenance/yanked warnings detailed above retained and disclosed.
- Scoped diff/format and local documentation link checks passed. No Linux or
  Windows runtime validation and no real remote-worker network exchange were performed.

Installing the new binary and restarting old daemon/MCP processes are separate
from changing source. Back up before installation. Do not publish private
history, change live sinks, copy personal memory, or enable scheduled agent runs
as a side effect of this release. The first live rollout should publish a few
reviewed non-sensitive project records and test two isolated clients against
the same hub before broadening scope.
