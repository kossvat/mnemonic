//! The log of what memory returned: one event per call the MCP server
//! answered and per context generated, with the ids of the memories in
//! the order returned. No query and no text of a memory is written: the
//! summary of a day needs neither, and so nothing here is for the
//! redaction policy to rewrite.
//!
//! The writer is never in the way of a reply. A record is written after
//! the reply was written and flushed, through a connection of its own
//! with a short wait for a lock, in one transaction; one that cannot be
//! written is dropped with one fixed line on standard error.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, OptionalExtension, params};

/// Where a call was answered.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Surface {
    Mcp,
    Context,
}

impl Surface {
    pub fn as_str(self) -> &'static str {
        match self {
            Surface::Mcp => "mcp",
            Surface::Context => "context",
        }
    }
}

/// What answered: a tool of the MCP server, or the context command.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    MemorySearch,
    MemorySimilar,
    MemoryRecent,
    MemoryGraph,
    MemoryContext,
    MemoryFacts,
    Context,
}

impl Tool {
    pub fn as_str(self) -> &'static str {
        match self {
            Tool::MemorySearch => "memory_search",
            Tool::MemorySimilar => "memory_similar",
            Tool::MemoryRecent => "memory_recent",
            Tool::MemoryGraph => "memory_graph",
            Tool::MemoryContext => "memory_context",
            Tool::MemoryFacts => "memory_facts",
            Tool::Context => "context",
        }
    }
}

/// The section of a context a memory stood in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Digest,
    Feedback,
    Decisions,
    Recent,
    Topic,
}

impl Section {
    pub fn as_str(self) -> &'static str {
        match self {
            Section::Digest => "digest",
            Section::Feedback => "feedback",
            Section::Decisions => "decisions",
            Section::Recent => "recent",
            Section::Topic => "topic",
        }
    }

    /// The section by the word a composition uses for it.
    pub fn of(word: &str) -> Option<Self> {
        Some(match word {
            "digest" => Section::Digest,
            "feedback" => Section::Feedback,
            "decision" | "decisions" => Section::Decisions,
            "recent" => Section::Recent,
            "topic" => Section::Topic,
            _ => return None,
        })
    }
}

/// One memory returned, by its id, with the section of a context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub memory_id: String,
    pub section: Option<Section>,
}

/// What is to be recorded for a call that was answered. How many of the
/// memories are corrections is counted as the record is written, from
/// the store as it is at that time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub surface: Surface,
    pub tool: Tool,
    /// The memories returned, in order.
    pub items: Vec<Item>,
    /// Characters of text returned.
    pub chars: usize,
    /// Memories left out as noise, for a context.
    pub hidden: usize,
}

/// Events older than this are removed.
pub const RETENTION_DAYS: i64 = 90;
/// How many expired events one pass removes, oldest first.
pub const PASS_SIZE: usize = 1000;
/// How long a record waits for a lock, at most.
pub const LOCK_WAIT: Duration = Duration::from_millis(250);
/// The longest name of a client that is stored.
pub const CLIENT_NAME_CHARS: usize = 64;
const PASS_INTERVAL: Duration = Duration::from_secs(3600);

/// The writer of one process: its run, the name its client gave, and
/// the time of its last pass of retention.
pub struct Writer {
    path: PathBuf,
    enabled: bool,
    run: String,
    client: Mutex<Option<String>>,
    last_pass: Mutex<Option<Instant>>,
}

impl Writer {
    /// A writer for the store at `path`. The run is made here, once for
    /// the process.
    pub fn new(path: &Path, enabled: bool) -> Self {
        Self {
            path: path.to_path_buf(),
            enabled,
            run: uuid::Uuid::new_v4().to_string(),
            client: Mutex::new(None),
            last_pass: Mutex::new(None),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn run(&self) -> &str {
        &self.run
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn enabled(&self) -> bool {
        self.enabled
    }

    /// The name of the client, once accepted.
    pub fn client(&self) -> Option<String> {
        self.client.lock().map(|c| c.clone()).unwrap_or(None)
    }

    /// Take the name a client gave at initialize. It is an identity: judged
    /// as it arrived and in lower case, cut to a fixed length, judged again
    /// and stored in lower case. A refused name leaves the writer without
    /// one. The first accepted name stays; a later one changes nothing.
    pub fn name_client(&self, name: &str) {
        let Ok(mut client) = self.client.lock() else {
            return;
        };
        if client.is_some() {
            return;
        }
        if crate::redaction::state::check_identities([name]).is_err() {
            return;
        }
        let cut: String = name
            .to_lowercase()
            .chars()
            .take(CLIENT_NAME_CHARS)
            .collect();
        if cut.is_empty() || crate::redaction::state::check_identities([cut.as_str()]).is_err() {
            return;
        }
        *client = Some(cut);
    }

    /// Record `record` as of now, or drop it with one line on `errors`
    /// that names the tool and nothing of the call.
    pub fn record(&self, record: &Record, errors: &mut dyn std::io::Write) {
        if !self.enabled {
            return;
        }
        if let Err(_e) = self.write(record, Utc::now()) {
            let _ = writeln!(
                errors,
                "mnemonic: a record of {} was dropped",
                record.tool.as_str()
            );
        }
    }

    /// Write one event with its items as of `at`, in one transaction of a
    /// connection of its own, and then make a pass of retention when one is
    /// due.
    pub fn write(&self, record: &Record, at: DateTime<Utc>) -> Result<i64> {
        crate::redaction::state::admit_identities(
            "recall",
            record.items.iter().map(|item| item.memory_id.as_str()),
        )?;
        let mut conn = self.connect()?;
        let event = {
            // The lock is taken at the start, so that the wait for it is
            // the one of the busy handler: a read that is upgraded to a
            // write later gets no wait, only a refusal.
            let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
            // The numbers of an event are written once: what was returned
            // counts a memory gone since, and the corrections are the
            // memories of type feedback as the store holds them now.
            let mut corrections = 0i64;
            let mut items = Vec::with_capacity(record.items.len());
            for (rank, item) in record.items.iter().enumerate() {
                // Only a memory that exists at this moment gets an item,
                // checked inside the transaction that inserts it: the
                // trigger of deletes cannot remove an item that was not
                // there when it fired.
                let memory_type: Option<String> = tx
                    .query_row(
                        "SELECT memory_type FROM memories WHERE id = ?1",
                        params![item.memory_id],
                        |r| r.get(0),
                    )
                    .optional()?;
                let Some(memory_type) = memory_type else {
                    continue;
                };
                if memory_type == "feedback" {
                    corrections += 1;
                }
                items.push((rank, item));
            }
            tx.execute(
                "INSERT INTO recall_events
                    (at, surface, tool, client, run, returned, corrections, chars, hidden)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    at.to_rfc3339(),
                    record.surface.as_str(),
                    record.tool.as_str(),
                    self.client(),
                    self.run,
                    record.items.len() as i64,
                    corrections,
                    record.chars as i64,
                    record.hidden as i64,
                ],
            )?;
            let event = tx.last_insert_rowid();
            for (rank, item) in items {
                tx.execute(
                    "INSERT INTO recall_items (event_id, rank, memory_id, section)
                     VALUES (?1, ?2, ?3, ?4)",
                    params![
                        event,
                        rank as i64,
                        item.memory_id,
                        item.section.map(Section::as_str)
                    ],
                )?;
            }
            tx.commit()?;
            event
        };
        if self.pass_is_due() {
            let done = Self::pass(&conn, at).is_ok();
            if let Ok(mut last) = self.last_pass.lock()
                && done
            {
                *last = Some(Instant::now());
            }
        }
        Ok(event)
    }

    /// A connection of the writer's own: foreign keys on, so that removing
    /// an event removes its items, and a short wait for a lock.
    fn connect(&self) -> Result<Connection> {
        let conn = Connection::open(&self.path)?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.busy_timeout(LOCK_WAIT)?;
        Ok(conn)
    }

    /// Whether a pass of retention is due: with the first record of the
    /// process, and then at most once an hour. A pass that failed is tried
    /// again with the next record.
    fn pass_is_due(&self) -> bool {
        self.last_pass
            .lock()
            .map(|last| last.is_none_or(|at| at.elapsed() >= PASS_INTERVAL))
            .unwrap_or(false)
    }

    /// Remove the oldest expired events, with their items, up to one pass.
    pub fn pass(conn: &Connection, now: DateTime<Utc>) -> Result<usize> {
        let cutoff = (now - chrono::Duration::days(RETENTION_DAYS)).to_rfc3339();
        let removed = conn.execute(
            "DELETE FROM recall_events WHERE id IN (
                SELECT id FROM recall_events WHERE at < ?1 ORDER BY at, id LIMIT ?2
             )",
            params![cutoff, PASS_SIZE as i64],
        )?;
        Ok(removed)
    }
}

#[cfg(test)]
#[path = "recall_log_tests.rs"]
mod tests;
