//! The summary of a day of the recall log: what memory returned on one
//! local calendar day, in one shape for the command line and the local
//! API. The numbers of events are history and do not change when a
//! memory is forgotten; the distinct memories and the top are of what
//! still exists.

use anyhow::Result;
use chrono::NaiveDate;
use rusqlite::params;
use serde::Serialize;

use crate::storage::Storage;

/// How many memories the top names, at most.
pub const TOP: usize = 5;

/// The events of one tool, and the memories they returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ToolCount {
    pub tool: String,
    pub events: u64,
    pub memories: u64,
}

/// The events of one client. A client that gave no name is "unknown".
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ClientCount {
    pub client: String,
    pub events: u64,
}

/// A memory returned most often on the day, as it still exists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TopMemory {
    pub memory_id: String,
    pub title: String,
    pub memory_type: String,
    pub times: u64,
}

/// The summary of one day. The fields are written in this order, and the
/// same bytes come out of the command and of the route for the same
/// store, the same timezone and the same configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct DaySummary {
    pub day: String,
    pub log_enabled: bool,
    pub contexts_generated: u64,
    pub calls_answered: u64,
    pub memories_returned: u64,
    pub corrections_returned: u64,
    pub hidden_as_noise: u64,
    pub distinct_memories: u64,
    pub by_tool: Vec<ToolCount>,
    pub by_client: Vec<ClientCount>,
    pub top: Vec<TopMemory>,
}

impl DaySummary {
    /// Whether the day has any event.
    pub fn is_empty(&self) -> bool {
        self.contexts_generated == 0 && self.calls_answered == 0
    }

    /// The summary as the command prints it without --json.
    pub fn text(&self) -> String {
        let mut lines = vec![format!("Memory on {}", self.day)];
        if !self.log_enabled {
            lines.push("  recall log is off".to_string());
        }
        if self.is_empty() {
            lines.push("  no recorded recalls".to_string());
            return lines.join("\n");
        }
        lines.push(format!(
            "  contexts generated:   {}",
            self.contexts_generated
        ));
        lines.push(format!("  calls answered:       {}", self.calls_answered));
        lines.push(format!(
            "  memories returned:    {}",
            self.memories_returned
        ));
        lines.push(format!(
            "  corrections returned: {}",
            self.corrections_returned
        ));
        lines.push(format!("  hidden as noise:      {}", self.hidden_as_noise));
        lines.push(format!(
            "  distinct memories:    {}",
            self.distinct_memories
        ));
        if !self.by_tool.is_empty() {
            lines.push("  by tool:".to_string());
            for tool in &self.by_tool {
                lines.push(format!(
                    "    {}: {} events, {} memories",
                    tool.tool, tool.events, tool.memories
                ));
            }
        }
        if !self.by_client.is_empty() {
            lines.push("  by client:".to_string());
            for client in &self.by_client {
                lines.push(format!("    {}: {} events", client.client, client.events));
            }
        }
        if !self.top.is_empty() {
            lines.push("  returned most:".to_string());
            for top in &self.top {
                // Eight characters of the id: an imported id need not be
                // ASCII, and a byte cut would split a character.
                lines.push(format!(
                    "    {}  {}  {}  {}",
                    top.memory_id.chars().take(8).collect::<String>(),
                    top.memory_type,
                    top.times,
                    top.title
                ));
            }
        }
        lines.join("\n")
    }
}

/// The summary of `day`, a local calendar day with the bounds the Journal
/// uses. `log_enabled` is of the process that answers. Everything is read
/// inside one transaction: the log's writer commits through a connection
/// of its own, so a recall recorded while the summary is computed is in
/// all of its numbers or in none of them.
pub fn summary(storage: &Storage, day: NaiveDate, log_enabled: bool) -> Result<DaySummary> {
    let Some((start, end)) = crate::activity::local_day_bounds_utc(day) else {
        anyhow::bail!("the day {day} has no local midnight");
    };
    let (start, end) = (start.to_rfc3339(), end.to_rfc3339());
    let conn = storage
        .conn
        .lock()
        .map_err(|e| anyhow::anyhow!("lock: {e}"))?;
    let tx = conn.unchecked_transaction()?;

    // The numbers of history, from the events alone.
    let (contexts_generated, calls_answered, memories_returned, corrections_returned, hidden): (
        u64,
        u64,
        u64,
        u64,
        u64,
    ) = tx.query_row(
        "SELECT
            COALESCE(SUM(surface = 'context' OR tool = 'memory_context'), 0),
            COALESCE(SUM(NOT (surface = 'context' OR tool = 'memory_context')), 0),
            COALESCE(SUM(returned), 0),
            COALESCE(SUM(corrections), 0),
            COALESCE(MAX(CASE WHEN surface = 'context' OR tool = 'memory_context'
                              THEN hidden ELSE 0 END), 0)
         FROM recall_events WHERE at >= ?1 AND at < ?2",
        params![start, end],
        |r| {
            Ok((
                r.get::<_, i64>(0)? as u64,
                r.get::<_, i64>(1)? as u64,
                r.get::<_, i64>(2)? as u64,
                r.get::<_, i64>(3)? as u64,
                r.get::<_, i64>(4)? as u64,
            ))
        },
    )?;

    let mut by_tool = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT tool, COUNT(*), COALESCE(SUM(returned), 0) FROM recall_events
             WHERE at >= ?1 AND at < ?2 GROUP BY tool ORDER BY tool",
        )?;
        for row in stmt.query_map(params![start, end], |r| {
            Ok(ToolCount {
                tool: r.get(0)?,
                events: r.get::<_, i64>(1)? as u64,
                memories: r.get::<_, i64>(2)? as u64,
            })
        })? {
            by_tool.push(row?);
        }
    }

    let mut by_client = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT COALESCE(client, 'unknown'), COUNT(*) FROM recall_events
             WHERE at >= ?1 AND at < ?2 GROUP BY COALESCE(client, 'unknown')
             ORDER BY COALESCE(client, 'unknown')",
        )?;
        for row in stmt.query_map(params![start, end], |r| {
            Ok(ClientCount {
                client: r.get(0)?,
                events: r.get::<_, i64>(1)? as u64,
            })
        })? {
            by_client.push(row?);
        }
    }

    // What still exists: the items of the day's events name it, and the
    // trigger of deletes removed the items of what is gone.
    let distinct_memories: u64 = tx.query_row(
        "SELECT COUNT(DISTINCT i.memory_id) FROM recall_items i
         JOIN recall_events e ON e.id = i.event_id
         JOIN memories m ON m.id = i.memory_id
         WHERE e.at >= ?1 AND e.at < ?2",
        params![start, end],
        |r| r.get::<_, i64>(0).map(|n| n as u64),
    )?;

    let mut top = Vec::new();
    {
        let mut stmt = tx.prepare(
            "SELECT i.memory_id, m.title, m.memory_type, COUNT(*) AS times
             FROM recall_items i
             JOIN recall_events e ON e.id = i.event_id
             JOIN memories m ON m.id = i.memory_id
             WHERE e.at >= ?1 AND e.at < ?2
             GROUP BY i.memory_id
             ORDER BY times DESC, i.memory_id
             LIMIT ?3",
        )?;
        for row in stmt.query_map(params![start, end, TOP as i64], |r| {
            Ok(TopMemory {
                memory_id: r.get(0)?,
                title: r.get(1)?,
                memory_type: r.get(2)?,
                times: r.get::<_, i64>(3)? as u64,
            })
        })? {
            top.push(row?);
        }
    }

    tx.commit()?;
    Ok(DaySummary {
        day: day.to_string(),
        log_enabled,
        contexts_generated,
        calls_answered,
        memories_returned,
        corrections_returned,
        hidden_as_noise: hidden,
        distinct_memories,
        by_tool,
        by_client,
        top,
    })
}

/// The summary the command prints for the fixture store of the tests: the
/// file the widget's test decodes.
#[cfg(test)]
pub const FIXTURE_JSON: &str = include_str!("../clients/macos/Tests/Fixtures/recall-day.json");

#[cfg(test)]
#[path = "recall_day_tests.rs"]
mod tests;
