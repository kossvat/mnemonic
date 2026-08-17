# Business memory — build spec (phase 1+2)

Goal: mnemonic remembers BUSINESS knowledge, not just code — domain facts per
project (product models, prices, supplier terms, commissions), a client base
built passively from conversation, marketing decisions. Design reviewed by
Codex Sol 2026-08-17 (verdict: build with changes; all 10 required changes
are folded into the items below).

Verifier-first: every item lists its machine check. An item is DONE only when
its check is green (`cargo fmt && cargo clippy --release --all-targets --
-D warnings && cargo test --release`) plus the item-specific check. Codex
review after each commit (Sol for A2/B2/B4 and the final gate; Terra rest).

## Stage A — foundation (schema, safety, lifecycle)

- [ ] A1. Fact model v2: stable subject entity ids (survive graph rebuilds),
  normalized qualifier in the key (price granularity per model/variant),
  controlled predicate vocabulary, provenance columns (source machine,
  extractor version, evidence span), status: manual | confirmed | provisional
  | pending_review. FK facts.source_memory_id -> memories with cascade
  handled in forget/cleanup (deleting a source memory removes/recomputes its
  derived facts). Check: migration tests + forget-cascade test.
- [ ] A2. Local-only sink policy, centrally enforced: memories/facts flagged
  sensitive (business/client) never leave the machine through ANY sink
  (memory_api etc.). Policy lives at the daemon fan-out, not in tags.
  Check: sink-routing test proves a business memory reaches local sinks only.
- [ ] A3. `mnemonic backup` + scheduled snapshots: WAL-checkpointed,
  integrity-checked archive of ~/.mnemonic; restore path documented.
  Check: backup -> restore into temp dir -> counts and spot queries match.

## Stage B — capture (queue, extractor, policy)

- [ ] B1. Durable raw-turn queue: ALL user-authored turns from Claude AND
  Codex watchers enter a local TTL-bound queue keyed by stable transcript id
  + original timestamp (replay-idempotent). The business heuristic only
  PRIORITIZES the queue, never gates it. Assistant turns excluded.
  Check: queue tests (idempotent replay, TTL expiry, priority order).
- [ ] B2. LLM fact extractor (local Ollama, strict versioned JSON envelope,
  deny_unknown_fields): per turn returns facts
  {subject, subject_type: client|product|supplier|campaign|project|other,
  predicate (controlled vocab), qualifier, value, evidence, confidence}.
  Rust-side enforcement: evidence span must literally occur in the source
  turn; numeric value+currency/unit must appear inside the evidence; caps on
  facts-per-turn and field lengths; duplicate keys within a response
  rejected; confidence finite in [0,1] and capped at 0.8 for LLM facts.
  Valid empty array = success; backend/malformed failure retries then
  dead-letters (never silent skip). Extraction cache key includes prompt +
  schema version, not just model. Check: extractor unit tests incl.
  hallucinated-evidence rejection + cache-version test.
- [ ] B3. Business updates bypass semantic dedup and note-importance
  filtering ("цена теперь $6" after "цена $5" is an UPDATE, not a dup).
  Check: test — two similar price turns both land; fact supersedes cleanly.
- [ ] B4. Trust policy: manual (CLI) facts are never auto-superseded by LLM
  output; a provisional fact conflicting with a current money-class fact
  (price/budget/commission/discount/deadline/supplier term) goes to
  pending_review instead of replacing it. Check: policy matrix tests.
- [ ] B5. Capture-gate heuristic (priority signal only): proximity rules
  (amount adjacent to currency/commercial unit + commercial language;
  relationship phrases "новый клиент X" / "supplier terms" need no number);
  weak-evidence downranking for AI-model words, bare percentages,
  resolutions/timestamps/fps/versions/ports/hashes/code/URLs. Reuse
  attachment stripping; drop the 10-char minimum for fact turns ("Price $5"
  is valid). Check: RU/EN positive+negative corpus test with recorded
  hit/miss rates (precedent: correction detector corpus).
- [ ] B6. Subject canonicalization v1: exact + alias match before creating
  entities; new clients get entity_type=client; alias merge stays manual.
  Check: variant-name test links to one entity via aliases.

## Stage C — recall surfaces

- [ ] C1. Project digest gains a "Key facts" block: current facts for
  entities linked to the project, ranked (not alphabetical), status-aware
  (provisional marked, pending_review hidden), each line cites its source
  memory id, capped, inside the existing 10k context budget.
  Check: digest tests (ranking, status visibility, budget).
- [ ] C2. `mnemonic client <name>` CLI + facts in /api/entities/{name} +
  MCP tool memory_facts(subject): current facts with status, confidence,
  source; history flag. Check: CLI/API/MCP round-trip tests.
- [ ] C3. Live verification on the real DB: seed a realistic business
  conversation through a test JSONL (like the 2026-08-17 watcher selftest),
  confirm facts extracted with correct evidence, digest shows them, client
  card renders; then clean the test artifacts.

## Deploy & wrap-up (per repo runbook)

Release build -> install to ~/.cargo/bin + codesign --force --sign - ->
launchctl restart daemon -> live checks (extractor log lines, facts count,
digest block) -> dual push (private origin + clean-copy public mirror with
sanity grep) -> session note.

## Out of scope for this loop

Widget UI for facts/clients; Hermes/OpenClaw adapters; multi-machine sync
(hub model decided: the always-on machine is the single brain; migration =
copy ~/.mnemonic). These build on top later.
