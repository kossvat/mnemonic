//! The summary of a day on a generated log: the numbers of history, what
//! still exists, the orders of the arrays, and the empty day. The bounds
//! of the day are those of the process's own zone, whatever it is; the
//! test of a named timezone runs the binary.
use super::*;
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::recall_log::{Item, Record, Section, Surface, Tool, Writer};
use chrono::{DateTime, Duration, Utc};

struct Log {
    _dir: tempfile::TempDir,
    storage: Storage,
    named: Writer,
    unnamed: Writer,
    day: NaiveDate,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

impl Log {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mn-recall-day-");
        let path = dir.path().join("memory.db");
        let storage = Storage::open(&path).unwrap();
        let named = Writer::new(&path, true);
        named.name_client("Claude Code");
        let unnamed = Writer::new(&path, true);
        // A day in the past, so that nothing of today is near its bounds.
        let day = NaiveDate::from_ymd_opt(2026, 3, 8).unwrap();
        let (start, end) = crate::activity::local_day_bounds_utc(day).unwrap();
        Self {
            _dir: dir,
            storage,
            named,
            unnamed,
            day,
            start,
            end,
        }
    }

    fn memory(&self, id: &str, title: &str, memory_type: MemoryType) -> String {
        let mut entry = MemoryEntry::new(title, "body", memory_type, EventSource::Socket);
        entry.id = id.to_string();
        self.storage.save(&entry).unwrap();
        entry.id
    }

    fn event(
        &self,
        writer: &Writer,
        at: DateTime<Utc>,
        surface: Surface,
        tool: Tool,
        items: &[(&str, Option<Section>)],
        hidden: usize,
    ) {
        let record = Record {
            surface,
            tool,
            items: items
                .iter()
                .map(|(id, section)| Item {
                    memory_id: id.to_string(),
                    section: *section,
                })
                .collect(),
            chars: 100,
            hidden,
        };
        writer.write(&record, at).unwrap();
    }

    fn summary(&self) -> DaySummary {
        summary(&self.storage, self.day, true).unwrap()
    }
}

/// The fixture the widget's test reads is what this log gives: three
/// memories, three events on the day, one before it and one after it.
fn fixture() -> Log {
    let log = Log::new();
    log.memory(
        "a1",
        "Decision: use SQLite for the index",
        MemoryType::Decision,
    );
    log.memory("b2", "stop, use the other parser", MemoryType::Feedback);
    log.memory(
        "c3",
        "Decision: keep the smaller index",
        MemoryType::Decision,
    );
    log.event(
        &log.unnamed,
        log.start,
        Surface::Context,
        Tool::Context,
        &[
            ("a1", Some(Section::Digest)),
            ("b2", Some(Section::Feedback)),
            ("c3", Some(Section::Decisions)),
        ],
        2,
    );
    log.event(
        &log.named,
        log.start + Duration::hours(7),
        Surface::Mcp,
        Tool::MemorySearch,
        &[("a1", None), ("c3", None)],
        0,
    );
    log.event(
        &log.named,
        log.end - Duration::seconds(1),
        Surface::Mcp,
        Tool::MemoryContext,
        &[("a1", Some(Section::Topic))],
        1,
    );
    // Outside the day, on both sides.
    log.event(
        &log.named,
        log.end,
        Surface::Mcp,
        Tool::MemoryRecent,
        &[("a1", None)],
        0,
    );
    log.event(
        &log.named,
        log.start - Duration::seconds(1),
        Surface::Mcp,
        Tool::MemoryRecent,
        &[("b2", None)],
        0,
    );
    log
}

fn expected() -> DaySummary {
    DaySummary {
        day: "2026-03-08".into(),
        log_enabled: true,
        contexts_generated: 2,
        calls_answered: 1,
        memories_returned: 6,
        corrections_returned: 1,
        hidden_as_noise: 2,
        distinct_memories: 3,
        by_tool: vec![
            ToolCount {
                tool: "context".into(),
                events: 1,
                memories: 3,
            },
            ToolCount {
                tool: "memory_context".into(),
                events: 1,
                memories: 1,
            },
            ToolCount {
                tool: "memory_search".into(),
                events: 1,
                memories: 2,
            },
        ],
        by_client: vec![
            ClientCount {
                client: "claude code".into(),
                events: 2,
            },
            ClientCount {
                client: "unknown".into(),
                events: 1,
            },
        ],
        top: vec![
            TopMemory {
                memory_id: "a1".into(),
                title: "Decision: use SQLite for the index".into(),
                memory_type: "decision".into(),
                times: 3,
            },
            TopMemory {
                memory_id: "c3".into(),
                title: "Decision: keep the smaller index".into(),
                memory_type: "decision".into(),
                times: 2,
            },
            TopMemory {
                memory_id: "b2".into(),
                title: "stop, use the other parser".into(),
                memory_type: "feedback".into(),
                times: 1,
            },
        ],
    }
}

#[test]
fn recall_day_the_numbers_equal_the_hand_computed_ones() {
    let log = fixture();
    assert_eq!(log.summary(), expected());
    // The bytes of the JSON, with the fields in the order of the design.
    let json = serde_json::to_string(&log.summary()).unwrap();
    assert_eq!(json, FIXTURE_JSON.trim_end());
}

#[test]
fn recall_day_the_events_outside_the_day_belong_to_their_own_days() {
    let log = fixture();
    let before = summary(&log.storage, log.day - Duration::days(1), true).unwrap();
    assert_eq!((before.calls_answered, before.memories_returned), (1, 1));
    let after = summary(&log.storage, log.day + Duration::days(1), true).unwrap();
    assert_eq!((after.calls_answered, after.memories_returned), (1, 1));
}

#[test]
fn recall_day_history_does_not_shrink_when_a_memory_is_forgotten() {
    let log = fixture();
    log.storage.forget_by_id("c3").unwrap();
    let after = log.summary();
    let mut wanted = expected();
    wanted.distinct_memories = 2;
    wanted.top.remove(1);
    assert_eq!(after, wanted);
}

#[test]
fn recall_day_the_top_holds_five_with_ties_by_id() {
    let log = Log::new();
    for id in ["f6", "e5", "d4", "c3", "b2", "a1"] {
        log.memory(id, &format!("memory {id}"), MemoryType::Note);
    }
    // Every memory once, and one of them twice.
    log.event(
        &log.named,
        log.start + Duration::hours(1),
        Surface::Mcp,
        Tool::MemoryRecent,
        &[
            ("f6", None),
            ("e5", None),
            ("d4", None),
            ("c3", None),
            ("b2", None),
            ("a1", None),
        ],
        0,
    );
    log.event(
        &log.named,
        log.start + Duration::hours(2),
        Surface::Mcp,
        Tool::MemorySearch,
        &[("e5", None)],
        0,
    );
    let top: Vec<(String, u64)> = log
        .summary()
        .top
        .into_iter()
        .map(|t| (t.memory_id, t.times))
        .collect();
    assert_eq!(
        top,
        [
            ("e5".to_string(), 2),
            ("a1".to_string(), 1),
            ("b2".to_string(), 1),
            ("c3".to_string(), 1),
            ("d4".to_string(), 1),
        ]
    );
    assert_eq!(log.summary().distinct_memories, 6);
}

#[test]
fn recall_day_an_empty_day_is_an_object_of_zeros_and_says_so() {
    let log = Log::new();
    let empty = log.summary();
    assert_eq!(
        empty,
        DaySummary {
            day: "2026-03-08".into(),
            log_enabled: true,
            contexts_generated: 0,
            calls_answered: 0,
            memories_returned: 0,
            corrections_returned: 0,
            hidden_as_noise: 0,
            distinct_memories: 0,
            by_tool: vec![],
            by_client: vec![],
            top: vec![],
        }
    );
    assert!(empty.is_empty());
    assert_eq!(empty.text(), "Memory on 2026-03-08\n  no recorded recalls");
    let off = summary(&log.storage, log.day, false).unwrap();
    assert!(!off.log_enabled);
    assert!(off.text().contains("recall log is off"));
    assert!(off.text().contains("no recorded recalls"));
}

#[test]
fn recall_day_the_text_says_the_same_numbers() {
    let log = fixture();
    let text = log.summary().text();
    for line in [
        "Memory on 2026-03-08",
        "  contexts generated:   2",
        "  calls answered:       1",
        "  memories returned:    6",
        "  corrections returned: 1",
        "  hidden as noise:      2",
        "  distinct memories:    3",
        "    context: 1 events, 3 memories",
        "    memory_search: 1 events, 2 memories",
        "    claude code: 2 events",
        "    unknown: 1 events",
        "    a1  decision  3  Decision: use SQLite for the index",
        "    b2  feedback  1  stop, use the other parser",
    ] {
        assert!(text.contains(line), "{line}\n{text}");
    }
    assert!(!text.contains("recall log is off"));
}

#[test]
fn recall_day_the_text_cuts_an_id_by_characters() {
    let log = Log::new();
    // An imported id need not be ASCII: a byte cut would split the glyph.
    log.memory(
        "abcde\u{1F9E0}fghij",
        "memory with a glyph",
        MemoryType::Note,
    );
    log.event(
        &log.named,
        log.start + Duration::hours(1),
        Surface::Mcp,
        Tool::MemoryRecent,
        &[("abcde\u{1F9E0}fghij", None)],
        0,
    );
    let text = log.summary().text();
    assert!(
        text.contains("    abcde\u{1F9E0}fg  note  1  memory with a glyph"),
        "{text}"
    );
}
