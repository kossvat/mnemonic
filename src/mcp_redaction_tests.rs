//! Explicit saves over the MCP surface an agent uses: what the store, the
//! embedder and the sinks receive. Credential fixtures are assembled at
//! run time; assertions never print them.
use std::sync::{Arc, Mutex};

use super::*;
use crate::redaction::SUMMARY_KEY;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

/// Remembers every text it was asked to embed.
#[derive(Default)]
struct RecordingEmbedder(Mutex<Vec<String>>);

impl Embedder for RecordingEmbedder {
    fn embed(&self, text: &str) -> anyhow::Result<crate::embedding::Embedding> {
        self.0.lock().unwrap().push(text.to_string());
        let mut vector = vec![0.0; crate::embedding::EMBED_DIMS];
        vector[0] = 1.0;
        Ok(vector)
    }

    fn model_id(&self) -> &'static str {
        "recording-test"
    }
}

struct RecordingSink(Arc<Mutex<Vec<MemoryEntry>>>);

impl OutputSink for RecordingSink {
    fn write(&self, entry: &MemoryEntry) -> anyhow::Result<()> {
        self.0.lock().unwrap().push(entry.clone());
        Ok(())
    }

    fn name(&self) -> &str {
        "recording"
    }
}

struct Harness {
    _dir: tempfile::TempDir,
    server: McpServer,
    storage: Storage,
    embedder: RecordingEmbedder,
    sinks: Vec<Box<dyn OutputSink>>,
    written: Arc<Mutex<Vec<MemoryEntry>>>,
}

impl Harness {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mnemonic-redaction-mcp-");
        let mut config = Config::default();
        config.storage.db_path = dir.path().join("memory.db");
        config.output.memory_files_enabled = false;
        config.output.obsidian_enabled = false;
        config.output.memory_api_enabled = false;
        config.output.memory_files_path = dir.path().join("memory-files");
        config.output.obsidian_path = dir.path().join("obsidian");
        let storage = Storage::open(&config.storage.db_path).unwrap();
        let written = Arc::new(Mutex::new(Vec::new()));
        Self {
            _dir: dir,
            server: McpServer::new(config),
            storage,
            embedder: RecordingEmbedder::default(),
            sinks: vec![Box::new(RecordingSink(written.clone()))],
            written,
        }
    }

    fn call(&self, tool: &str, arguments: Value) -> Result<Value, String> {
        let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
            "params": {"name": tool, "arguments": arguments}})
        .to_string();
        let out = self
            .server
            .response_for_line(
                &line,
                &self.storage,
                &self.embedder,
                &ImportanceScorer::default(),
                None,
                &self.sinks,
            )
            .expect("a request gets a reply");
        let reply: Value = serde_json::from_str(&out).unwrap();
        if let Some(error) = reply.get("error") {
            return Err(error.to_string());
        }
        let text = reply["result"]["content"][0]["text"].as_str().unwrap();
        Ok(serde_json::from_str(text).unwrap())
    }

    fn embedded(&self) -> String {
        self.embedder.0.lock().unwrap().join("\n")
    }

    fn stored_text(&self) -> String {
        let conn = self.storage.conn.lock().unwrap();
        let mut out = String::new();
        for sql in [
            "SELECT title || ' ' || content || ' ' || tags || ' ' || metadata FROM memories",
            "SELECT title || ' ' || content || ' ' || tags FROM memories_fts",
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

#[test]
fn redaction_memory_mcp_save_stores_embeds_and_exports_prepared_text_only() {
    let token = token();
    let h = Harness::new();
    let reply = h
        .call(
            "memory_save",
            json!({
                "title": format!("deploy key {token}"),
                "content": format!("не так, use {token} <private>and call 555-0100</private>"),
                "memory_type": "feedback",
                "tags": format!("deploy, tag-{token}"),
                "project": "demoapp",
            }),
        )
        .unwrap();
    assert!(reply["status"] == "saved");
    assert!(reply["redaction"]["counts"]["provider_token"] == 3);
    assert!(reply["redaction"]["counts"]["private_block"] == 1);
    assert!(reply["project"] == "demoapp");

    let stored = h.stored_text();
    assert!(!stored.contains(&token) && !stored.contains("555"));
    assert!(stored.contains("[REDACTED:credential]") && stored.contains("[REDACTED:private]"));
    let embedded = h.embedded();
    assert!(!embedded.is_empty(), "the save was embedded");
    assert!(!embedded.contains(&token) && !embedded.contains("555"));
    let written = h.written.lock().unwrap();
    assert!(written.len() == 1, "the sink saw the save");
    let sunk = format!(
        "{} {} {:?}",
        written[0].title, written[0].content, written[0].tags
    );
    assert!(!sunk.contains(&token) && !sunk.contains("555"));
    assert!(written[0].metadata[SUMMARY_KEY]["counts"]["provider_token"] == 3);
}

/// A private region may span the comma that splits tags: the tag string
/// is redacted whole before it is split, so no fragment survives.
#[test]
fn redaction_memory_mcp_tags_are_redacted_before_they_are_split() {
    let h = Harness::new();
    let reply = h
        .call(
            "memory_save",
            json!({"title": "t", "content": "plain content here",
                   "tags": "public,<private>one,two</private>,after"}),
        )
        .unwrap();
    assert!(reply["status"] == "saved");
    assert!(reply["redaction"]["counts"]["private_block"] == 1);
    let stored = h.storage.recent(1).unwrap();
    assert!(
        stored[0].tags
            == vec![
                "public".to_string(),
                "[REDACTED:private]".to_string(),
                "after".to_string()
            ]
    );
    let text = h.stored_text();
    assert!(!text.contains("one") && !text.contains("two"));
}

#[test]
fn redaction_memory_mcp_save_refuses_a_sensitive_project_before_any_effect() {
    let token = token();
    let h = Harness::new();
    let err = h
        .call(
            "memory_save",
            json!({"title": "t", "content": "plain content here", "project": format!("p-{token}")}),
        )
        .unwrap_err();
    assert!(err.contains("SENSITIVE_CONTENT"));
    assert!(!err.contains(&token), "the error names the project");
    assert!(h.embedded().is_empty(), "nothing was embedded");
    assert!(h.written.lock().unwrap().is_empty(), "no sink was written");
    assert!(h.stored_text().is_empty());
}

/// A fact whose value is a credential fails whole, before the embedder or
/// any table sees it.
#[test]
fn redaction_memory_mcp_fact_set_with_a_credential_writes_nothing() {
    let token = token();
    let h = Harness::new();
    let err = h
        .call(
            "memory_fact_set",
            json!({"project": "demoapp", "subject": "api", "predicate": "key", "value": token}),
        )
        .unwrap_err();
    assert!(err.contains("SENSITIVE_CONTENT"));
    assert!(!err.contains(&token));
    assert!(h.embedder.0.lock().unwrap().is_empty(), "it was embedded");
    let conn = h.storage.conn.lock().unwrap();
    for table in ["memories", "fact_values", "fact_slots", "fact_events"] {
        let n: i64 = conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert!(n == 0, "{table} was written");
    }
}

/// A safe fact with a sensitive note: the fact stands as stated, the note
/// is prepared, and the reply says what was redacted.
#[test]
fn redaction_state_mcp_fact_note_is_prepared_and_the_fact_stands() {
    let value = body(32);
    let h = Harness::new();
    let reply = h
        .call(
            "memory_fact_set",
            json!({"project": "demoapp", "subject": "api", "predicate": "host",
                   "value": "db.internal", "note": format!("rotate it, password={value}")}),
        )
        .unwrap();
    assert!(reply["outcome"] == "create");
    assert!(reply["fact"]["current"]["value"] == "db.internal");
    assert!(reply["redaction"]["counts"]["credential_assignment"] == 1);
    assert!(!reply.to_string().contains(&value));
    assert!(!h.stored_text().contains(&value));
    assert!(h.stored_text().contains("[REDACTED:credential]"));
    let embedded = h.embedder.0.lock().unwrap();
    assert!(!embedded.is_empty() && embedded.iter().all(|t| !t.contains(&value)));
}

/// A follow-up's title is narrative, what names it is not; the action of an
/// update is input too and is not echoed.
#[test]
fn redaction_state_mcp_followups_prepare_titles_and_refuse_identities() {
    let token = token();
    let h = Harness::new();
    let opened = h
        .call(
            "memory_followup_open",
            json!({"project": "demoapp", "title": format!("rotate {token} before friday")}),
        )
        .unwrap();
    let title = opened["followup"]["title"].as_str().unwrap();
    assert!(title == "rotate [REDACTED:credential] before friday");
    let id = opened["followup"]["id"].as_str().unwrap().to_string();
    for arguments in [
        json!({"project": format!("p-{token}"), "title": "plain"}),
        json!({"project": "demoapp", "title": "plain", "request_id": token}),
    ] {
        let err = h.call("memory_followup_open", arguments).unwrap_err();
        assert!(
            err.contains("SENSITIVE_CONTENT") && !err.contains(&token),
            "refused"
        );
    }
    for arguments in [
        json!({"id": id, "action": token}),
        json!({"id": token, "action": "close"}),
        json!({"id": id, "action": "close", "project": token}),
        json!({"id": id, "action": "close", "evidence_memory_id": token}),
        json!({"id": id, "action": "close", "request_id": token}),
    ] {
        let err = h.call("memory_followup_update", arguments).unwrap_err();
        assert!(!err.contains(&token), "the error echoes its input");
    }
    let conn = h.storage.conn.lock().unwrap();
    let (rows, open, events): (i64, i64, i64) = conn
        .query_row(
            "SELECT (SELECT count(*) FROM followups),
                    (SELECT count(*) FROM followups WHERE status = 'open'),
                    (SELECT count(*) FROM followup_events)",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .unwrap();
    assert!((rows, open, events) == (1, 1, 1));
}

/// Every text embeds to the same vector here, so a second save is a
/// duplicate: the skipped reply still says what admission changed.
#[test]
fn redaction_memory_mcp_duplicate_reply_reports_the_redaction() {
    let token = token();
    let h = Harness::new();
    let first = h
        .call(
            "memory_save",
            json!({"title": "t", "content": "use SQLite for the index"}),
        )
        .unwrap();
    assert!(first["status"] == "saved");
    let second = h
        .call(
            "memory_save",
            json!({"title": "t", "content": format!("the key is {token}")}),
        )
        .unwrap();
    assert!(second["status"] == "skipped");
    assert!(second["redaction"]["counts"]["provider_token"] == 1);
    assert!(!h.stored_text().contains(&token));
}

/// Resolving a project name to an existing canonical one can drop the
/// syntax that made it a credential: `password=<v>` resolves to a project
/// entity `password-<v>` (extraction creates entities in that form), and
/// the resolved name passes the structural check. The raw name is judged
/// before it is resolved.
#[test]
fn redaction_memory_mcp_project_is_judged_before_it_is_resolved() {
    use crate::graph::{Entity, EntityType};
    let value = body(32).to_lowercase();
    let h = Harness::new();
    h.storage
        .upsert_entity(&Entity {
            name: format!("password-{value}"),
            entity_type: EntityType::Project,
        })
        .unwrap();
    // The seed is what the raw name resolves to.
    assert!(
        crate::followups::resolve_project(&h.storage, &format!("password={value}")).unwrap()
            == format!("password-{value}")
    );
    let err = h
        .call(
            "memory_save",
            json!({"title": "t2", "content": "another plain note here", "project": format!("password={value}")}),
        )
        .unwrap_err();
    assert!(err.contains("SENSITIVE_CONTENT"));
    assert!(!err.contains(&value), "the error names the project");
    assert!(h.storage.count().unwrap() == 0, "the save was stored");
}

#[test]
fn redaction_memory_mcp_clean_save_is_unchanged() {
    let h = Harness::new();
    let reply = h
        .call(
            "memory_save",
            json!({"title": "Decision", "content": "use SQLite for the index", "tags": "db"}),
        )
        .unwrap();
    assert!(reply["status"] == "saved");
    assert!(reply.get("redaction").is_none());
    let stored = h.storage.recent(1).unwrap();
    assert!(stored[0].content == "use SQLite for the index");
    assert!(stored[0].metadata.get(SUMMARY_KEY).is_none());
}

/// A query is new text: the model is given the prepared one, cut to what
/// it takes, and the reply says nothing of it.
#[test]
fn redaction_generated_mcp_similar_embeds_the_prepared_query() {
    let token = token();
    let h = Harness::new();
    let reply = h
        .call(
            "memory_similar",
            json!({"query": format!("where is {token} kept")}),
        )
        .unwrap();
    assert!(reply["count"] == 0);
    let embedded = h.embedded();
    assert!(!embedded.contains(&token));
    assert!(embedded.contains(crate::redaction::CREDENTIAL_MARKER));

    // A cut that ends inside a token leaves no part of it.
    let long = format!("{}{token}", "x ".repeat(EMBED_TEXT_MAX_BYTES / 2 - 10));
    h.call("memory_similar", json!({"query": long})).unwrap();
    assert!(!h.embedded().contains(&token[..20]));

    // A cut can make a credential of what was code: the call that made
    // `password=f(x)` a call is what the cut takes away.
    let value = body(24);
    let code = format!("password={value}(x)");
    assert!(crate::redaction::is_clean(&code));
    let padding = EMBED_TEXT_MAX_BYTES - "password=".len() - value.len();
    let long = format!("y{}{code}", "x ".repeat((padding - 1) / 2));
    assert!(long.len() == EMBED_TEXT_MAX_BYTES + "(x)".len());
    h.call("memory_similar", json!({"query": long})).unwrap();
    assert!(!h.embedded().contains(&value));
}

/// Every file under `dir`, as (path, bytes).
fn files_under(dir: &std::path::Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                let bytes = std::fs::read(&path).unwrap();
                out.push((path.to_string_lossy().into_owned(), bytes));
            }
        }
    }
    out
}

fn holds(bytes: &[u8], text: &str) -> bool {
    bytes
        .windows(text.len())
        .any(|window| window == text.as_bytes())
}

/// A harness whose sinks are the ones a configuration builds, writing
/// under the harness's own directory, beside the recording one.
fn harness_with_file_sinks() -> Harness {
    let mut h = Harness::new();
    let dir = h._dir.path().to_path_buf();
    h.sinks
        .push(Box::new(crate::output::memory_files::MemoryFileSink::new(
            dir.join("memory-files"),
        )));
    h.sinks
        .push(Box::new(crate::output::obsidian::ObsidianSink::new(
            dir.join("obsidian"),
        )));
    h
}

/// Every copy of an accepted save is of the prepared entry: the reply,
/// the row, the index, what the model was given, and what each sink
/// wrote, in a file's text and in its name. The summary the reply shows
/// is the one that is stored, and counts what one preparation found.
#[test]
fn redaction_outputs_mcp_save_every_copy_is_of_the_prepared_entry() {
    let token = token();
    let h = harness_with_file_sinks();
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
    "name": "memory_save", "arguments": {
        "title": format!("deploy key {token}"),
        "content": format!("we keep {token} and {token} <private>and a plan</private>"),
        "memory_type": "decision",
        "tags": "deploy",
        "project": "demoapp",
    }}})
    .to_string();
    let reply = h
        .server
        .response_for_line(
            &line,
            &h.storage,
            &h.embedder,
            &ImportanceScorer::default(),
            None,
            &h.sinks,
        )
        .unwrap();
    assert!(!reply.contains(&token) && !reply.contains("and a plan"));
    let reply: Value = serde_json::from_str(&reply).unwrap();
    let reply: Value =
        serde_json::from_str(reply["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert!(reply["status"] == "saved");

    let stored = h.stored_text();
    assert!(!stored.contains(&token) && !stored.contains("and a plan"));
    assert!(!h.embedded().contains(&token) && !h.embedded().is_empty());
    assert!(h.written.lock().unwrap().len() == 1);

    let files = files_under(h._dir.path());
    let written: Vec<&(String, Vec<u8>)> = files
        .iter()
        .filter(|(path, _)| path.contains("memory-files") || path.contains("obsidian"))
        .collect();
    assert!(written.len() >= 2, "a sink wrote no file");
    for (path, bytes) in &files {
        assert!(!path.contains(&token), "a file is named after the token");
        assert!(!holds(bytes, &token), "a file holds the token");
        assert!(!holds(bytes, "and a plan"), "a file holds the private text");
    }
    assert!(
        written
            .iter()
            .all(|(_, bytes)| holds(bytes, crate::redaction::CREDENTIAL_MARKER))
    );

    // One preparation, counted once: three tokens and one private block.
    let kept = h.storage.recent(1).unwrap();
    assert!(kept[0].metadata[SUMMARY_KEY] == reply["redaction"]);
    assert!(reply["redaction"]["counts"]["provider_token"] == 3);
    assert!(reply["redaction"]["counts"]["private_block"] == 1);
    assert!(reply["redaction"]["policy_version"] == crate::redaction::POLICY_VERSION);
}

/// A save the store does not take reaches no sink.
#[test]
fn redaction_outputs_mcp_failed_commit_reaches_no_sink() {
    let token = token();
    let h = harness_with_file_sinks();
    h.storage
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_every_memory BEFORE INSERT ON memories
             BEGIN SELECT RAISE(ABORT, 'the store takes nothing'); END",
        )
        .unwrap();
    let error = h
        .call(
            "memory_save",
            json!({"title": format!("deploy key {token}"), "content": "plain content here"}),
        )
        .unwrap_err();
    assert!(!error.contains(&token), "the error says the title back");
    assert!(h.written.lock().unwrap().is_empty(), "a sink was written");
    assert!(h.stored_text().is_empty());
    let written = files_under(h._dir.path())
        .into_iter()
        .filter(|(path, _)| path.contains("memory-files") || path.contains("obsidian"))
        .count();
    assert!(written == 0, "a sink wrote a file");
    assert!(!h.embedded().contains(&token));
}

/// What a reply says of a request it cannot serve is prepared like any
/// text: a method, a tool or an id that was given is not said back when
/// the policy refuses it, and a line that does not parse is not quoted.
#[test]
fn redaction_outputs_mcp_replies_do_not_say_back_what_the_policy_refuses() {
    let token = token();
    let h = Harness::new();
    let lines = [
        json!({"jsonrpc": "2.0", "id": 1, "method": format!("memory_{token}")}).to_string(),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": format!("tool_{token}"), "arguments": {}}})
        .to_string(),
        json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
               "params": {"name": "memory_followup_update",
                          "arguments": {"id": token, "action": "done"}}})
        .to_string(),
        format!("{{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": {token}"),
        format!("{{\"jsonrpc\": \"2.0\", \"id\": 1, \"method\": 7, \"params\": \"{token}\"}}"),
    ];
    for line in lines {
        let reply = h
            .server
            .response_for_line(
                &line,
                &h.storage,
                &h.embedder,
                &ImportanceScorer::default(),
                None,
                &h.sinks,
            )
            .unwrap();
        assert!(!reply.contains(&token), "the reply says the request back");
        let reply: Value = serde_json::from_str(&reply).unwrap();
        assert!(
            reply["error"]["message"]
                .as_str()
                .is_some_and(|m| !m.is_empty())
        );
    }
    // A name the policy admits is said, as it was.
    let line = json!({"jsonrpc": "2.0", "id": 1, "method": "memory_nothing"}).to_string();
    let reply = h
        .server
        .response_for_line(
            &line,
            &h.storage,
            &h.embedder,
            &ImportanceScorer::default(),
            None,
            &h.sinks,
        )
        .unwrap();
    assert!(reply.contains("Unknown method: memory_nothing"));
}

/// A save is embedded by its title and its content together, cut to what
/// the model takes: what the model is given is prepared as it is given.
#[test]
fn redaction_outputs_mcp_save_embeds_the_entry_as_it_reads() {
    let value = body(32);
    let h = Harness::new();
    let reply = h
        .call(
            "memory_save",
            json!({"title": "Deploy password:", "content": format!("{value} for staging")}),
        )
        .unwrap();
    assert!(reply["status"] == "saved");
    assert!(reply["redaction"]["counts"]["credential_assignment"] == 1);
    assert!(!h.embedded().contains(&value) && !h.stored_text().contains(&value));
    assert!(h.embedded().contains(crate::redaction::CREDENTIAL_MARKER));

    // A cut that takes away what made a call of an assignment.
    let h = Harness::new();
    let code = format!("password={value}(x)");
    let title = "t";
    let padding = EMBED_TEXT_MAX_BYTES - title.len() - 1 - "password=".len() - value.len();
    let content = format!("y{}{code}", "x ".repeat((padding - 1) / 2));
    assert!((title.len() + 1 + content.len()) == EMBED_TEXT_MAX_BYTES + "(x)".len());
    h.call("memory_save", json!({"title": title, "content": content}))
        .unwrap();
    assert!(!h.embedded().is_empty() && !h.embedded().contains(&value));
    // The memory keeps the code it was given.
    assert!(h.stored_text().contains(&code));
}

/// A name that names nothing is said back unless the policy refuses it.
#[test]
fn redaction_outputs_mcp_graph_does_not_say_back_a_refused_name() {
    let token = token();
    let h = Harness::new();
    let reply = h.call("memory_graph", json!({"entity": token})).unwrap();
    assert!(reply["found"] == false);
    assert!(!reply.to_string().contains(&token));
    let reply = h
        .call("memory_graph", json!({"entity": "demoapp"}))
        .unwrap();
    assert!(reply["found"] == false && reply["entity"] == "demoapp");
}
