//! The marks of noise in what the MCP tools return: a capture the rules
//! name says so, other items carry no such field, and what is returned is
//! what was returned before.
use super::*;
use crate::quality::Capture;
use crate::test_support::{ConstEmbedder, captured};

struct Harness {
    _dir: tempfile::TempDir,
    storage: Storage,
    server: McpServer,
}

impl Harness {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mn-mcp-quality-");
        let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
        Self {
            _dir: dir,
            storage,
            server: McpServer::new(Config::default()),
        }
    }

    fn save(&self, entry: &MemoryEntry) {
        let vector = ConstEmbedder.embed("").unwrap();
        self.storage
            .save_with_embedding(entry, Some(&vector))
            .unwrap();
    }
}

fn results(reply: &Value) -> Vec<&Value> {
    reply["results"].as_array().unwrap().iter().collect()
}

fn mark<'a>(items: &[&'a Value], id: &str) -> Option<&'a Value> {
    items
        .iter()
        .find(|item| item["id"] == id)
        .map(|item| item.get("noise").unwrap_or(&Value::Null))
}

#[test]
fn context_quality_mcp_replies_mark_noise_and_return_what_they_returned() {
    let h = Harness::new();
    let noise = captured(Capture::Decision, "| Option | SQLite | Decided |");
    let clean = captured(Capture::Decision, "Decision: use SQLite for the index");
    let order = captured(Capture::Correction, "| stop | SQLite |");
    let saved = MemoryEntry::new(
        "| Option | SQLite | Decided |",
        "| Option | SQLite | Decided |",
        MemoryType::Decision,
        EventSource::Manual,
    );
    for entry in [&noise, &clean, &order, &saved] {
        h.save(entry);
    }

    let search = h
        .server
        .handle_search(&json!({"query": "SQLite", "limit": 10}), &h.storage)
        .unwrap();
    let items = results(&search);
    assert_eq!(items.len(), 4, "{search}");
    assert_eq!(mark(&items, &noise.id), Some(&json!("table-row")));
    assert_eq!(mark(&items, &order.id), Some(&json!("table-row")));
    assert_eq!(mark(&items, &clean.id), Some(&Value::Null));
    assert_eq!(mark(&items, &saved.id), Some(&Value::Null));

    let recent = h
        .server
        .handle_recent(&json!({"limit": 10}), &h.storage)
        .unwrap();
    let items = results(&recent);
    assert_eq!(items.len(), 4);
    assert_eq!(mark(&items, &noise.id), Some(&json!("table-row")));
    assert_eq!(mark(&items, &clean.id), Some(&Value::Null));

    let similar = h
        .server
        .handle_similar(
            &json!({"query": "SQLite", "limit": 10}),
            &h.storage,
            &ConstEmbedder,
        )
        .unwrap();
    let items = results(&similar);
    assert_eq!(items.len(), 4, "{similar}");
    assert_eq!(mark(&items, &order.id), Some(&json!("table-row")));
    assert_eq!(mark(&items, &saved.id), Some(&Value::Null));
    for item in &items {
        assert!(item.get("similarity").is_some());
    }
}
