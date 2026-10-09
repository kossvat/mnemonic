//! The recall log over the MCP surface: which calls write an event, with
//! which items, and which write nothing; what the reply carries before
//! the record is tried; and that no text is in the log.
use std::io::Cursor;

use super::*;
use crate::graph::{Entity, EntityType};
use crate::quality::Capture;
use crate::recall_log::Section;
use crate::test_support::{ConstEmbedder, captured};

/// One event as the table holds it: id, surface, tool, client, returned,
/// corrections, chars, hidden.
type EventRow = (i64, String, String, Option<String>, i64, i64, i64, i64);

struct Harness {
    dir: tempfile::TempDir,
    storage: Storage,
    server: McpServer,
    sinks: Vec<Box<dyn OutputSink>>,
}

impl Harness {
    fn new() -> Self {
        Self::with_log(true)
    }

    fn with_log(log: bool) -> Self {
        let dir = crate::test_support::temp_dir("mn-mcp-recall-");
        let mut config = Config::default();
        config.storage.db_path = dir.path().join("memory.db");
        config.output.memory_files_path = dir.path().join("memory-files");
        config.recall.log = log;
        let storage = Storage::open(&config.storage.db_path).unwrap();
        Self {
            dir,
            storage,
            server: McpServer::new(config),
            sinks: Vec::new(),
        }
    }

    fn save(&self, entry: &MemoryEntry) -> String {
        let vector = ConstEmbedder.embed("").unwrap();
        self.storage
            .save_with_embedding(entry, Some(&vector))
            .unwrap();
        entry.id.clone()
    }

    /// Serve the lines, one request each, and give back what was written
    /// to the output and to the errors.
    fn serve(&self, lines: &[String]) -> (String, String) {
        let input = lines.join("\n") + "\n";
        let mut output = Vec::new();
        let mut errors = Vec::new();
        self.server
            .serve_store(
                Cursor::new(input).lines(),
                &mut output,
                &mut errors,
                &self.storage,
                &ConstEmbedder,
                &ImportanceScorer::default(),
                None,
                &self.sinks,
            )
            .unwrap();
        (
            String::from_utf8(output).unwrap(),
            String::from_utf8(errors).unwrap(),
        )
    }

    fn call(&self, tool: &str, arguments: Value) -> (String, String) {
        self.serve(&[json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}})
        .to_string()])
    }

    fn direct(&self, method: &str, params: Value) -> (String, String) {
        self.serve(&[
            json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}).to_string(),
        ])
    }

    fn events(&self) -> Vec<EventRow> {
        let conn = self.storage.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, surface, tool, client, returned, corrections, chars, hidden
                 FROM recall_events ORDER BY id",
            )
            .unwrap();
        stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                r.get(1)?,
                r.get(2)?,
                r.get(3)?,
                r.get(4)?,
                r.get(5)?,
                r.get(6)?,
                r.get(7)?,
            ))
        })
        .unwrap()
        .map(|r| r.unwrap())
        .collect()
    }

    fn items(&self, event: i64) -> Vec<(String, Option<String>)> {
        let conn = self.storage.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT memory_id, section FROM recall_items WHERE event_id = ?1 ORDER BY rank",
            )
            .unwrap();
        stmt.query_map(rusqlite::params![event], |r| Ok((r.get(0)?, r.get(1)?)))
            .unwrap()
            .map(|r| r.unwrap())
            .collect()
    }

    fn only_event(&self, tool: &str) -> (i64, Vec<(String, Option<String>)>) {
        let events = self.events();
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].1, "mcp");
        assert_eq!(events[0].2, tool);
        (events[0].0, self.items(events[0].0))
    }

    fn clear(&self) {
        let conn = self.storage.conn.lock().unwrap();
        conn.execute_batch("DELETE FROM recall_events").unwrap();
    }

    /// Every text of every column of the log, for a search.
    fn log_text(&self) -> String {
        let conn = self.storage.conn.lock().unwrap();
        let mut out = String::new();
        for sql in [
            "SELECT id || at || surface || tool || coalesce(client, '') || run || returned
                    || corrections || chars || hidden FROM recall_events",
            "SELECT event_id || rank || memory_id || coalesce(section, '') FROM recall_items",
        ] {
            let mut stmt = conn.prepare(sql).unwrap();
            for row in stmt.query_map([], |r| r.get::<_, String>(0)).unwrap() {
                out.push_str(&row.unwrap());
                out.push('\n');
            }
        }
        out
    }
}

const SQLITE: &str = "Decision: use SQLite for the index";
const CACHE: &str = "Decision: use SQLite for the cache too";
const ORDER: &str = "stop, use the other parser for the index";

fn ids(items: &[(String, Option<String>)]) -> Vec<&str> {
    items.iter().map(|(id, _)| id.as_str()).collect()
}

#[test]
fn recall_log_search_similar_and_recent_write_one_event_each_through_both_entries() {
    let h = Harness::new();
    let a = h.save(&captured(Capture::Decision, SQLITE));
    let b = h.save(&captured(Capture::Correction, ORDER));
    let c = h.save(&captured(Capture::Decision, CACHE));

    for (tool, arguments) in [
        ("memory_search", json!({"query": "index", "limit": 10})),
        ("memory_similar", json!({"query": "index", "limit": 10})),
        ("memory_recent", json!({"limit": 10})),
    ] {
        for entry in ["tools/call", "direct"] {
            h.clear();
            let (output, errors) = if entry == "tools/call" {
                h.call(tool, arguments.clone())
            } else {
                h.direct(tool, arguments.clone())
            };
            assert!(errors.is_empty(), "{tool} {entry}: {errors}");
            let reply: Value = serde_json::from_str(output.trim()).unwrap();
            let results: Vec<Value> = if entry == "tools/call" {
                let text = reply["result"]["content"][0]["text"].as_str().unwrap();
                serde_json::from_str::<Value>(text).unwrap()["results"]
                    .as_array()
                    .unwrap()
                    .clone()
            } else {
                reply["result"]["results"].as_array().unwrap().clone()
            };
            let returned: Vec<&str> = results.iter().map(|r| r["id"].as_str().unwrap()).collect();
            assert!(!returned.is_empty(), "{tool} {entry}");
            let (event, items) = h.only_event(tool);
            assert_eq!(
                ids(&items),
                returned,
                "{tool} {entry}: in the order returned"
            );
            let numbers = &h.events()[0];
            assert_eq!(
                numbers.4 as usize,
                returned.len(),
                "{tool} {entry}: returned"
            );
            let corrections = returned.iter().filter(|id| **id == b).count();
            assert_eq!(
                numbers.5 as usize, corrections,
                "{tool} {entry}: corrections"
            );
            assert!(numbers.6 > 0, "{tool} {entry}: chars");
            assert_eq!(numbers.7, 0, "{tool} {entry}: hidden");
            assert!(event > 0);
        }
    }
    let _ = (a, c);
}

#[test]
fn recall_log_a_graph_reply_records_the_ids_the_handler_knew() {
    let h = Harness::new();
    let entity = h
        .storage
        .upsert_entity(&Entity {
            name: "demoapp".into(),
            entity_type: EntityType::Project,
        })
        .unwrap();
    let a = h.save(&captured(Capture::Decision, SQLITE));
    let b = h.save(&captured(Capture::Correction, ORDER));
    for id in [&a, &b] {
        h.storage.link_memory_entity(id, &entity).unwrap();
    }
    let known: Vec<String> = h
        .storage
        .graph_query("demoapp")
        .unwrap()
        .memories
        .iter()
        .map(|m| m.id.clone())
        .collect();
    assert_eq!(known.len(), 2);

    for entry in ["tools/call", "direct"] {
        h.clear();
        let (output, errors) = if entry == "tools/call" {
            h.call("memory_graph", json!({"entity": "demoapp"}))
        } else {
            h.direct("memory_graph", json!({"entity": "demoapp"}))
        };
        assert!(errors.is_empty(), "{errors}");
        // The reply sends titles and no ids; the items are the ids the
        // handler knew.
        for id in &known {
            assert!(
                !output.contains(id.as_str()),
                "{entry}: the reply sends no id"
            );
        }
        let (_, items) = h.only_event("memory_graph");
        assert_eq!(
            ids(&items),
            known.iter().map(String::as_str).collect::<Vec<_>>(),
            "{entry}"
        );
        assert_eq!(h.events()[0].5, 1, "one correction");
    }

    // An entity that is not found writes nothing; so does the list.
    h.clear();
    h.call("memory_graph", json!({"entity": "nowhere"}));
    h.direct("memory_graph", json!({"list_all": true}));
    assert!(h.events().is_empty());
}

#[test]
fn recall_log_a_context_records_the_memories_it_shows_with_their_sections() {
    let h = Harness::new();
    let a = h.save(&captured(Capture::Decision, SQLITE));
    let b = h.save(&captured(Capture::Correction, ORDER));
    let noise = h.save(&captured(Capture::Decision, "| Option | Decided |"));
    let whisper = Whisper::new(h.dir.path().join("unused").join("CONTEXT.md"));
    let composed = whisper.compose(&h.storage).unwrap();
    let expected: Vec<(String, Option<String>)> = composed
        .shown
        .iter()
        .map(|s| {
            (
                s.id.clone(),
                Some(Section::of(s.section).unwrap().as_str().to_string()),
            )
        })
        .collect();
    assert!(expected.iter().any(|(id, _)| id == &a) && expected.iter().any(|(id, _)| id == &b));

    for entry in ["tools/call", "direct"] {
        h.clear();
        let (output, errors) = if entry == "tools/call" {
            h.call("memory_context", json!({}))
        } else {
            h.direct("memory_context", json!({}))
        };
        assert!(errors.is_empty(), "{errors}");
        assert!(
            !output.contains(&noise),
            "{entry}: the context shows no noise"
        );
        let (_, items) = h.only_event("memory_context");
        assert_eq!(items, expected, "{entry}");
        let numbers = &h.events()[0];
        assert_eq!(numbers.7, 1, "{entry}: one memory hidden");
        assert_eq!(numbers.5, 1, "{entry}: one correction");
        // The characters of the text sent: the context and what wraps it.
        assert!(
            numbers.6 as usize >= composed.text.chars().count(),
            "{entry}: chars"
        );
    }
}

#[test]
fn recall_log_facts_record_the_sources_of_the_values_through_the_tool_alone() {
    let h = Harness::new();
    let source = h.save(&captured(Capture::Decision, SQLITE));
    crate::facts::store::apply(
        &h.storage,
        &crate::facts::store::FactWrite {
            project: Some("demoapp"),
            subject: "Widget",
            predicate: "price",
            value: Some("six"),
            actor: "test",
            source_memory_id: Some(&source),
            ..Default::default()
        },
    )
    .unwrap();
    let (output, errors) = h.call("memory_facts", json!({"subject": "Widget"}));
    assert!(errors.is_empty(), "{errors}");
    assert!(output.contains("six"));
    let (_, items) = h.only_event("memory_facts");
    assert_eq!(ids(&items), [source.as_str()]);
    // There is no direct method for facts.
    h.clear();
    let (output, _) = h.direct("memory_facts", json!({"subject": "Widget"}));
    assert!(output.contains("error"));
    assert!(h.events().is_empty());
}

#[test]
fn recall_log_calls_that_are_not_recalls_write_nothing() {
    let h = Harness::new();
    h.save(&captured(Capture::Decision, SQLITE));
    let calls = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": "Claude Code"}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "tools/call",
            "params": {"name": "memory_save", "arguments": {"title": "a", "content": "b"}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "memory_save",
            "params": {"title": "c", "content": "d"}}),
        json!({"jsonrpc": "2.0", "id": 5, "method": "memory_status"}),
        json!({"jsonrpc": "2.0", "id": 6, "method": "tools/call",
            "params": {"name": "memory_followups", "arguments": {"project": "demoapp"}}}),
        json!({"jsonrpc": "2.0", "id": 7, "method": "tools/call",
            "params": {"name": "memory_fact_set", "arguments":
                {"subject": "Widget", "predicate": "price", "value": "six"}}}),
        // A request without an id gets no reply and no record.
        json!({"jsonrpc": "2.0", "method": "memory_recent", "params": {"limit": 5}}),
        // A reply that is an error has no record.
        json!({"jsonrpc": "2.0", "id": 8, "method": "memory_similar", "params": {}}),
        json!({"jsonrpc": "2.0", "id": 9, "method": "no/such/method"}),
    ];
    let lines: Vec<String> = calls.iter().map(Value::to_string).collect();
    let (output, errors) = h.serve(&lines);
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(output.lines().count(), 9, "{output}");
    assert!(h.events().is_empty(), "{:?}", h.events());
}

#[test]
fn recall_log_a_reply_that_could_not_be_written_has_no_record() {
    struct Broken;
    impl Write for Broken {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::other("the pipe is gone"))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let h = Harness::new();
    h.save(&captured(Capture::Decision, SQLITE));
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": "memory_recent",
        "params": {"limit": 5}})
    .to_string();
    let mut errors = Vec::new();
    let result = h.server.serve_store(
        Cursor::new(line + "\n").lines(),
        &mut Broken,
        &mut errors,
        &h.storage,
        &ConstEmbedder,
        &ImportanceScorer::default(),
        None,
        &h.sinks,
    );
    assert!(result.is_err());
    assert!(h.events().is_empty());
    assert!(errors.is_empty());
}

#[test]
fn recall_log_no_column_holds_a_text_of_a_memory_or_a_query() {
    let h = Harness::new();
    for text in [SQLITE, CACHE] {
        h.save(&captured(Capture::Decision, text));
    }
    h.save(&captured(Capture::Correction, ORDER));
    h.call(
        "memory_search",
        json!({"query": "SQLite index", "limit": 10}),
    );
    h.direct(
        "memory_similar",
        json!({"query": "the other parser", "limit": 10}),
    );
    h.call("memory_context", json!({}));
    h.call("memory_context", json!({"topic": "SQLite"}));
    assert_eq!(h.events().len(), 4);
    let log = h.log_text();
    for word in [
        "SQLite",
        "index",
        "cache",
        "parser",
        "Decision",
        "stop",
        "other",
        "memory-files",
    ] {
        assert!(!log.contains(word), "{word} is in the log: {log}");
    }
}

#[test]
fn recall_log_the_client_is_named_once_by_initialize() {
    let h = Harness::new();
    h.save(&captured(Capture::Decision, SQLITE));
    let credential = ["sk-", "proj-", &"a1B2c3D4e5F6".repeat(4)].concat();
    let lines: Vec<String> = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"clientInfo": {"name": credential}}}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "memory_recent", "params": {"limit": 5}}),
        json!({"jsonrpc": "2.0", "id": 3, "method": "initialize",
            "params": {"clientInfo": {"name": "Claude Code"}}}),
        json!({"jsonrpc": "2.0", "id": 4, "method": "memory_recent", "params": {"limit": 5}}),
        json!({"jsonrpc": "2.0", "id": 5, "method": "initialize",
            "params": {"clientInfo": {"name": "Another"}}}),
        json!({"jsonrpc": "2.0", "id": 6, "method": "memory_recent", "params": {"limit": 5}}),
    ]
    .iter()
    .map(Value::to_string)
    .collect();
    let (_, errors) = h.serve(&lines);
    assert!(errors.is_empty(), "{errors}");
    let clients: Vec<Option<String>> = h.events().into_iter().map(|e| e.3).collect();
    assert_eq!(
        clients,
        [
            None,
            Some("claude code".to_string()),
            Some("claude code".to_string())
        ]
    );
    assert!(!h.log_text().contains(&credential));
    // The run is one for the process.
    let conn = h.storage.conn.lock().unwrap();
    let runs: i64 = conn
        .query_row("SELECT COUNT(DISTINCT run) FROM recall_events", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(runs, 1);
}

#[test]
fn recall_log_a_locked_store_drops_the_record_after_the_reply_went_out() {
    let h = Harness::new();
    h.save(&captured(Capture::Decision, SQLITE));
    // Another connection holds the write lock until the record has been
    // tried. Reading is not blocked, so the reply is complete.
    let holder = rusqlite::Connection::open(h.dir.path().join("memory.db")).unwrap();
    holder.execute_batch("BEGIN IMMEDIATE").unwrap();
    let started = std::time::Instant::now();
    let (output, errors) = h.direct("memory_recent", json!({"limit": 5}));
    let waited = started.elapsed();
    holder.execute_batch("ROLLBACK").unwrap();
    assert!(output.contains(SQLITE), "the reply is complete: {output}");
    assert_eq!(errors, "mnemonic: a record of memory_recent was dropped\n");
    assert!(waited >= recall_log::LOCK_WAIT && waited < std::time::Duration::from_secs(5));
    assert!(h.events().is_empty());
}

#[test]
fn recall_log_a_topic_is_cut_to_what_the_embedder_takes() {
    let h = Harness::new();
    h.save(&captured(Capture::Decision, SQLITE));
    let embedder = crate::test_support::RecordingEmbedder::new();
    let topic = "index ".repeat(4000);
    assert!(topic.len() > EMBED_TEXT_MAX_BYTES);
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": "memory_context",
        "params": {"topic": topic}})
    .to_string();
    let mut output = Vec::new();
    let mut errors = Vec::new();
    h.server
        .serve_store(
            Cursor::new(line + "\n").lines(),
            &mut output,
            &mut errors,
            &h.storage,
            &embedder,
            &ImportanceScorer::default(),
            None,
            &h.sinks,
        )
        .unwrap();
    assert!(errors.is_empty(), "{}", String::from_utf8_lossy(&errors));
    let texts = embedder.texts();
    assert!(!texts.is_empty(), "the topic was embedded");
    assert!(
        texts.iter().all(|t| t.len() <= EMBED_TEXT_MAX_BYTES),
        "the topic was cut"
    );
    assert_eq!(h.events().len(), 1);
}

#[test]
fn recall_log_with_the_log_off_the_server_writes_nothing() {
    let h = Harness::with_log(false);
    h.save(&captured(Capture::Decision, SQLITE));
    let (output, errors) = h.direct("memory_recent", json!({"limit": 5}));
    assert!(output.contains(SQLITE));
    assert!(errors.is_empty());
    assert!(h.events().is_empty());
}

/// The handshake is answered before the store opens, and the client it
/// names is the client of every call after it: the stdio path, with the
/// store opened by the opener as the server opens it.
#[test]
fn recall_log_the_client_named_before_the_store_opens_is_recorded() {
    let h = Harness::new();
    h.save(&captured(Capture::Decision, SQLITE));
    let mut config = Config::default();
    config.storage.db_path = h.dir.path().join("memory.db");
    config.output.memory_files_enabled = false;
    config.output.obsidian_enabled = false;
    config.output.memory_api_enabled = false;
    config.peers.auto_tag = false;
    let db = config.storage.db_path.clone();
    let input = [
        json!({"jsonrpc": "2.0", "id": 1, "method": "initialize",
            "params": {"protocolVersion": "2024-11-05", "clientInfo": {"name": "Claude Code"}}}),
        json!({"jsonrpc": "2.0", "method": "notifications/initialized"}),
        json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call",
            "params": {"name": "memory_recent", "arguments": {"limit": 5}}}),
    ]
    .map(|line| line.to_string())
    .join("\n")
        + "\n";
    let mut output = Vec::new();
    McpServer::new(config)
        .serve(Cursor::new(input), &mut output, move || Storage::open(&db))
        .unwrap();
    let output = String::from_utf8(output).unwrap();
    assert!(output.contains("\"instructions\""), "{output}");
    assert!(output.contains("\"id\":2"), "{output}");
    let events = h.events();
    assert_eq!(events.len(), 1, "{events:?}");
    assert_eq!(events[0].2, "memory_recent");
    assert_eq!(events[0].3.as_deref(), Some("claude code"));
    assert_eq!(events[0].4, 1);
}
