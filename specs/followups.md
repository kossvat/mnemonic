# Follow-up lifecycle - build spec

Problem: the digest "Next / open" block and the Journal follow-ups are a keyword
heuristic over note titles (FOLLOWUP_MARKERS). A months-old "надо доделать X"
renders as current open work forever, and nothing can ever be closed.
Design reviewed before implementation.

Rule: keywords may SUGGEST a follow-up, never establish authoritative state.
Closing happens only through an explicit, ID-linked action. No language-based
auto-close in v1.

## Model
followups(id, project, title, status, source_memory_id, revision, created_at, updated_at)
  status: proposed | open | closed | dismissed
followup_events(id AUTOINCREMENT, followup_id, action, evidence_memory_id, actor,
  occurred_at, request_id UNIQUE)   -- append-only history, idempotency key
UNIQUE(source_memory_id) WHERE NOT NULL  -- one follow-up per source memory, so a
  replayed or re-swept memory can never resurrect a closed/dismissed item.

Transitions (transactional, optimistic revision check):
  propose -> proposed;  proposed -> open (confirm) | dismissed
  open -> closed;  closed -> open (reopen)
A retried request_id returns the current row without writing a second event.
A stale expected_revision is rejected. A project mismatch is rejected.

## Creation paths
- Sweep worker (deterministic, idempotent): recent project-linked Notes whose
  cleaned first line carries a marker -> `proposed` (actor=heuristic).
- CLI `mnemonic followup add|confirm|dismiss|close|reopen|list` -> authoritative.
- MCP tools memory_followups / memory_followup_open / memory_followup_close
  -> authoritative (actor=mcp).

## Recall
- Digest "Next / open": status=open rows for the project, newest first, queried
  directly (not limited to the 200-memory pool). With zero open rows it may show
  up to 2 proposals no older than 14 days, labelled "(unconfirmed)". Older
  proposals never render.
- Context "Open follow-ups (other projects)": the context renders at most 3
  project digests, so open rows of every project that did NOT get one are listed
  separately (max 5, newest first, `[project] title id`). How busy a project is
  must not decide whether an explicitly recorded item reaches the next session.
- Journal day: state reconstructed as of the end of that day from followup_events.

## Consolidation
`reflect --apply` folds near-duplicate notes into a canonical memory with a NEW
id, so the one-follow-up-per-source index cannot see it. The sweep therefore
walks `reflection_sources` (transitively: a canonical can be folded again) and
skips any note consolidated from a memory that already carries a follow-up in
any status. A canonical built only from untracked notes is proposed as usual.
Tracking is read from the follow-up's FIRST event (its evidence is the source,
no foreign key to memories) as well as the live link, so forgetting the source
later cannot reopen the trail.

Commit notes (source GitWatcher) never propose: a commit subject describes
finished work even when it contains a marker word. The rule follows the
headline through consolidation (the canonical is a Manual note that keeps a
source title): an ancestor commit carrying the headline blocks it, a person's
note carrying it allows it, and a synthesized headline is blocked only when
every surviving leaf is a commit.

## Source deletion
Forgetting the source memory deletes a still-`proposed` follow-up (it was only a
guess from that text); open/closed rows survive with source_memory_id = NULL.

## Verifier (machine-checkable, all in cargo test)
create->close->reopen history; duplicate request_id is a no-op; competing
revisions rejected; wrong-project rejected; source deletion policy; a completion
sentence in a new memory closes nothing; re-sweep never resurrects closed or
dismissed items, including through a consolidated canonical (depth two, and
after the source is forgotten); commit notes never propose;
an old open item still renders after 250 newer notes evict its source from the
pool; proposals older than 14 days never render; journal as-of
reconstruction.
