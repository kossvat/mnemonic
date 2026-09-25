//! Acceptance for fact updates, end to end over the
//! MCP JSON-RPC surface an agent actually uses.
//!
//! Every text embeds to the same vector, so every pair is a near duplicate
//! (cosine 1.0) and a leg passes only because of the rule under test, never
//! because two texts happened to embed apart.
use super::*;
use crate::test_support::ConstEmbedder;

struct Harness {
    _dir: tempfile::TempDir,
    server: McpServer,
    storage: Storage,
    embedder: ConstEmbedder,
    scorer: ImportanceScorer,
    next_id: std::cell::Cell<u64>,
}

impl Harness {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mnemonic-fact-updates-");
        let mut config = Config::default();
        config.storage.db_path = dir.path().join("memory.db");
        config.output.memory_files_enabled = false;
        config.output.obsidian_enabled = false;
        config.output.memory_api_enabled = false;
        let storage = Storage::open(&config.storage.db_path).unwrap();
        Self {
            _dir: dir,
            server: McpServer::new(config),
            storage,
            embedder: ConstEmbedder,
            scorer: ImportanceScorer::default(),
            next_id: std::cell::Cell::new(1),
        }
    }

    /// Call one MCP tool and return its JSON result, or the error text.
    fn call(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        let id = self.next_id.get();
        self.next_id.set(id + 1);
        let line = json!({"jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}})
        .to_string();
        let out = self
            .server
            .response_for_line(
                &line,
                &self.storage,
                &self.embedder,
                &self.scorer,
                None,
                &[],
            )
            .expect("a request gets a reply");
        let reply: Value = serde_json::from_str(&out).unwrap();
        if let Some(error) = reply.get("error") {
            return Err(error.to_string());
        }
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        Ok(serde_json::from_str(text).unwrap())
    }

    fn save(&self, title: &str, content: &str) -> Value {
        self.call(
            "memory_save",
            json!({"title": title, "content": content, "memory_type": "note"}),
        )
        .unwrap()
    }

    fn set(&self, project: &str, subject: &str, predicate: &str, value: &str) -> Value {
        self.call(
            "memory_fact_set",
            json!({"project": project, "subject": subject, "predicate": predicate, "value": value}),
        )
        .unwrap()
    }

    fn facts(&self, project: &str, subject: &str) -> Value {
        self.call(
            "memory_facts",
            json!({"project": project, "subject": subject, "history": true}),
        )
        .unwrap()
    }

    /// The search hit for memory `id`, with whatever annotations recall adds.
    fn hit(&self, query: &str, id: &str) -> Value {
        let found = self
            .call("memory_search", json!({"query": query, "limit": 20}))
            .unwrap();
        found["results"]
            .as_array()
            .unwrap()
            .iter()
            .find(|hit| hit["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("{id} not in {found}"))
    }
}

fn saved_id(reply: &Value) -> String {
    assert_eq!(reply["status"], "saved", "{reply}");
    reply["id"].as_str().unwrap().to_owned()
}

/// The one current value of a slot as memory_facts reports it.
fn current(facts: &Value) -> Value {
    facts["facts"][0]["current"]["value"].clone()
}

fn trail(facts: &Value) -> Vec<String> {
    facts["facts"][0]["history"]
        .as_array()
        .unwrap()
        .iter()
        .map(|value| value["value"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn leg1_similar_price_saves_both_land_and_link() {
    let h = Harness::new();
    let old = saved_id(&h.save("Widget price", "Widget price is $5."));
    let reply = h.save("Widget price", "Widget price is now $6.");
    let new = saved_id(&reply);
    let link = &reply["updates"][0];
    assert_eq!(link["id"], old.as_str(), "{reply}");
    assert_eq!(link["was"], "$5", "{reply}");
    assert_eq!(link["now"], "$6", "{reply}");
    assert_ne!(old, new);
}

#[test]
fn leg2_other_subject_same_price_not_suppressed() {
    let h = Harness::new();
    saved_id(&h.save("Widget price", "Widget price is $5."));
    let reply = h.save("Gadget price", "Gadget price is $5.");
    saved_id(&reply);
    assert!(
        reply["updates"].as_array().is_none_or(Vec::is_empty),
        "{reply}"
    );
}

#[test]
fn leg3_fact_supersedes_with_history() {
    let h = Harness::new();
    assert_eq!(
        h.set("alpha-shop", "Widget", "price", "$5")["outcome"],
        "create"
    );
    let reply = h.set("alpha-shop", "Widget", "price", "$6");
    assert_eq!(reply["outcome"], "update", "{reply}");
    assert_eq!(reply["replaced"]["value"], "$5", "{reply}");
    let facts = h.facts("alpha-shop", "Widget");
    assert_eq!(current(&facts), "$6", "{facts}");
    assert_eq!(trail(&facts), vec!["$6", "$5"], "{facts}");
    // The old value was valid until the new one began.
    let history = &facts["facts"][0]["history"];
    assert_eq!(history[1]["valid_to"], history[0]["valid_from"], "{facts}");
}

#[test]
fn leg4_recall_marks_old_value_replaced() {
    let h = Harness::new();
    // Plain saves: the memory that was replaced says so, and by what.
    let old = saved_id(&h.save("Widget price", "Widget price is $5."));
    let new = saved_id(&h.save("Widget price", "Widget price is now $6."));
    let hit = h.hit("Widget price", &old);
    assert_eq!(hit["replaced_by"]["id"], new.as_str(), "{hit}");
    assert!(h.hit("Widget price", &new)["replaced_by"].is_null());

    // Declared facts: the source memory of the old value points at the fact's
    // current value.
    let first = h.set("alpha-shop", "Gadget", "price", "$7");
    h.set("alpha-shop", "Gadget", "price", "$8");
    let source = first["memory_id"].as_str().unwrap();
    let hit = h.hit("Gadget price", source);
    assert_eq!(hit["facts"][0]["current"], "$8", "{hit}");
    assert_eq!(hit["facts"][0]["this"], "$7", "{hit}");
}

#[test]
fn leg5_other_project_absent_from_scoped_recall() {
    let h = Harness::new();
    h.set("alpha-shop", "Widget", "price", "$5");
    h.set("beta-lab", "Widget", "price", "$9");
    assert_eq!(current(&h.facts("alpha-shop", "Widget")), "$5");
    assert_eq!(current(&h.facts("beta-lab", "Widget")), "$9");
    let facts = h.facts("alpha-shop", "Widget");
    assert!(!facts.to_string().contains("$9"), "{facts}");
}

#[test]
fn leg6_forget_cascades() {
    let h = Harness::new();
    // A declared value outlives its source memory, which only loses the link.
    h.set("alpha-shop", "Widget", "price", "$5");
    let reply = h.set("alpha-shop", "Widget", "price", "$6");
    let source = reply["memory_id"].as_str().unwrap().to_owned();
    assert!(h.storage.forget_by_id(&source).unwrap());
    let facts = h.facts("alpha-shop", "Widget");
    assert_eq!(current(&facts), "$6", "{facts}");
    assert!(
        facts["facts"][0]["current"]["source_memory_id"].is_null(),
        "{facts}"
    );

    // Forgetting the newer of two linked plain memories makes the older one
    // current again: nothing points at it as replaced any more.
    let old = saved_id(&h.save("Gadget price", "Gadget price is $5."));
    let new = saved_id(&h.save("Gadget price", "Gadget price is now $6."));
    assert!(h.storage.forget_by_id(&new).unwrap());
    assert!(h.hit("Gadget price", &old)["replaced_by"].is_null());
}

#[test]
fn control_exact_resave_skipped_with_duplicate_of() {
    let h = Harness::new();
    let first = saved_id(&h.save("Widget price", "Widget price is $5."));
    let reply = h.save("Widget price", "Widget price is $5.");
    assert_eq!(reply["status"], "skipped", "{reply}");
    assert_eq!(reply["duplicate_of"], first.as_str(), "{reply}");
}

#[test]
fn control_versions_ports_hashes_still_dedup() {
    for (first, second) in [
        ("Build uses version 1.2.3", "Build uses version 1.2.4"),
        (
            "Dev server listens on port 8080",
            "Dev server listens on port 8081",
        ),
        ("Deploy commit a1b2c3d", "Deploy commit e4f5a6b"),
    ] {
        let h = Harness::new();
        saved_id(&h.save("note", first));
        let reply = h.save("note", second);
        assert_eq!(reply["status"], "skipped", "{first} / {second}: {reply}");
    }
}

#[test]
fn revert_5_6_5_plain() {
    let h = Harness::new();
    saved_id(&h.save("Widget price", "Widget price is $5."));
    let six = saved_id(&h.save("Widget price", "Widget price is now $6."));
    // Back to $5: not a duplicate of the first memory, an update of the head.
    let reply = h.save("Widget price", "Widget price is back to $5.");
    saved_id(&reply);
    assert_eq!(reply["updates"][0]["id"], six.as_str(), "{reply}");
}

#[test]
fn revert_5_6_5_declared() {
    let h = Harness::new();
    h.set("alpha-shop", "Widget", "price", "$5");
    h.set("alpha-shop", "Widget", "price", "$6");
    let reply = h.set("alpha-shop", "Widget", "price", "$5");
    assert_eq!(reply["outcome"], "update", "{reply}");
    let facts = h.facts("alpha-shop", "Widget");
    assert_eq!(current(&facts), "$5");
    assert_eq!(trail(&facts), vec!["$5", "$6", "$5"], "{facts}");
}

#[test]
fn memory_facts_shows_the_last_few_values_unless_asked_for_all() {
    let h = Harness::new();
    for price in ["$1", "$2", "$3", "$4", "$5"] {
        h.set("alpha-shop", "Widget", "price", price);
    }
    let short = h
        .call(
            "memory_facts",
            json!({"project": "alpha-shop", "subject": "Widget"}),
        )
        .unwrap();
    assert_eq!(trail(&short), vec!["$5", "$4", "$3"], "{short}");
    assert_eq!(trail(&h.facts("alpha-shop", "Widget")).len(), 5);
}

#[test]
fn a_duplicate_save_records_when_the_value_was_said_again() {
    let h = Harness::new();
    let first = saved_id(&h.save("Widget price", "Widget price is $5."));
    let reply = h.save("Widget price", "Widget price is $5.");
    assert_eq!(reply["duplicate_of"], first.as_str(), "{reply}");
    let reaffirmed: i64 = h
        .storage
        .conn
        .lock()
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM memory_reaffirmed WHERE memory_id = ?1",
            [&first],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(reaffirmed, 1);
}

#[test]
fn leg7_legacy_rows_migrate_intact() {
    let dir = crate::test_support::temp_dir("mnemonic-fact-legacy-leg7-");
    let path = dir.path().join("memory.db");
    drop(Storage::open(&path).unwrap());
    let seeded = {
        let conn = rusqlite::Connection::open(&path).unwrap();
        crate::test_support::legacy_facts_fixture(&conn)
    };
    let storage = Storage::open(&path).unwrap();
    let facts = crate::facts::store::current_all(&storage).unwrap();
    assert_eq!(facts.len(), seeded.current.len());
    for (_, _, value) in &seeded.current {
        assert!(
            facts.iter().any(|f| &f.value == value),
            "{value} not migrated"
        );
    }
    assert_eq!(
        crate::facts::store::value_count(&storage).unwrap(),
        seeded.rows
    );
}
