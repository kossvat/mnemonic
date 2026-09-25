//! Follow-up lifecycle — explicit open/closed records instead of a keyword
//! heuristic (see `specs/followups.md`, design reviewed before implementation).
//!
//! The digest "Next / open" block and the Journal used to scan note titles
//! for words like "todo" / "надо". A months-old intention therefore rendered
//! as current open work forever, and nothing could ever be closed.
//!
//! Invariants:
//! - Keywords may only PROPOSE a follow-up (`sweep`). Authoritative state
//!   (`open` / `closed`) is established solely by an explicit, ID-linked
//!   action from the CLI or an MCP caller. Nothing is auto-closed from prose.
//! - `followup_events` is append-only; `followups` holds the current state and
//!   an optimistic `revision`.
//! - `request_id` is an idempotency key: a retried request is a no-op that
//!   returns the current row.
//! - One follow-up per source memory (partial UNIQUE index), so a re-swept or
//!   replayed memory can never resurrect a closed or dismissed item.

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use rusqlite::{OptionalExtension, params};

use crate::journal::{FOLLOWUP_MARKERS, clean_line, is_noise_title};
use crate::storage::{Storage, is_meta_memory};

/// A keyword proposal older than this never renders: an unconfirmed guess
/// about an intention from weeks ago is history, not open work.
pub const PROPOSAL_MAX_AGE_DAYS: i64 = 14;
const SWEEP_BATCH: i64 = 200;
const SWEEP_MAX_PAGES: usize = 100;
const MAX_TITLE_CHARS: usize = 300;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Proposed,
    Open,
    Closed,
    Dismissed,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Proposed => "proposed",
            Status::Open => "open",
            Status::Closed => "closed",
            Status::Dismissed => "dismissed",
        }
    }

    fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "proposed" => Status::Proposed,
            "open" => Status::Open,
            "closed" => Status::Closed,
            "dismissed" => Status::Dismissed,
            other => bail!("unknown follow-up status {other:?}"),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// proposed -> open (confirm a keyword proposal)
    Open,
    /// proposed | open -> closed (done)
    Close,
    /// closed | dismissed -> open
    Reopen,
    /// proposed | open -> dismissed (not going to happen)
    Dismiss,
}

impl Action {
    fn as_str(self) -> &'static str {
        match self {
            Action::Open => "open",
            Action::Close => "close",
            Action::Reopen => "reopen",
            Action::Dismiss => "dismiss",
        }
    }

    /// The state machine. `None` = the transition is not allowed.
    fn apply(self, from: Status) -> Option<Status> {
        match (self, from) {
            (Action::Open, Status::Proposed) => Some(Status::Open),
            (Action::Close, Status::Proposed | Status::Open) => Some(Status::Closed),
            (Action::Reopen, Status::Closed | Status::Dismissed) => Some(Status::Open),
            (Action::Dismiss, Status::Proposed | Status::Open) => Some(Status::Dismissed),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct Followup {
    pub id: String,
    pub project: String,
    pub title: String,
    pub status: Status,
    pub source_memory_id: Option<String>,
    pub revision: i64,
    pub created_at: String,
    pub updated_at: String,
}

pub struct NewFollowup<'a> {
    pub project: &'a str,
    pub title: &'a str,
    pub source_memory_id: Option<&'a str>,
    /// true = created `open` by an explicit caller; false = a keyword proposal.
    pub authoritative: bool,
    pub actor: &'a str,
    pub request_id: &'a str,
}

pub struct Transition<'a> {
    pub id: &'a str,
    pub action: Action,
    /// When set, the transition is rejected unless the row is at this revision.
    pub expected_revision: Option<i64>,
    /// When set, the transition is rejected unless the row belongs to it.
    pub project: Option<&'a str>,
    pub evidence_memory_id: Option<&'a str>,
    pub actor: &'a str,
    pub request_id: &'a str,
}

const COLUMNS: &str =
    "id, project, title, status, source_memory_id, revision, created_at, updated_at";

fn row_to_followup(r: &rusqlite::Row<'_>) -> rusqlite::Result<(Followup, String)> {
    let status: String = r.get(3)?;
    Ok((
        Followup {
            id: r.get(0)?,
            project: r.get(1)?,
            title: r.get(2)?,
            status: Status::Proposed, // fixed up by the caller from the raw text
            source_memory_id: r.get(4)?,
            revision: r.get(5)?,
            created_at: r.get(6)?,
            updated_at: r.get(7)?,
        },
        status,
    ))
}

fn finish(pair: (Followup, String)) -> Result<Followup> {
    let (mut f, raw) = pair;
    f.status = Status::parse(&raw)?;
    Ok(f)
}

fn by_id(conn: &rusqlite::Connection, id: &str) -> Result<Option<Followup>> {
    conn.query_row(
        &format!("SELECT {COLUMNS} FROM followups WHERE id = ?1"),
        params![id],
        row_to_followup,
    )
    .optional()?
    .map(finish)
    .transpose()
}

/// The follow-up an already-seen `request_id` belongs to, if any.
fn by_request(conn: &rusqlite::Connection, request_id: &str) -> Result<Option<Followup>> {
    let fid: Option<String> = conn
        .query_row(
            "SELECT followup_id FROM followup_events WHERE request_id = ?1",
            params![request_id],
            |r| r.get(0),
        )
        .optional()?;
    match fid {
        Some(fid) => by_id(conn, &fid),
        None => Ok(None),
    }
}

/// Resolve a project name to the key follow-ups are stored under. After
/// `merge_entities(canonical, alias)` the alias name is obsolete: work stored
/// under it would be invisible to the canonical project's digest and queries,
/// and the merge-time update only repairs rows that already existed (review
/// point). Every entry point that takes a project name goes through here.
///
/// Resolution is the GRAPH's own (`Storage::canonical_for_alias_conn`:
/// case-insensitive, chain-following, cycle-safe) rather than a second
/// lookalike — "Legacy" must land where "legacy" does (review point). With no
/// alias, an existing entity is matched case-insensitively so the stored key
/// is the entity's real name; an unknown project keeps its trimmed spelling.
pub(crate) fn canonical_project(conn: &rusqlite::Connection, name: &str) -> Result<String> {
    let name = name.trim();
    // Aliases are registered under the graph's spelling. The graph resolver
    // only lowercases its input, so "Old Project Name" would miss an alias
    // stored as "old-project-name" — and after a merge the alias ENTITY is
    // gone too, leaving nothing else to match. Try the normalized form as
    // well before falling back to the supplied spelling (review point).
    for candidate in [name.to_string(), project_key(name)] {
        if let Some(canonical) = Storage::canonical_for_alias_conn(conn, &candidate)? {
            return Ok(canonical);
        }
    }
    // An existing project entity wins, matched first on the spelling given
    // and then on the graph's canonical form — extraction stores entities as
    // `canonicalize_name` output ("Example Org" lives as "example-org")
    // without registering an alias, and the digest looks its memory pool up
    // by that real entity name.
    for candidate in [name.to_lowercase(), project_key(name)] {
        let existing: Option<String> = conn
            .query_row(
                "SELECT name FROM entities
                  WHERE lower(name) = ?1 AND entity_type = 'project'
                  ORDER BY mention_count DESC, name ASC LIMIT 1",
                params![candidate],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
    }
    Ok(name.to_string())
}

/// The key follow-up queries match on: the resolved project name run through
/// the GRAPH's own `canonicalize_name` (lowercase, separators to dashes,
/// filler words stripped). Graph extraction creates entities under exactly
/// that form without registering an alias, so "Example Org" typed by a
/// caller and the "example-org" entity must map to one key (review point).
/// It also means casing can never split a project — not even for a row
/// created before its entity existed. `project` keeps the display spelling.
pub(crate) fn project_key(name: &str) -> String {
    let canonical = crate::graph::canonical::canonicalize_name(name);
    if canonical.is_empty() {
        // A name made only of separators/filler canonicalizes to nothing;
        // fall back to a plain fold rather than collapsing projects into "".
        name.trim().to_lowercase()
    } else {
        canonical
    }
}

/// `canonical_project` for callers that don't hold the connection.
pub(crate) fn resolve_project(storage: &Storage, name: &str) -> Result<String> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    canonical_project(&conn, name)
}

/// Open a write transaction with `BEGIN IMMEDIATE`. A deferred transaction
/// that reads first and writes later must UPGRADE its lock, and SQLite fails
/// that upgrade with "database is locked" as soon as another connection (the
/// daemon ingesting memories) wrote in between — the busy timeout does not
/// apply to a stale-snapshot upgrade. Taking the write lock up front makes
/// the busy timeout do its job, so unrelated ingestion can't reject an
/// explicit follow-up action (review point).
fn write_tx(conn: &rusqlite::Connection) -> Result<rusqlite::Transaction<'_>> {
    Ok(rusqlite::Transaction::new_unchecked(
        conn,
        rusqlite::TransactionBehavior::Immediate,
    )?)
}

/// Create a follow-up. Idempotent on `request_id` AND on `source_memory_id`:
/// either already existing returns the existing row untouched — in particular
/// a closed or dismissed item is never flipped back by a re-proposal.
pub fn create(storage: &Storage, new: NewFollowup<'_>, now: DateTime<Utc>) -> Result<Followup> {
    match create_outcome(storage, new, now)? {
        CreateOutcome::Created(f) | CreateOutcome::Existing(f) => Ok(f),
        CreateOutcome::SourceGone => bail!("source memory no longer exists"),
    }
}

/// What `create_outcome` did. `SourceGone` = the source memory was forgotten
/// between candidate selection and insertion; nothing was written.
pub enum CreateOutcome {
    Created(Followup),
    Existing(Followup),
    SourceGone,
}

pub fn create_outcome(
    storage: &Storage,
    new: NewFollowup<'_>,
    now: DateTime<Utc>,
) -> Result<CreateOutcome> {
    let project = new.project.trim();
    let title = normalize_title(new.title);
    if project.is_empty() {
        bail!("follow-up needs a project");
    }
    if title.trim().is_empty() {
        bail!("follow-up needs a title");
    }
    if new.request_id.trim().is_empty() {
        bail!("follow-up needs a request id");
    }

    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let tx = write_tx(&conn)?;
    let project = canonical_project(&tx, project)?;
    if let Some(existing) = by_request(&tx, new.request_id)? {
        return Ok(CreateOutcome::Existing(existing));
    }
    if let Some(src) = new.source_memory_id {
        // Checked INSIDE the insertion transaction (review point): the delete
        // trigger can't remove a follow-up that didn't exist when it fired,
        // and source_memory_id has no FK, so a proposal inserted after its
        // source was forgotten would resurface forgotten text forever.
        let alive: bool = tx
            .query_row("SELECT 1 FROM memories WHERE id = ?1", params![src], |_| {
                Ok(true)
            })
            .optional()?
            .unwrap_or(false);
        if !alive {
            return Ok(CreateOutcome::SourceGone);
        }
        let existing = tx
            .query_row(
                &format!("SELECT {COLUMNS} FROM followups WHERE source_memory_id = ?1"),
                params![src],
                row_to_followup,
            )
            .optional()?
            .map(finish)
            .transpose()?;
        if let Some(existing) = existing {
            return Ok(CreateOutcome::Existing(existing));
        }
    }

    let id = uuid::Uuid::new_v4().to_string();
    let (status, action) = if new.authoritative {
        (Status::Open, "open")
    } else {
        (Status::Proposed, "propose")
    };
    let ts = now.to_rfc3339();
    tx.execute(
        "INSERT INTO followups
            (id, project, project_key, title, status, source_memory_id, revision,
             created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, 1, ?7, ?7)",
        params![
            id,
            project,
            project_key(&project),
            title,
            status.as_str(),
            new.source_memory_id,
            ts
        ],
    )?;
    tx.execute(
        "INSERT INTO followup_events
            (followup_id, action, evidence_memory_id, actor, occurred_at, request_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            action,
            new.source_memory_id,
            new.actor,
            ts,
            new.request_id
        ],
    )?;
    let created = by_id(&tx, &id)?.expect("row just inserted");
    tx.commit()?;
    Ok(CreateOutcome::Created(created))
}

/// Normalize a title for STORAGE: first non-empty line, whitespace collapsed,
/// capped at the storage limit. Deliberately not `clean_line`, which truncates
/// to the Journal's short display width — CLI/MCP follow-ups have no source
/// memory, so anything cut here would be unrecoverable (review point).
/// Rendering surfaces shorten with `clean_line` themselves.
fn normalize_title(raw: &str) -> String {
    let line = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
    line.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(MAX_TITLE_CHARS)
        .collect()
}

/// Apply one state transition. Rejects an unknown id, a project mismatch, a
/// stale `expected_revision`, and any move the state machine doesn't allow.
/// The revision guard is part of the UPDATE itself, so two processes sharing
/// the database (several MCP servers + the daemon) can't both win a race.
pub fn transition(storage: &Storage, t: Transition<'_>, now: DateTime<Utc>) -> Result<Followup> {
    if t.request_id.trim().is_empty() {
        bail!("follow-up transition needs a request id");
    }
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let tx = write_tx(&conn)?;
    if let Some(existing) = by_request(&tx, t.request_id)? {
        return Ok(existing);
    }
    let Some(current) = by_id(&tx, t.id)? else {
        bail!("no follow-up with id {}", t.id);
    };
    let guard_project = match t.project {
        Some(p) => Some(canonical_project(&tx, p)?),
        None => None,
    };
    if let Some(project) = guard_project.as_deref()
        && project_key(project) != project_key(&current.project)
    {
        bail!(
            "follow-up {} belongs to project {:?}, not {:?}",
            short_id(&current.id),
            current.project,
            project
        );
    }
    if let Some(expected) = t.expected_revision
        && expected != current.revision
    {
        bail!(
            "revision conflict on follow-up {}: expected {expected}, found {}",
            short_id(&current.id),
            current.revision
        );
    }
    let Some(next) = t.action.apply(current.status) else {
        bail!(
            "cannot {} a follow-up that is {}",
            t.action.as_str(),
            current.status.as_str()
        );
    };

    let ts = now.to_rfc3339();
    let changed = tx.execute(
        "UPDATE followups SET status = ?1, revision = revision + 1, updated_at = ?2
          WHERE id = ?3 AND revision = ?4",
        params![next.as_str(), ts, current.id, current.revision],
    )?;
    if changed != 1 {
        bail!(
            "revision conflict on follow-up {}: changed concurrently",
            short_id(&current.id)
        );
    }
    tx.execute(
        "INSERT INTO followup_events
            (followup_id, action, evidence_memory_id, actor, occurred_at, request_id)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            current.id,
            t.action.as_str(),
            t.evidence_memory_id,
            t.actor,
            ts,
            t.request_id
        ],
    )?;
    let updated = by_id(&tx, &current.id)?.expect("row just updated");
    tx.commit()?;
    Ok(updated)
}

fn query(storage: &Storage, sql: &str, args: &[&dyn rusqlite::ToSql]) -> Result<Vec<Followup>> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let mut stmt = conn.prepare(sql)?;
    let rows = stmt
        .query_map(args, row_to_followup)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(finish).collect()
}

/// Projects that have authoritative OPEN follow-ups, most recently touched
/// first. The digest selector uses this so confirmed work stays visible even
/// for a project with no recent memory activity (review point).
pub fn projects_with_open(storage: &Storage, limit: usize) -> Result<Vec<String>> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let mut stmt = conn.prepare(
        "SELECT project FROM followups WHERE status = 'open'
          GROUP BY project_key ORDER BY MAX(updated_at) DESC, project_key ASC LIMIT ?1",
    )?;
    let names: Vec<String> = stmt
        .query_map(params![limit as i64], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    // Resolve to the graph's real project name so the digest's memory pool
    // (looked up by entity name) and the follow-ups line up on one page.
    names.iter().map(|n| canonical_project(&conn, n)).collect()
}

/// Authoritative open work for a project, most recently touched first.
/// Queried directly — NOT limited to whatever memory pool a digest loaded, so
/// an old open item survives any flood of newer notes.
pub fn open_for_project(storage: &Storage, project: &str, limit: usize) -> Result<Vec<Followup>> {
    let project = &project_key(&resolve_project(storage, project)?);
    query(
        storage,
        &format!(
            "SELECT {COLUMNS} FROM followups
              WHERE project_key = ?1 AND status = 'open'
              ORDER BY updated_at DESC, id ASC LIMIT ?2"
        ),
        &[&project, &(limit as i64)],
    )
}

/// Authoritative open work for every project EXCEPT `shown`, most recently
/// touched first. The context renders at most a few project digests; an item
/// someone explicitly recorded must still reach the agent when its project did
/// not win a slot, so the context lists these separately.
pub fn open_outside(storage: &Storage, shown: &[String], limit: usize) -> Result<Vec<Followup>> {
    let mut keys = Vec::with_capacity(shown.len());
    for name in shown {
        keys.push(project_key(&resolve_project(storage, name)?));
    }
    let exclude = if keys.is_empty() {
        String::new()
    } else {
        let marks: Vec<String> = (0..keys.len()).map(|i| format!("?{}", i + 2)).collect();
        format!("AND project_key NOT IN ({})", marks.join(", "))
    };
    let limit = limit as i64;
    let mut args: Vec<&dyn rusqlite::ToSql> = vec![&limit];
    args.extend(keys.iter().map(|k| k as &dyn rusqlite::ToSql));
    query(
        storage,
        &format!(
            "SELECT {COLUMNS} FROM followups
              WHERE status = 'open' {exclude}
              ORDER BY updated_at DESC, id ASC LIMIT ?1"
        ),
        &args,
    )
}

/// Keyword proposals young enough to still be worth showing (as unconfirmed).
pub fn fresh_proposals(
    storage: &Storage,
    project: &str,
    now: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<Followup>> {
    let project = &project_key(&resolve_project(storage, project)?);
    let cutoff = (now - chrono::Duration::days(PROPOSAL_MAX_AGE_DAYS)).to_rfc3339();
    query(
        storage,
        &format!(
            "SELECT {COLUMNS} FROM followups
              WHERE project_key = ?1 AND status = 'proposed' AND created_at >= ?2
              ORDER BY created_at DESC, id ASC LIMIT ?3"
        ),
        &[&project, &cutoff, &(limit as i64)],
    )
}

/// One page of follow-ups plus whether more remain. Callers that enumerate
/// (the MCP tool, the CLI) must be able to reach EVERY item: an agent with no
/// saved id has no other way to find an old item to confirm, close or dismiss
/// it (review point). Ordering is stable (`updated_at DESC, id ASC`), so
/// walking `offset` forward visits each row once while nothing changes.
pub struct Page {
    pub items: Vec<Followup>,
    pub has_more: bool,
}

pub fn list_page(
    storage: &Storage,
    project: Option<&str>,
    include_done: bool,
    limit: usize,
    offset: usize,
) -> Result<Page> {
    let status_clause = if include_done {
        ""
    } else {
        "AND status IN ('proposed', 'open')"
    };
    let resolved = match project {
        Some(p) => Some(project_key(&resolve_project(storage, p)?)),
        None => None,
    };
    // Fetch one extra row: its presence is the has_more signal.
    let (fetch, off) = ((limit as i64) + 1, offset as i64);
    let mut items = match resolved.as_deref() {
        Some(p) => query(
            storage,
            &format!(
                "SELECT {COLUMNS} FROM followups WHERE project_key = ?1 {status_clause}
                  ORDER BY updated_at DESC, id ASC LIMIT ?2 OFFSET ?3"
            ),
            &[&p, &fetch, &off],
        )?,
        None => query(
            storage,
            &format!(
                "SELECT {COLUMNS} FROM followups WHERE 1 = 1 {status_clause}
                  ORDER BY updated_at DESC, id ASC LIMIT ?1 OFFSET ?2"
            ),
            &[&fetch, &off],
        )?,
    };
    let has_more = items.len() > limit;
    items.truncate(limit);
    Ok(Page { items, has_more })
}

/// First page only. Test convenience: production callers page explicitly so
/// they can never silently truncate a backlog.
#[cfg(test)]
pub fn list(
    storage: &Storage,
    project: Option<&str>,
    include_done: bool,
    limit: usize,
) -> Result<Vec<Followup>> {
    Ok(list_page(storage, project, include_done, limit, 0)?.items)
}

/// Resolve a full id or an unambiguous prefix (min 8 chars, hex and dashes
/// only — the same contract as `session show`). Ambiguity is an error, never
/// a silent first match: closing the wrong obligation is the failure to avoid.
pub fn resolve_id(storage: &Storage, id_or_prefix: &str) -> Result<String> {
    let p = id_or_prefix.trim();
    if p.len() < 8 || !p.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
        bail!("follow-up id must be at least 8 hex characters, got {p:?}");
    }
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let mut stmt = conn.prepare("SELECT id FROM followups WHERE id LIKE ?1 LIMIT 3")?;
    let ids: Vec<String> = stmt
        .query_map(params![format!("{p}%")], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    match ids.len() {
        0 => bail!("no follow-up with id {p}"),
        1 => Ok(ids.into_iter().next().expect("one id")),
        _ => bail!("follow-up id prefix {p} is ambiguous"),
    }
}

/// Follow-ups that first appeared within `[start, end)`, with their status
/// AS OF `end`, keeping only the ones still proposed or open at that moment.
/// The Journal uses this so closing an item next week never rewrites what
/// last week's page said was open.
pub fn created_in_window_as_of(
    storage: &Storage,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    limit: usize,
) -> Result<Vec<Followup>> {
    let (start_s, end_s) = (start.to_rfc3339(), end.to_rfc3339());
    let rows = query(
        storage,
        &format!(
            "SELECT {COLUMNS} FROM followups
              WHERE created_at >= ?1 AND created_at < ?2
              ORDER BY created_at ASC, id ASC"
        ),
        &[&start_s, &end_s],
    )?;
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let mut out = Vec::new();
    for mut f in rows {
        // Replay this item's history up to `end`.
        let mut stmt = conn.prepare(
            "SELECT action FROM followup_events
              WHERE followup_id = ?1 AND occurred_at < ?2 ORDER BY id ASC",
        )?;
        let actions: Vec<String> = stmt
            .query_map(params![f.id, end_s], |r| r.get(0))?
            .collect::<rusqlite::Result<_>>()?;
        let mut status: Option<Status> = None;
        for a in &actions {
            status = match (a.as_str(), status) {
                ("propose", None) => Some(Status::Proposed),
                ("open", _) | ("reopen", _) => Some(Status::Open),
                ("close", _) => Some(Status::Closed),
                ("dismiss", _) => Some(Status::Dismissed),
                (_, s) => s,
            };
        }
        if let Some(s @ (Status::Proposed | Status::Open)) = status {
            f.status = s;
            out.push(f);
            if out.len() >= limit {
                break;
            }
        }
    }
    Ok(out)
}

/// True when a note headline reads like a forward-looking intention. This is
/// the ONLY place keywords are consulted, and it only ever yields proposals.
/// Guards inherited from the Journal/digest reviews: corrections and
/// conversation chatter are meta, not work; a Russian "не надо X" ("do not do
/// X") contains the "надо " marker; decisions are things done; only the
/// cleaned first line is scanned.
pub(crate) fn looks_like_followup(title: &str, memory_type: &str, tags: &str) -> bool {
    if memory_type != "note" || is_meta_memory(memory_type, tags) || is_noise_title(title) {
        return false;
    }
    let headline = clean_line(title).to_lowercase();
    !headline.contains("не надо") && FOLLOWUP_MARKERS.iter().any(|w| headline.contains(w))
}

/// Propose follow-ups from recent project-linked notes. Deterministic and
/// idempotent: the request id is derived from the source memory, and a memory
/// that already has a follow-up (in ANY status) is skipped by the query.
/// Returns how many proposals were created.
///
/// The scan PAGES through the whole proposal window with a keyset cursor.
/// A single `LIMIT` would re-select the same newest notes every run, reject
/// them, and never reach an eligible TODO sitting behind them before it ages
/// out (review point). The window is bounded by `PROPOSAL_MAX_AGE_DAYS`, and
/// `SWEEP_MAX_PAGES` is a runaway backstop, not a work planner.
pub fn sweep(storage: &Storage, now: DateTime<Utc>) -> Result<usize> {
    let cutoff = (now - chrono::Duration::days(PROPOSAL_MAX_AGE_DAYS)).to_rfc3339();
    let mut cursor: (String, String) = ("9999".into(), String::new());
    let mut created = 0;
    // A commit subject describes work that is DONE. "Page the follow-up
    // listing" carries a marker yet is the opposite of an open intention, so
    // commit notes never propose. `source` is stored as its JSON form.
    let commit_source = serde_json::to_string(&crate::event::EventSource::GitWatcher)?;

    for _ in 0..SWEEP_MAX_PAGES {
        let page: Vec<(String, String, String, String, String)> = {
            let conn = storage
                .conn
                .lock()
                .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
            let mut stmt = conn.prepare(
                "SELECT m.id, m.timestamp, m.title, m.memory_type, m.tags
                   FROM memories m
                  WHERE m.superseded_by IS NULL
                    AND m.memory_type = 'note'
                    AND m.source != ?5
                    AND m.timestamp >= ?1
                    AND (m.timestamp < ?2 OR (m.timestamp = ?2 AND m.id < ?3))
                    AND NOT EXISTS
                        (SELECT 1 FROM followups f WHERE f.source_memory_id = m.id)
                  ORDER BY m.timestamp DESC, m.id DESC LIMIT ?4",
            )?;
            stmt.query_map(
                params![cutoff, cursor.0, cursor.1, SWEEP_BATCH, commit_source],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )?
            .collect::<rusqlite::Result<_>>()?
        };
        let Some(last) = page.last() else { break };
        cursor = (last.1.clone(), last.0.clone());
        let page_len = page.len();

        for (memory_id, _ts, title, memory_type, tags) in page {
            if !looks_like_followup(&title, &memory_type, &tags) {
                continue;
            }
            // A note linked to several projects goes where its HEADLINE points
            // (the Journal's chooser); an ambiguous one is deferred rather
            // than permanently filed under the most-mentioned project.
            // Consolidation gives the surviving note a NEW id, so the
            // exact-id exclusion above cannot see that one of its sources was
            // already tracked (review point): without this, closing an item
            // and then running `reflect --apply` brings it back as a proposal.
            if consolidated_from_tracked(storage, &memory_id)?
                || headline_comes_from_commit(storage, &memory_id, &title, &commit_source)?
            {
                continue;
            }
            let candidates = linked_projects(storage, &memory_id)?;
            let Some((_, project)) = crate::journal::primary_project_for_title(&title, &candidates)
            else {
                continue;
            };
            let outcome = create_outcome(
                storage,
                NewFollowup {
                    project: &project,
                    title: &title,
                    source_memory_id: Some(&memory_id),
                    authoritative: false,
                    actor: "heuristic",
                    request_id: &format!("sweep:{memory_id}"),
                },
                now,
            )?;
            if matches!(outcome, CreateOutcome::Created(_)) {
                created += 1;
            }
        }
        if (page_len as i64) < SWEEP_BATCH {
            break;
        }
    }
    Ok(created)
}

/// Whether any memory this one was consolidated from (at any depth: a
/// canonical can itself be folded into a later canonical) already carries a
/// follow-up, in ANY status. A proposed or open one would be duplicated; a
/// closed or dismissed one would be resurrected.
///
/// The live `source_memory_id` link is not enough: forgetting the source
/// clears it (the deletion trigger), and the finished item would come back
/// through its canonical (review point). A follow-up's FIRST event records the
/// source as evidence and has no foreign key to `memories`, so it outlives the
/// source; both are consulted (rows migrated from an older table may have a
/// link but no creation event).
fn consolidated_from_tracked(storage: &Storage, memory_id: &str) -> Result<bool> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    // UNION (not UNION ALL) dedupes visited ids, so a provenance cycle in a
    // damaged database still terminates.
    let tracked: bool = conn.query_row(
        "WITH RECURSIVE ancestors(id) AS (
             SELECT source_id FROM reflection_sources WHERE canonical_id = ?1
             UNION
             SELECT rs.source_id FROM reflection_sources rs
               JOIN ancestors a ON rs.canonical_id = a.id
         )
         SELECT EXISTS (
             SELECT 1 FROM ancestors a
              WHERE EXISTS (SELECT 1 FROM followups f WHERE f.source_memory_id = a.id)
                 OR EXISTS (SELECT 1 FROM followup_events e
                             WHERE e.evidence_memory_id = a.id
                               AND e.id = (SELECT MIN(e2.id) FROM followup_events e2
                                            WHERE e2.followup_id = e.followup_id)))",
        params![memory_id],
        |r| r.get(0),
    )?;
    Ok(tracked)
}

/// Whether a consolidated note's headline describes a commit. Consolidation
/// re-emits the cluster under a fresh `Manual` note that keeps a source title
/// (the rule synthesizer picks the longest), so the `source` filter in the
/// sweep query no longer recognises finished work (review point). Walks the
/// same provenance as [`consolidated_from_tracked`]:
/// - an ancestor commit carrying this very headline: yes;
/// - otherwise a non-commit leaf carrying it: no, a person wrote it;
/// - otherwise the headline was synthesized: yes only when every surviving
///   leaf is a commit. Forgotten sources are unknown and do not vote.
fn headline_comes_from_commit(
    storage: &Storage,
    memory_id: &str,
    title: &str,
    commit_source: &str,
) -> Result<bool> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let mut stmt = conn.prepare(
        "WITH RECURSIVE ancestors(id) AS (
             SELECT source_id FROM reflection_sources WHERE canonical_id = ?1
             UNION
             SELECT rs.source_id FROM reflection_sources rs
               JOIN ancestors a ON rs.canonical_id = a.id
         )
         SELECT m.title, m.source,
                NOT EXISTS (SELECT 1 FROM reflection_sources r WHERE r.canonical_id = m.id)
           FROM ancestors a JOIN memories m ON m.id = a.id",
    )?;
    let rows: Vec<(String, String, bool)> = stmt
        .query_map(params![memory_id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })?
        .collect::<rusqlite::Result<_>>()?;

    let norm = |t: &str| clean_line(t).to_lowercase();
    let headline = norm(title);
    let (mut person_wrote_it, mut leaves, mut commit_leaves) = (false, 0usize, 0usize);
    for (ancestor_title, source, leaf) in &rows {
        let is_commit = source == commit_source;
        if norm(ancestor_title) == headline {
            if is_commit {
                return Ok(true);
            }
            person_wrote_it |= *leaf;
        }
        if *leaf {
            leaves += 1;
            commit_leaves += usize::from(is_commit);
        }
    }
    Ok(!person_wrote_it && leaves > 0 && leaves == commit_leaves)
}

/// `(entity_id, name)` of every project entity linked to a memory.
fn linked_projects(storage: &Storage, memory_id: &str) -> Result<Vec<(String, String)>> {
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let mut stmt = conn.prepare(
        "SELECT e.id, e.name FROM memory_entities me
           JOIN entities e ON e.id = me.entity_id
          WHERE me.memory_id = ?1 AND e.entity_type = 'project'
          ORDER BY e.name ASC",
    )?;
    Ok(stmt
        .query_map(params![memory_id], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn short_id(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryEntry, MemoryType};
    use crate::graph::{Entity, EntityType};

    fn tmp_storage() -> crate::test_support::InTempDir<Storage> {
        crate::test_support::InTempDir::new("mn-fu-", |dir| {
            Storage::open(&dir.join("memory.db")).unwrap()
        })
    }

    fn project(storage: &Storage, name: &str) -> String {
        storage
            .upsert_entity(&Entity {
                name: name.into(),
                entity_type: EntityType::Project,
            })
            .unwrap()
    }

    fn linked(storage: &Storage, eid: &str, title: &str, mt: MemoryType) -> String {
        let e = MemoryEntry::new(title, "body", mt, EventSource::Socket);
        storage.save(&e).unwrap();
        storage.link_memory_entity(&e.id, eid).unwrap();
        e.id.clone()
    }

    fn open_item(storage: &Storage, project: &str, title: &str, req: &str) -> Followup {
        create(
            storage,
            NewFollowup {
                project,
                title,
                source_memory_id: None,
                authoritative: true,
                actor: "test",
                request_id: req,
            },
            Utc::now(),
        )
        .unwrap()
    }

    fn step(storage: &Storage, id: &str, action: Action, req: &str) -> Result<Followup> {
        transition(
            storage,
            Transition {
                id,
                action,
                expected_revision: None,
                project: None,
                evidence_memory_id: None,
                actor: "test",
                request_id: req,
            },
            Utc::now(),
        )
    }

    fn actions(storage: &Storage, id: &str) -> Vec<String> {
        let conn = storage.conn.lock().unwrap();
        let mut stmt = conn
            .prepare("SELECT action FROM followup_events WHERE followup_id = ?1 ORDER BY id")
            .unwrap();
        stmt.query_map(params![id], |r| r.get(0))
            .unwrap()
            .collect::<rusqlite::Result<_>>()
            .unwrap()
    }

    #[test]
    fn create_close_reopen_keeps_full_history() {
        let s = tmp_storage();
        let f = open_item(&s, "demoapp", "Ship the importer", "r1");
        assert_eq!(f.status, Status::Open);
        assert_eq!(f.revision, 1);

        let closed = step(&s, &f.id, Action::Close, "r2").unwrap();
        assert_eq!(closed.status, Status::Closed);
        assert_eq!(closed.revision, 2);

        let reopened = step(&s, &f.id, Action::Reopen, "r3").unwrap();
        assert_eq!(reopened.status, Status::Open);
        assert_eq!(reopened.revision, 3);

        assert_eq!(actions(&s, &f.id), vec!["open", "close", "reopen"]);
    }

    #[test]
    fn duplicate_request_id_is_a_noop() {
        let s = tmp_storage();
        let a = open_item(&s, "demoapp", "Ship the importer", "same");
        let b = open_item(&s, "demoapp", "A different title entirely", "same");
        assert_eq!(a.id, b.id, "retried create returns the original row");
        assert_eq!(list(&s, None, true, 10).unwrap().len(), 1);

        step(&s, &a.id, Action::Close, "close-1").unwrap();
        let again = step(&s, &a.id, Action::Close, "close-1").unwrap();
        assert_eq!(again.status, Status::Closed);
        assert_eq!(again.revision, 2, "retry must not bump the revision");
        assert_eq!(actions(&s, &a.id), vec!["open", "close"]);
    }

    #[test]
    fn competing_revisions_are_rejected() {
        let s = tmp_storage();
        let f = open_item(&s, "demoapp", "Ship the importer", "r1");
        let attempt = |rev: i64, req: &str| {
            transition(
                &s,
                Transition {
                    id: &f.id,
                    action: Action::Close,
                    expected_revision: Some(rev),
                    project: None,
                    evidence_memory_id: None,
                    actor: "test",
                    request_id: req,
                },
                Utc::now(),
            )
        };
        // Two writers both read revision 1; only the first may win.
        assert!(attempt(1, "writer-a").is_ok());
        let err = attempt(1, "writer-b").unwrap_err().to_string();
        assert!(err.contains("revision conflict"), "got: {err}");
        assert_eq!(actions(&s, &f.id), vec!["open", "close"]);
    }

    #[test]
    fn wrong_project_and_invalid_moves_are_rejected() {
        let s = tmp_storage();
        let f = open_item(&s, "demoapp", "Ship the importer", "r1");
        let err = transition(
            &s,
            Transition {
                id: &f.id,
                action: Action::Close,
                expected_revision: None,
                project: Some("otherapp"),
                evidence_memory_id: None,
                actor: "test",
                request_id: "r2",
            },
            Utc::now(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("belongs to project"), "got: {err}");

        // open -> reopen and open -> confirm are not moves.
        assert!(step(&s, &f.id, Action::Reopen, "r3").is_err());
        assert!(step(&s, &f.id, Action::Open, "r4").is_err());
        assert!(step(&s, "00000000-dead", Action::Close, "r5").is_err());
        assert_eq!(
            actions(&s, &f.id),
            vec!["open"],
            "rejections write no event"
        );
    }

    #[test]
    fn sweep_only_proposes_and_respects_the_guards() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        let wanted = linked(&s, &eid, "TODO: wire the importer retry", MemoryType::Note);
        // A prohibition ("не надо X") carries the "надо " marker.
        linked(&s, &eid, "не надо трогать импортёр", MemoryType::Note);
        // Decisions and corrections are never work items.
        linked(
            &s,
            &eid,
            "TODO decision about storage",
            MemoryType::Decision,
        );
        linked(&s, &eid, "надо было иначе", MemoryType::Feedback);
        // A marker note with no project link has nowhere to live.
        let orphan = MemoryEntry::new(
            "todo: unlinked idea",
            "body",
            MemoryType::Note,
            EventSource::Socket,
        );
        s.save(&orphan).unwrap();

        assert_eq!(sweep(&s, Utc::now()).unwrap(), 1);
        let rows = list(&s, None, true, 10).unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].status, Status::Proposed, "keywords never open");
        assert_eq!(rows[0].source_memory_id.as_deref(), Some(wanted.as_str()));
        assert_eq!(rows[0].project, "demoapp");

        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0, "sweep is idempotent");
    }

    #[test]
    fn resweep_never_resurrects_closed_or_dismissed() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        linked(&s, &eid, "TODO: wire the importer retry", MemoryType::Note);
        linked(&s, &eid, "follow-up: rotate the token", MemoryType::Note);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 2);
        let rows = list(&s, None, false, 10).unwrap();
        step(&s, &rows[0].id, Action::Close, "c1").unwrap();
        step(&s, &rows[1].id, Action::Dismiss, "d1").unwrap();

        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0);
        assert!(
            list(&s, None, false, 10).unwrap().is_empty(),
            "closed/dismissed items must stay that way"
        );
        assert_eq!(list(&s, None, true, 10).unwrap().len(), 2);
    }

    /// Consolidate `sources` into a fresh canonical note carrying `title`,
    /// the way `reflect --apply` does (new id, inherited project links).
    fn consolidate(storage: &Storage, title: &str, sources: &[&str]) -> String {
        let run_id = storage.begin_reflection_run("apply", 0.9, "rule").unwrap();
        let canonical = MemoryEntry::new(title, "body", MemoryType::Note, EventSource::Socket);
        let cluster: Vec<(String, f32)> = sources.iter().map(|id| (id.to_string(), 0.99)).collect();
        storage
            .apply_reflection(&run_id, &canonical, None, &cluster)
            .unwrap()
            .unwrap()
    }

    #[test]
    fn consolidation_never_resurrects_a_finished_followup() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        let done = linked(&s, &eid, "TODO: wire the importer retry", MemoryType::Note);
        let twin = linked(&s, &eid, "TODO: wire importer retries", MemoryType::Note);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 2);
        for f in list(&s, None, false, 10).unwrap() {
            let action = if f.source_memory_id.as_deref() == Some(done.as_str()) {
                Action::Close
            } else {
                Action::Dismiss
            };
            step(&s, &f.id, action, &format!("fin-{}", f.id)).unwrap();
        }

        // The canonical keeps the TODO headline under a brand-new id.
        let c1 = consolidate(&s, "TODO: wire the importer retry", &[&done, &twin]);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0, "provenance is consulted");

        // Folding that canonical into a later one must not lose the trail.
        let other = linked(&s, &eid, "importer retry notes", MemoryType::Note);
        consolidate(&s, "TODO: wire the importer retry", &[&c1, &other]);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0, "depth two as well");

        assert!(list(&s, None, false, 10).unwrap().is_empty());
        assert_eq!(list(&s, None, true, 10).unwrap().len(), 2);
    }

    #[test]
    fn forgetting_the_source_does_not_reopen_the_trail() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        let done = linked(&s, &eid, "TODO: wire the importer retry", MemoryType::Note);
        let twin = linked(&s, &eid, "importer retry notes", MemoryType::Note);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 1);
        let f = list(&s, None, false, 10).unwrap().remove(0);
        step(&s, &f.id, Action::Close, "c1").unwrap();

        consolidate(&s, "TODO: wire the importer retry", &[&done, &twin]);
        // Forgetting the source clears the live link on the closed row...
        assert!(s.forget_by_id(&done).unwrap());
        let closed = list(&s, None, true, 10).unwrap().remove(0);
        assert_eq!(closed.source_memory_id, None);
        // ...but the creation event still names it, so nothing comes back.
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0);
        assert!(list(&s, None, false, 10).unwrap().is_empty());
    }

    #[test]
    fn commit_notes_never_propose() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        let commit = MemoryEntry::new(
            "Page the follow-up listing",
            "body",
            MemoryType::Note,
            EventSource::GitWatcher,
        );
        s.save(&commit).unwrap();
        s.link_memory_entity(&commit.id, &eid).unwrap();
        assert_eq!(
            sweep(&s, Utc::now()).unwrap(),
            0,
            "a commit is finished work"
        );
        // The same headline typed by a person is still a candidate.
        linked(&s, &eid, "follow-up: page the listing", MemoryType::Note);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 1);
    }

    fn linked_commit(storage: &Storage, eid: &str, subject: &str) -> String {
        let e = MemoryEntry::new(subject, "body", MemoryType::Note, EventSource::GitWatcher);
        storage.save(&e).unwrap();
        storage.link_memory_entity(&e.id, eid).unwrap();
        e.id.clone()
    }

    #[test]
    fn consolidated_commits_never_propose() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        let a = linked_commit(&s, &eid, "Page the follow-up listing");
        let b = linked_commit(&s, &eid, "Page follow-up list");
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0);

        // The canonical keeps a commit subject under a non-commit source.
        let c1 = consolidate(&s, "Page the follow-up listing", &[&a, &b]);
        assert_eq!(
            sweep(&s, Utc::now()).unwrap(),
            0,
            "headline is a commit subject"
        );

        // Folded again, the subject is still a commit's.
        let other = linked(&s, &eid, "listing pagination notes", MemoryType::Note);
        consolidate(&s, "Page the follow-up listing", &[&c1, &other]);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0, "depth two as well");

        // A headline synthesized from nothing but commits is still finished work.
        let x = linked_commit(&s, &eid, "Add retry to importer");
        let y = linked_commit(&s, &eid, "Importer retries on timeout");
        consolidate(&s, "TODO importer retry work", &[&x, &y]);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0, "all-commit cluster");
        assert!(list(&s, None, true, 10).unwrap().is_empty());
    }

    #[test]
    fn a_persons_headline_survives_a_commit_in_the_cluster() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        // Out of the sweep window, so the note itself was never tracked.
        let note = linked(&s, &eid, "TODO: add importer backoff", MemoryType::Note);
        {
            let conn = s.conn.lock().unwrap();
            conn.execute(
                "UPDATE memories SET timestamp = '2023-01-01T00:00:00+00:00' WHERE id = ?1",
                params![note],
            )
            .unwrap();
        }
        let commit = linked_commit(&s, &eid, "Add retry to importer");
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0);

        let c = consolidate(&s, "TODO: add importer backoff", &[&note, &commit]);
        assert_eq!(
            sweep(&s, Utc::now()).unwrap(),
            1,
            "a person wrote this headline"
        );
        let rows = list(&s, None, false, 10).unwrap();
        assert_eq!(rows[0].source_memory_id.as_deref(), Some(c.as_str()));
    }

    #[test]
    fn consolidating_untracked_notes_still_proposes() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        // Neither source looks like a follow-up, so nothing tracks them; the
        // synthesized headline does, and must not be blocked by provenance.
        let a = linked(&s, &eid, "importer drops rows on timeout", MemoryType::Note);
        let b = linked(&s, &eid, "importer loses rows when slow", MemoryType::Note);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 0);
        let c = consolidate(&s, "TODO: make the importer retry", &[&a, &b]);
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 1);
        let rows = list(&s, None, false, 10).unwrap();
        assert_eq!(rows[0].source_memory_id.as_deref(), Some(c.as_str()));
    }

    #[test]
    fn completion_prose_closes_nothing() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        let f = open_item(&s, "demoapp", "Wire the importer retry", "r1");
        // A later memory claims the work is done — in words only.
        linked(
            &s,
            &eid,
            "Доделал importer retry, всё готово",
            MemoryType::Note,
        );
        linked(&s, &eid, "done: importer retry finished", MemoryType::Note);
        sweep(&s, Utc::now()).unwrap();

        let open = open_for_project(&s, "demoapp", 10).unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].id, f.id, "only an ID-linked action may close");
    }

    #[test]
    fn source_deletion_kills_proposals_but_not_authoritative_rows() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        let guess_src = linked(&s, &eid, "TODO: maybe refactor", MemoryType::Note);
        let real_src = linked(&s, &eid, "TODO: ship the importer", MemoryType::Note);
        sweep(&s, Utc::now()).unwrap();
        let real = list(&s, None, false, 10)
            .unwrap()
            .into_iter()
            .find(|f| f.source_memory_id.as_deref() == Some(real_src.as_str()))
            .unwrap();
        step(&s, &real.id, Action::Open, "confirm").unwrap();

        assert!(s.forget_by_id(&guess_src).unwrap());
        assert!(s.forget_by_id(&real_src).unwrap());

        let left = list(&s, None, true, 10).unwrap();
        assert_eq!(left.len(), 1, "the unconfirmed guess died with its source");
        assert_eq!(left[0].id, real.id);
        assert_eq!(left[0].status, Status::Open);
        assert!(left[0].source_memory_id.is_none(), "link cleared, row kept");
        // The dead proposal's history went with it (FK cascade).
        let conn = s.conn.lock().unwrap();
        let orphans: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM followup_events
                  WHERE followup_id NOT IN (SELECT id FROM followups)",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(orphans, 0);
    }

    #[test]
    fn stale_proposals_never_render() {
        let s = tmp_storage();
        let now = Utc::now();
        let old = now - chrono::Duration::days(PROPOSAL_MAX_AGE_DAYS + 1);
        create(
            &s,
            NewFollowup {
                project: "demoapp",
                title: "An intention from weeks ago",
                source_memory_id: None,
                authoritative: false,
                actor: "heuristic",
                request_id: "old",
            },
            old,
        )
        .unwrap();
        create(
            &s,
            NewFollowup {
                project: "demoapp",
                title: "A fresh intention",
                source_memory_id: None,
                authoritative: false,
                actor: "heuristic",
                request_id: "new",
            },
            now,
        )
        .unwrap();
        let shown = fresh_proposals(&s, "demoapp", now, 10).unwrap();
        assert_eq!(shown.len(), 1);
        assert_eq!(shown[0].title, "A fresh intention");
    }

    #[test]
    fn journal_window_reports_state_as_of_the_day_end() {
        let s = tmp_storage();
        let day_start = Utc::now() - chrono::Duration::days(3);
        let day_end = day_start + chrono::Duration::days(1);
        let mk = |title: &str, req: &str| {
            create(
                &s,
                NewFollowup {
                    project: "demoapp",
                    title,
                    source_memory_id: None,
                    authoritative: true,
                    actor: "test",
                    request_id: req,
                },
                day_start + chrono::Duration::hours(2),
            )
            .unwrap()
        };
        let closed_same_day = mk("Closed the same day", "a");
        let closed_later = mk("Closed a day later", "b");
        let close_at = |id: &str, req: &str, at: DateTime<Utc>| {
            transition(
                &s,
                Transition {
                    id,
                    action: Action::Close,
                    expected_revision: None,
                    project: None,
                    evidence_memory_id: None,
                    actor: "test",
                    request_id: req,
                },
                at,
            )
            .unwrap()
        };
        close_at(
            &closed_same_day.id,
            "c-a",
            day_start + chrono::Duration::hours(5),
        );
        close_at(
            &closed_later.id,
            "c-b",
            day_end + chrono::Duration::hours(5),
        );

        let seen = created_in_window_as_of(&s, day_start, day_end, 10).unwrap();
        let titles: Vec<&str> = seen.iter().map(|f| f.title.as_str()).collect();
        assert_eq!(
            titles,
            vec!["Closed a day later"],
            "that day's page still shows what was open when the day ended"
        );
        assert_eq!(seen[0].status, Status::Open);
    }

    /// An eligible TODO sitting behind more than one page of newer ordinary
    /// notes must still be reached (review point: batch-limit starvation).
    #[test]
    fn sweep_pages_past_rejected_candidates() {
        let s = tmp_storage();
        let eid = project(&s, "demoapp");
        let buried = linked(&s, &eid, "TODO: the buried importer fix", MemoryType::Note);
        {
            let conn = s.conn.lock().unwrap();
            let older = (Utc::now() - chrono::Duration::days(2)).to_rfc3339();
            conn.execute(
                "UPDATE memories SET timestamp = ?2 WHERE id = ?1",
                params![buried, older],
            )
            .unwrap();
        }
        for i in 0..(SWEEP_BATCH as usize + 25) {
            linked(
                &s,
                &eid,
                &format!("ordinary working note {i}"),
                MemoryType::Note,
            );
        }
        assert_eq!(sweep(&s, Utc::now()).unwrap(), 1);
        let rows = list(&s, None, false, 10).unwrap();
        assert_eq!(rows[0].source_memory_id.as_deref(), Some(buried.as_str()));
    }

    /// A note linked to two projects is filed under the one its headline
    /// names, even when the other project is mentioned far more globally;
    /// a headline naming neither is deferred, not guessed.
    #[test]
    fn multi_project_note_follows_its_headline() {
        let s = tmp_storage();
        let alpha = project(&s, "alpha");
        let beta = project(&s, "beta");
        {
            let conn = s.conn.lock().unwrap();
            conn.execute(
                "UPDATE entities SET mention_count = 500 WHERE name = 'beta'",
                [],
            )
            .unwrap();
        }
        let named = linked(&s, &alpha, "TODO: finish alpha importer", MemoryType::Note);
        s.link_memory_entity(&named, &beta).unwrap();
        let vague = linked(&s, &alpha, "TODO: tidy the shared thing", MemoryType::Note);
        s.link_memory_entity(&vague, &beta).unwrap();

        assert_eq!(
            sweep(&s, Utc::now()).unwrap(),
            1,
            "the ambiguous one is deferred"
        );
        let rows = list(&s, None, false, 10).unwrap();
        assert_eq!(rows[0].project, "alpha");
        assert_eq!(rows[0].source_memory_id.as_deref(), Some(named.as_str()));
    }

    /// A proposal must never be inserted for a source that was forgotten
    /// between candidate selection and insertion.
    #[test]
    fn vanished_source_is_skipped_inside_the_transaction() {
        let s = tmp_storage();
        let outcome = create_outcome(
            &s,
            NewFollowup {
                project: "demoapp",
                title: "TODO: from a memory that is already gone",
                source_memory_id: Some("not-a-real-memory-id"),
                authoritative: false,
                actor: "heuristic",
                request_id: "sweep:not-a-real-memory-id",
            },
            Utc::now(),
        )
        .unwrap();
        assert!(matches!(outcome, CreateOutcome::SourceGone));
        assert!(list(&s, None, true, 10).unwrap().is_empty());
    }

    /// Follow-ups are keyed by project name, so graph canonicalization
    /// (rename and merge) must carry them to the canonical project.
    #[test]
    fn project_keys_follow_rename_and_merge() {
        let s = tmp_storage();
        project(&s, "demo-app");
        project(&s, "legacyname");
        project(&s, "canonical");
        let renamed = open_item(&s, "demo-app", "Ship the importer", "r1");
        let merged = open_item(&s, "legacyname", "Rotate the token", "r2");

        assert!(s.rename_entity("demo-app", "demoapp").unwrap());
        s.merge_entities("canonical", "legacyname").unwrap();

        let a = open_for_project(&s, "demoapp", 10).unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].id, renamed.id);
        let b = open_for_project(&s, "canonical", 10).unwrap();
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].id, merged.id);
        // A plain rename leaves no alias, so the old name finds nothing.
        assert!(open_for_project(&s, "demo-app", 10).unwrap().is_empty());
        // A merge registers the alias, so the old name resolves to the
        // canonical project instead of silently returning nothing. What must
        // NOT exist is a row still stored under the obsolete key.
        assert_eq!(open_for_project(&s, "legacyname", 10).unwrap().len(), 1);
        let conn = s.conn.lock().unwrap();
        let stale: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM followups
                  WHERE project_key IN ('demo-app', 'legacyname')",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(stale, 0);
    }

    /// After a merge the alias name is obsolete: work created under it must
    /// land on (and be found through) the canonical project.
    #[test]
    fn alias_names_resolve_to_the_canonical_project() {
        let s = tmp_storage();
        project(&s, "canonical");
        project(&s, "legacy");
        s.merge_entities("canonical", "legacy").unwrap();

        let f = open_item(&s, "legacy", "Created under the obsolete name", "r1");
        assert_eq!(f.project, "canonical");
        assert_eq!(open_for_project(&s, "canonical", 10).unwrap().len(), 1);
        assert_eq!(
            open_for_project(&s, "legacy", 10).unwrap().len(),
            1,
            "reads through the alias resolve too"
        );
        // The project guard accepts the alias because it names the same project.
        transition(
            &s,
            Transition {
                id: &f.id,
                action: Action::Close,
                expected_revision: None,
                project: Some("legacy"),
                evidence_memory_id: None,
                actor: "test",
                request_id: "close",
            },
            Utc::now(),
        )
        .unwrap();
    }

    /// Alias resolution is the graph's own, which is case-insensitive:
    /// "Legacy" must land exactly where "legacy" does, and a project named
    /// with different casing than its entity is stored under the real name.
    #[test]
    fn alias_resolution_is_case_insensitive() {
        let s = tmp_storage();
        project(&s, "canonical");
        project(&s, "legacy");
        s.merge_entities("canonical", "legacy").unwrap();
        let f = open_item(&s, "  Legacy ", "Created with odd casing", "r1");
        assert_eq!(f.project, "canonical");
        assert_eq!(open_for_project(&s, "LEGACY", 10).unwrap().len(), 1);

        project(&s, "demoapp");
        let g = open_item(&s, "DemoApp", "Cased differently than the entity", "r2");
        assert_eq!(g.project, "demoapp", "stored under the entity's real name");
        assert_eq!(open_for_project(&s, "demoapp", 10).unwrap().len(), 1);
    }

    /// A follow-up created BEFORE its graph entity exists keeps the caller's
    /// spelling. When extraction later creates the entity with a different
    /// casing, the row must still be found through the project and rendered
    /// in its digest — casing can never split one project's follow-ups.
    #[test]
    fn followup_created_before_its_entity_is_still_found() {
        let s = tmp_storage();
        let early = open_item(&s, "DemoApp", "Recorded before the graph knew it", "r1");
        assert_eq!(
            early.project, "DemoApp",
            "unknown project keeps its spelling"
        );

        // Extraction catches up and creates the entity in lowercase.
        project(&s, "demoapp");

        for name in ["demoapp", "DemoApp", "DEMOAPP"] {
            let found = open_for_project(&s, name, 10).unwrap();
            assert_eq!(found.len(), 1, "lookup via {name:?}");
            assert_eq!(found[0].id, early.id);
        }
        assert_eq!(list(&s, Some("demoapp"), false, 10).unwrap().len(), 1);
        assert_eq!(
            projects_with_open(&s, 10).unwrap(),
            vec!["demoapp".to_string()],
            "one project, resolved to the entity's real name"
        );
        // A later item under the entity's spelling joins the same project.
        open_item(&s, "demoapp", "Recorded after", "r2");
        assert_eq!(open_for_project(&s, "DemoApp", 10).unwrap().len(), 2);
        assert_eq!(projects_with_open(&s, 10).unwrap().len(), 1);
        // The project guard treats both spellings as the same project.
        transition(
            &s,
            Transition {
                id: &early.id,
                action: Action::Close,
                expected_revision: None,
                project: Some("demoapp"),
                evidence_memory_id: None,
                actor: "test",
                request_id: "close",
            },
            Utc::now(),
        )
        .unwrap();
    }

    /// The matching key is the GRAPH's canonical form: "Example Org" typed
    /// by a caller and the "example-org" entity extraction creates (with no
    /// alias registered) are one project.
    #[test]
    fn keys_use_the_graph_canonical_form() {
        let s = tmp_storage();
        let f = open_item(&s, "Example Org", "Draft the release notes", "r1");
        assert_eq!(f.project, "Example Org", "display spelling is kept");
        project(&s, "example-org");
        for name in ["example-org", "Example Org", "EXAMPLE_ORG"] {
            assert_eq!(
                open_for_project(&s, name, 10).unwrap().len(),
                1,
                "lookup via {name:?}"
            );
        }
        assert_eq!(
            projects_with_open(&s, 10).unwrap(),
            vec!["example-org".to_string()]
        );
        assert_eq!(
            project_key("  --  "),
            "--",
            "degenerate names never collapse to empty"
        );
    }

    /// A database created by the first follow-up commits has no project_key
    /// column and an index on `project`. Opening it must add and backfill the
    /// column and leave creates and lookups working.
    #[test]
    fn legacy_followups_table_is_migrated_on_open() {
        let path = crate::test_support::temp_path("mn-fu-mig-", "memory.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE followups (
                    id TEXT PRIMARY KEY,
                    project TEXT NOT NULL,
                    title TEXT NOT NULL,
                    status TEXT NOT NULL,
                    source_memory_id TEXT,
                    revision INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                 );
                 CREATE INDEX idx_followups_project_status ON followups(project, status);
                 INSERT INTO followups VALUES
                    ('11111111-aaaa', 'Example Org', 'Legacy open item', 'open', NULL, 1,
                     '2026-09-01T00:00:00+00:00', '2026-09-01T00:00:00+00:00');",
            )
            .unwrap();
        }
        let s = Storage::open(&path).unwrap();
        let found = open_for_project(&s, "example-org", 10).unwrap();
        assert_eq!(
            found.len(),
            1,
            "legacy row backfilled with the canonical key"
        );
        assert_eq!(found[0].title, "Legacy open item");
        open_item(&s, "example-org", "Created after the upgrade", "r1");
        assert_eq!(open_for_project(&s, "Example Org", 10).unwrap().len(), 2);
        {
            let conn = s.conn.lock().unwrap();
            let old_index: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM sqlite_master
                      WHERE type = 'index' AND name = 'idx_followups_project_status'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert_eq!(old_index, 0, "index on the old column is gone");
        }
        drop(s);
        // Idempotent: a second open changes nothing and still works.
        let again = Storage::open(&path).unwrap();
        assert_eq!(
            open_for_project(&again, "example-org", 10).unwrap().len(),
            2
        );
    }

    /// A key written by an EARLIER normalization scheme (plain lowercase,
    /// "example org") must be recomputed on open, not just empty keys.
    #[test]
    fn stale_keys_from_an_older_scheme_are_recomputed_on_open() {
        let path = crate::test_support::temp_path("mn-fu-rekey-", "memory.db");
        {
            let s = Storage::open(&path).unwrap();
            let f = open_item(&s, "Example Org", "Keyed by the old scheme", "r1");
            let conn = s.conn.lock().unwrap();
            conn.execute(
                "UPDATE followups SET project_key = 'example org' WHERE id = ?1",
                params![f.id],
            )
            .unwrap();
        }
        let s = Storage::open(&path).unwrap();
        for name in ["Example Org", "example-org"] {
            assert_eq!(
                open_for_project(&s, name, 10).unwrap().len(),
                1,
                "lookup via {name:?} after reopening"
            );
        }
    }

    /// An alias is registered under the graph's spelling. A caller typing
    /// the human form of a merged-away project must still land on the
    /// canonical project — after the merge the alias entity is gone, so the
    /// alias table is the only thing left to match.
    #[test]
    fn human_spelling_of_a_merged_alias_resolves() {
        let s = tmp_storage();
        project(&s, "canonical");
        project(&s, "old-project-name");
        s.merge_entities("canonical", "old-project-name").unwrap();

        let f = open_item(&s, "Old Project Name", "Typed the human way", "r1");
        assert_eq!(f.project, "canonical");
        for name in ["canonical", "old-project-name", "Old Project Name"] {
            assert_eq!(
                open_for_project(&s, name, 10).unwrap().len(),
                1,
                "lookup via {name:?}"
            );
        }
    }

    /// Every item must be reachable by paging, however large the backlog:
    /// walking the offset forward visits each row exactly once.
    #[test]
    fn listing_pages_reach_every_item() {
        let s = tmp_storage();
        for i in 0..101 {
            open_item(&s, "demoapp", &format!("item {i}"), &format!("r{i}"));
        }
        let first = list_page(&s, Some("demoapp"), false, 100, 0).unwrap();
        assert_eq!(first.items.len(), 100);
        assert!(first.has_more, "the 101st item must be announced");
        let second = list_page(&s, Some("demoapp"), false, 100, 100).unwrap();
        assert_eq!(second.items.len(), 1);
        assert!(!second.has_more);

        let mut seen = std::collections::HashSet::new();
        for f in first.items.iter().chain(second.items.iter()) {
            assert!(seen.insert(f.id.clone()), "no row is served twice");
        }
        assert_eq!(seen.len(), 101);
        assert!(!list_page(&s, None, false, 200, 0).unwrap().has_more);
    }

    /// Explicit follow-up actions must not fail with "database is locked"
    /// just because another connection is busy ingesting. A deferred
    /// transaction loses its read->write upgrade in exactly this situation;
    /// an IMMEDIATE one waits on the busy timeout instead.
    #[test]
    fn concurrent_ingestion_does_not_reject_followup_writes() {
        let path = crate::test_support::temp_path("mn-fu-busy-", "memory.db");
        let writer = std::sync::Arc::new(Storage::open(&path).unwrap());
        let ingest = Storage::open(&path).unwrap();

        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_bg = stop.clone();
        let bg = std::thread::spawn(move || {
            let mut i = 0;
            while !stop_bg.load(std::sync::atomic::Ordering::Relaxed) {
                let e = MemoryEntry::new(
                    format!("ingested note {i}"),
                    "body",
                    MemoryType::Note,
                    EventSource::Socket,
                );
                ingest.save(&e).unwrap();
                i += 1;
            }
        });

        let mut failures = Vec::new();
        for i in 0..300 {
            let res = create(
                &writer,
                NewFollowup {
                    project: "demoapp",
                    title: &format!("item {i}"),
                    source_memory_id: None,
                    authoritative: true,
                    actor: "test",
                    request_id: &format!("busy-{i}"),
                },
                Utc::now(),
            );
            if let Err(e) = res {
                failures.push(e.to_string());
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        bg.join().unwrap();
        assert!(
            failures.is_empty(),
            "{} of 300 creates rejected under contention, first: {:?}",
            failures.len(),
            failures.first()
        );
        assert_eq!(list(&writer, None, true, 1000).unwrap().len(), 300);
    }

    /// Titles are stored in full (up to the storage cap); only rendering
    /// shortens them. A CLI/MCP follow-up has no source to recover from.
    #[test]
    fn long_titles_are_stored_untruncated() {
        let s = tmp_storage();
        let long = format!(
            "Migrate the importer {}",
            "and keep every detail ".repeat(9)
        );
        assert!(long.chars().count() > 150);
        let f = open_item(
            &s,
            "demoapp",
            &format!("  {long}  \nsecond line ignored"),
            "r1",
        );
        assert_eq!(f.title, long.trim());
        assert!(
            f.title.chars().count() > 110,
            "not cut to the display width"
        );
        let capped = open_item(&s, "demoapp", &"x".repeat(MAX_TITLE_CHARS + 50), "r2");
        assert_eq!(capped.title.chars().count(), MAX_TITLE_CHARS);
    }

    /// The delete trigger must exist on a database whose `memories` table
    /// went through the rebuild migration (it is installed after it).
    #[test]
    fn delete_trigger_is_installed() {
        let s = tmp_storage();
        let conn = s.conn.lock().unwrap();
        let n: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master
                  WHERE type = 'trigger' AND name = 'trg_followups_source_deleted'
                    AND tbl_name = 'memories'",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(n, 1);
    }

    #[test]
    fn id_prefix_resolution_is_strict() {
        let s = tmp_storage();
        let f = open_item(&s, "demoapp", "Ship the importer", "r1");
        assert_eq!(resolve_id(&s, &f.id[..8]).unwrap(), f.id);
        assert_eq!(resolve_id(&s, &f.id).unwrap(), f.id);
        assert!(resolve_id(&s, &f.id[..4]).is_err(), "too short");
        assert!(resolve_id(&s, "zzzzzzzz").is_err(), "not hex");
        assert!(resolve_id(&s, "00000000").is_err(), "unknown");
    }
}
