//! Shared durable JSONL checkpointing for both transcript formats.
use std::collections::{HashMap, HashSet};
use std::fs::{File, Metadata};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Result, ensure};
use chrono::Utc;
use tracing::warn;

use super::tail;
use crate::event::{Event, EventKind, EventSource};
use crate::ingest::{IngestCursor, IngestPayload, IngestRecord};
use crate::storage::Storage;

pub(super) struct ParsedTurn {
    pub event: Event,
    pub source_at: Option<String>,
    pub transcript_id: Option<String>,
    pub message_id: Option<String>,
}

impl ParsedTurn {
    /// Construct the bounded downstream event. The timestamp is preserved
    /// verbatim in source_at and normalized in Event.
    ///
    /// Note what `content` holds: a decision keeps only its excerpt, but a
    /// correction keeps the WHOLE message, because that is the memory. Not
    /// storing the turn separately bounds the copies, it does not make the
    /// payload free of private text.
    pub(super) fn new(
        line: &str,
        source: EventSource,
        role: &str,
        message: String,
        source_at: String,
        metadata: serde_json::Value,
    ) -> Option<Self> {
        use super::conversation::ConversationWatcher;
        let (kind, content) = if role == "user" && ConversationWatcher::is_correction(&message) {
            (EventKind::UserCorrection, message.clone())
        } else if ConversationWatcher::is_decision(&message) {
            (
                EventKind::Custom("conversation_decision".into()),
                ConversationWatcher::decision_excerpt(&message),
            )
        } else {
            return None;
        };
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let mut event = Event::new(source.clone(), kind, content).with_metadata(metadata);
        if let Ok(time) = chrono::DateTime::parse_from_rfc3339(&source_at) {
            event.timestamp = time.with_timezone(&Utc);
        }
        let message_id = match source {
            EventSource::ConversationWatcher => {
                value.get("uuid").or_else(|| value.pointer("/message/id"))
            }
            EventSource::CodexWatcher => value.pointer("/payload/id").or_else(|| value.get("id")),
            _ => None,
        }
        .and_then(|v| v.as_str())
        .filter(|v| !v.is_empty())
        .map(str::to_owned);
        let transcript_id = value
            .get("sessionId")
            .or_else(|| value.get("session_id"))
            .and_then(|v| v.as_str())
            .filter(|v| !v.is_empty())
            .map(str::to_owned);
        Some(Self {
            event,
            source_at: (!source_at.is_empty()).then_some(source_at),
            transcript_id,
            message_id,
        })
    }
}

/// The watcher whose message ids are globally unique, not per transcript.
pub(super) const CONVERSATION_NAMESPACE: &str = "conversation";

/// The cursor stream of one transcript. Transcript filenames contain the
/// session identity; the filename rather than the directory also lets a
/// rename into archives resume.
pub(super) fn stream_of(namespace: &str, path: &Path) -> String {
    serde_json::json!([
        namespace,
        path.file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
    ])
    .to_string()
}

/// The durable identity of a turn that carries a message id, or `None`.
///
/// Claude Code mints `uuid` per MESSAGE and copies it verbatim when /compact
/// or a resume carries earlier turns into a new transcript, but rewrites
/// `sessionId`. Scoping the key by transcript would re-ingest every copied
/// turn under a new key, and an ingress UserCorrection bypasses semantic
/// dedup, so it would land twice in the store. Measured on this corpus:
/// 10,084 uuids appear in two transcripts with DIFFERENT session ids, and no
/// uuid binds two different captured texts.
///
/// Codex ids are only known to be unique within one rollout, so that
/// namespace keeps the transcript scope.
pub(super) fn message_key(namespace: &str, stream: &str, turn: &ParsedTurn) -> Option<String> {
    let message = turn.message_id.as_deref()?;
    Some(if namespace == CONVERSATION_NAMESPACE {
        serde_json::json!([namespace, "message", message]).to_string()
    } else {
        serde_json::json!([
            namespace,
            "message",
            turn.transcript_id.as_deref().unwrap_or(stream),
            message
        ])
        .to_string()
    })
}

/// Decides which records of one transcript may be captured, for a store that
/// serves one project out of a folder shared by every project.
///
/// It sees every complete record in order, including the ones `parse`
/// ignores, because Codex announces the session's folder in records that
/// carry no turn. A refused record is still consumed: the cursor moves past
/// it, so it is never retried.
pub(super) trait RecordScope {
    /// About to read `path` from byte `offset` of cursor `generation`.
    /// `file` is the descriptor the records come from: anything the scope
    /// reads back must come from the same file, not whatever the path names
    /// by now.
    fn begin(&mut self, file: &mut File, path: &Path, generation: u64, offset: u64) -> Result<()>;
    /// May a turn in this record be captured?
    fn admit(&mut self, line: &str) -> bool;
    /// The poll committed everything up to `next_offset`.
    fn commit(&mut self, path: &Path, generation: u64, next_offset: u64);
}

pub(super) struct IngressTail {
    storage: Arc<Storage>,
    namespace: &'static str,
    /// Historic files whose adoption must be retried. `None` until the first
    /// bootstrap pass of this process has looked at every existing file.
    adoption_retries: Mutex<Option<HashSet<PathBuf>>>,
}

/// Outcome of adopting one pre-existing transcript.
enum Adoption {
    Settled,
    /// Not adopted yet: rewritten under us, or unreadable. Try again next
    /// tick; `Some(reason)` is worth telling the user about once.
    Retry(Option<String>),
}

impl IngressTail {
    pub fn new(storage: Arc<Storage>, namespace: &'static str) -> Self {
        Self {
            storage,
            namespace,
            adoption_retries: Mutex::new(None),
        }
    }

    /// Every cursor of this watcher's namespace, read in one query. Stream
    /// keys are the JSON array built in [`Self::stream`], so its serialized
    /// opening `["<namespace>",` is an exact prefix (the bootstrap marker,
    /// `initialized:<namespace>`, does not match it).
    pub fn cursor_snapshot(&self) -> Result<HashMap<String, IngestCursor>> {
        let prefix = format!("[{},", serde_json::to_string(self.namespace)?);
        self.storage.ingest_cursors_with_prefix(&prefix)
    }

    /// [`Self::tick_scoped`] with no project scope (tests).
    #[cfg(test)]
    pub fn tick(
        &self,
        files: &[PathBuf],
        legacy: &HashMap<PathBuf, u64>,
        parse: fn(&Path, &str) -> Option<ParsedTurn>,
    ) -> Result<usize> {
        self.tick_scoped(files, legacy, parse, None)
    }

    /// One poll tick: adopt what is still pending, read this namespace's
    /// cursors ONCE, then poll every transcript against that snapshot,
    /// keeping only the records `scope` admits when there is one.
    /// Returns how many turns were captured.
    ///
    /// Adoption or snapshot failure fails the whole tick, and it is retried on
    /// the next one; a failure on one transcript is logged and does not stop
    /// the rest. A stale snapshot is safe: `append_ingest` re-reads the cursor
    /// inside its write transaction and refuses a compare-and-swap that no
    /// longer matches, so the worst case is one skipped poll for that file.
    pub fn tick_scoped(
        &self,
        files: &[PathBuf],
        legacy: &HashMap<PathBuf, u64>,
        parse: fn(&Path, &str) -> Option<ParsedTurn>,
        mut scope: Option<&mut dyn RecordScope>,
    ) -> Result<usize> {
        let adopting = self.bootstrap(files, legacy)?;
        let mut snapshot = self.cursor_snapshot()?;
        let mut captured = 0;
        for path in files {
            if adopting.contains(path) {
                continue;
            }
            match self.poll(path, parse, &mut snapshot, scope.as_deref_mut()) {
                Ok(n) => captured += n,
                Err(e) => warn!(
                    "{} ingress failed for {}: {e}",
                    self.namespace,
                    path.display()
                ),
            }
        }
        Ok(captured)
    }

    /// Poll one transcript on its own (tests). Production goes through
    /// [`Self::tick`]; this reads the snapshot the same way, just for one file.
    #[cfg(test)]
    pub fn poll_file(
        &self,
        path: &Path,
        parse: fn(&Path, &str) -> Option<ParsedTurn>,
    ) -> Result<usize> {
        let mut snapshot = self.cursor_snapshot()?;
        self.poll_file_with(path, parse, &mut snapshot)
    }

    /// Transcript filenames contain the session identity. Using the filename
    /// rather than the directory also lets a rename into archives resume.
    pub fn stream(&self, path: &Path) -> String {
        stream_of(self.namespace, path)
    }

    /// Legacy JSON offsets are read only during initial adoption. A durable
    /// bootstrap marker distinguishes first-ever history from sessions created
    /// while the daemon was down, which must start at byte zero on restart.
    ///
    /// Adoption is per file (review point): one unreadable or busy transcript
    /// must not stop every healthy stream from being polled. Returns the
    /// files to leave alone this tick because their adoption is still being
    /// retried; polling one of those would replay its whole history from byte
    /// zero. An unreadable file does not hold the marker back: it cannot be
    /// fingerprinted at all, and is read from the top if it ever opens.
    pub fn bootstrap(
        &self,
        files: &[PathBuf],
        legacy: &HashMap<PathBuf, u64>,
    ) -> Result<HashSet<PathBuf>> {
        let marker = format!("initialized:{}", self.namespace);
        if self.storage.ingest_cursor(&marker)?.is_some() {
            return Ok(HashSet::new());
        }
        let mut retries = self
            .adoption_retries
            .lock()
            .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
        // First pass: every existing file is history. Later passes revisit
        // only the files still being retried; anything else that has no
        // cursor by then was created after startup and is a NEW session,
        // which the poll reads from its first byte.
        let candidates: Vec<PathBuf> = match retries.as_ref() {
            None => files.to_vec(),
            Some(pending) => pending.iter().cloned().collect(),
        };
        let mut pending = HashSet::new();
        let mut outcome = Ok(());
        for (index, path) in candidates.iter().enumerate() {
            match self.adopt(path, legacy) {
                Ok(Adoption::Settled) => {}
                Ok(Adoption::Retry(reason)) => {
                    // A persistent condition must not print every tick.
                    if let Some(reason) = reason
                        && !retries.as_ref().is_some_and(|prior| prior.contains(path))
                    {
                        warn!(
                            "{} ingress cannot adopt {} yet: {reason}",
                            self.namespace,
                            path.display()
                        );
                    }
                    pending.insert(path.clone());
                }
                Err(e) => {
                    // The store failed. Everything this pass has not settled
                    // stays for the next one, INCLUDING the candidates it
                    // never reached (review point): dropping those would let
                    // a later pass finish without them, and the poll reads a
                    // cursor-less file from byte zero, replaying its whole
                    // history and ignoring its legacy offset.
                    pending.insert(path.clone());
                    pending.extend(candidates[index + 1..].iter().cloned());
                    outcome = Err(e);
                    break;
                }
            }
        }
        // Record the pass BEFORE anything else can fail (review point). If a
        // transient error escaped here with `retries` still None, the next
        // tick would rescan the whole file list and adopt any session created
        // in between as history, checkpointing it at EOF and losing its
        // opening turns -- the most correction-dense part of a session.
        *retries = Some(pending.clone());
        outcome?;
        if pending.is_empty() {
            self.storage.append_ingest(
                None,
                &IngestCursor {
                    stream: marker,
                    generation: 0,
                    offset: 0,
                    file_id: String::new(),
                    prefix_len: 0,
                    prefix_hash: String::new(),
                    anchor_hash: String::new(),
                },
                &[],
            )?;
        }
        Ok(pending)
    }

    fn adopt(&self, path: &Path, legacy: &HashMap<PathBuf, u64>) -> Result<Adoption> {
        let stream = self.stream(path);
        if self.storage.ingest_cursor(&stream)?.is_some() {
            return Ok(Adoption::Settled);
        }
        // A file we cannot fingerprint is RETRIED, never settled: settling it
        // would leave it cursor-less, and the poll reads a cursor-less file
        // from byte zero, replaying its whole history the moment it becomes
        // readable. Only a vanished path is settled. Store errors propagate.
        let fingerprinted = (|| -> std::io::Result<Option<IngestCursor>> {
            let mut file = File::open(path)?;
            let meta = file.metadata()?;
            let saved = legacy.get(path).copied();
            let truncated = saved.is_some_and(|offset| offset > meta.len());
            let offset = if truncated {
                0
            } else {
                tail::last_complete_offset(&mut file, saved.unwrap_or(meta.len()))?
            };
            let generation = u64::from(truncated);
            let next = checkpoint(&mut file, &meta, stream.clone(), generation, offset, None)
                .map_err(std::io::Error::other)?;
            // An append while we looked is harmless: the adopted offset and
            // both fingerprints lie before it. Anything else is a rewrite.
            let after = file.metadata()?;
            if after.len() < meta.len() {
                return Ok(None);
            }
            if after.len() != meta.len() || after.modified()? != meta.modified()? {
                let again = checkpoint(&mut file, &meta, stream.clone(), generation, offset, None)
                    .map_err(std::io::Error::other)?;
                if again != next {
                    return Ok(None);
                }
            }
            Ok(Some(next))
        })();
        match fingerprinted {
            Ok(Some(next)) => {
                self.storage.append_ingest(None, &next, &[])?;
                Ok(Adoption::Settled)
            }
            Ok(None) => Ok(Adoption::Retry(None)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Adoption::Settled),
            Err(e) => Ok(Adoption::Retry(Some(e.to_string()))),
        }
    }

    #[cfg(test)]
    fn poll_file_with(
        &self,
        path: &Path,
        parse: fn(&Path, &str) -> Option<ParsedTurn>,
        snapshot: &mut HashMap<String, IngestCursor>,
    ) -> Result<usize> {
        self.poll(path, parse, snapshot, None)
    }

    /// `poll_file` limited to the records `scope` admits (tests; production
    /// goes through [`Self::tick_scoped`]).
    #[cfg(test)]
    pub fn poll_file_scoped(
        &self,
        path: &Path,
        parse: fn(&Path, &str) -> Option<ParsedTurn>,
        scope: &mut dyn RecordScope,
    ) -> Result<usize> {
        let mut snapshot = self.cursor_snapshot()?;
        self.poll(path, parse, &mut snapshot, Some(scope))
    }

    fn poll<'s>(
        &self,
        path: &Path,
        parse: fn(&Path, &str) -> Option<ParsedTurn>,
        snapshot: &mut HashMap<String, IngestCursor>,
        mut scope: Option<&mut (dyn RecordScope + 's)>,
    ) -> Result<usize> {
        // Stat/read/fingerprint the same descriptor: a path may rotate at any
        // time. Its replacement is detected on the next poll.
        let mut file = File::open(path)?;
        let meta = file.metadata()?;
        let stream = self.stream(path);
        let previous = snapshot.get(&stream).cloned();
        let (generation, offset, known_prefix) = match previous.as_ref() {
            Some(previous) => match unchanged(&mut file, &meta, previous)? {
                Some(prefix_hash) => (
                    previous.generation,
                    previous.offset,
                    Some((previous.prefix_len, prefix_hash)),
                ),
                None => (
                    previous
                        .generation
                        .checked_add(1)
                        .ok_or_else(|| anyhow::anyhow!("generation exhausted"))?,
                    0,
                    None,
                ),
            },
            None => (0, 0, None),
        };
        let chunk = tail::read_snapshot(&mut file, meta.len(), offset)?;
        let observed_at = Utc::now();
        let mut records = Vec::new();
        let mut begun = false;
        for (position, line) in chunk.records(offset) {
            // Only a file with new records pays for the scope's setup.
            let admitted = match scope.as_deref_mut() {
                Some(scope) => {
                    if !begun {
                        scope.begin(&mut file, path, generation, offset)?;
                        begun = true;
                    }
                    scope.admit(line)
                }
                None => true,
            };
            let Some(mut turn) = parse(path, line) else {
                continue;
            };
            if !admitted {
                continue;
            }
            if turn
                .source_at
                .as_deref()
                .and_then(|ts| chrono::DateTime::parse_from_rfc3339(ts).ok())
                .is_none()
            {
                turn.event.timestamp = observed_at;
            }
            let source_key = message_key(self.namespace, &stream, &turn).unwrap_or_else(|| {
                serde_json::json!([self.namespace, "position", stream, generation, position])
                    .to_string()
            });
            records.push(IngestRecord {
                source_key,
                source_at: turn.source_at,
                observed_at,
                payload: IngestPayload { event: turn.event },
            });
        }
        let next = checkpoint(
            &mut file,
            &meta,
            stream,
            generation,
            chunk.next_offset,
            known_prefix
                .as_ref()
                .map(|(len, hash)| (*len, hash.as_str())),
        )?;
        // Validate after reading the cursor fingerprints too: a rewrite
        // during either parsing or checkpointing must leave the cursor alone.
        //
        // An APPEND is not a rewrite (review point). `read_snapshot` stopped
        // at the length captured above, so a turn that lands while we parse
        // belongs to the next poll; refusing the batch for it would starve a
        // transcript that is written continuously. What must still hold is
        // that the bytes we consumed, now and in earlier polls, are the bytes
        // on disk.
        let after = file.metadata()?;
        ensure!(after.len() >= meta.len(), "transcript shrank while reading");
        if after.len() != meta.len() || after.modified()? != meta.modified()? {
            let resumed = previous
                .as_ref()
                .filter(|cursor| cursor.generation == generation);
            let earlier_intact = match resumed {
                Some(cursor) => unchanged(&mut file, &after, cursor)?.is_some(),
                None => true, // read from byte zero: the chunk is everything
            };
            ensure!(
                earlier_intact && chunk.still_on_disk(&mut file, offset)?,
                "transcript rewritten while reading"
            );
        }
        if previous.as_ref() != Some(&next) || !records.is_empty() {
            self.storage
                .append_ingest(previous.as_ref(), &next, &records)?;
            // Only a COMMITTED cursor enters the snapshot.
            snapshot.insert(next.stream.clone(), next);
        }
        // Only now is the new position durable; a failure above leaves the
        // scope where the cursor still is.
        if begun && let Some(scope) = scope {
            scope.commit(path, generation, chunk.next_offset);
        }
        Ok(records.len())
    }
}

fn file_id(meta: &Metadata) -> String {
    let created = meta
        .created()
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{}:{}:{created}", meta.dev(), meta.ino())
    }
    #[cfg(not(unix))]
    {
        created.to_string()
    }
}

/// A deterministic content fingerprint, not an authentication primitive. Store
/// no raw prefix/suffix bytes in the indefinitely retained cursor table.
fn fingerprint(file: &mut File, start: u64, len: u64) -> Result<String> {
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = vec![0; len as usize];
    file.read_exact(&mut bytes)?;
    let hash = bytes.into_iter().fold(0xcbf29ce484222325u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    Ok(format!("{hash:016x}"))
}
/// `Some(prefix_hash)` when the file still matches the cursor, `None` when it
/// does not. The hash is handed back so the checkpoint that follows does not
/// read and hash the same bytes of the same descriptor a second time: on an
/// idle transcript that duplicate pass is half of this feature's steady-state
/// read volume, on the tokio runtime, every tick, forever.
fn unchanged(file: &mut File, meta: &Metadata, cursor: &IngestCursor) -> Result<Option<String>> {
    if cursor.file_id != file_id(meta) || meta.len() < cursor.offset {
        return Ok(None);
    }
    let prefix_hash = fingerprint(file, 0, cursor.prefix_len)?;
    let anchor_len = cursor.offset.min(128);
    if prefix_hash != cursor.prefix_hash
        || fingerprint(file, cursor.offset - anchor_len, anchor_len)? != cursor.anchor_hash
    {
        return Ok(None);
    }
    Ok(Some(prefix_hash))
}
/// `known_prefix` is a `(len, hash)` pair already computed over this same
/// descriptor; it is reused only when the prefix has not moved.
fn checkpoint(
    file: &mut File,
    meta: &Metadata,
    stream: String,
    generation: u64,
    offset: u64,
    known_prefix: Option<(u64, &str)>,
) -> Result<IngestCursor> {
    let prefix_len = offset.min(4096);
    let anchor_len = offset.min(128);
    let prefix_hash = match known_prefix {
        Some((len, hash)) if len == prefix_len => hash.to_string(),
        _ => fingerprint(file, 0, prefix_len)?,
    };
    Ok(IngestCursor {
        stream,
        generation,
        offset,
        file_id: file_id(meta),
        prefix_len,
        prefix_hash,
        anchor_hash: fingerprint(file, offset - anchor_len, anchor_len)?,
    })
}

#[cfg(test)]
#[path = "ingress_tests.rs"]
mod tests;
