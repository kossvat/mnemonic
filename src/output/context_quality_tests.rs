//! The generated context without noise: what the rules name is left out
//! of every section, is reported instead, and nothing in the store
//! changes. Every store here is generated in a scratch folder.
use chrono::Utc;

use super::whisper::{Composed, Whisper, hidden_report};
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::graph::{Entity, EntityType};
use crate::quality::{Capture, HiddenReason, Noise};
use crate::storage::Storage;
use crate::test_support::{ConstEmbedder, captured};

const ROW: &str = "| Option | Decided | Owner |";
const ORDER_ROW: &str = "| stop | reason |";
const CLEAN: &str = "Decision: use SQLite for the index";
const CLEAN_ORDER: &str = "stop, use the other parser";

struct Store {
    dir: tempfile::TempDir,
    storage: Storage,
}

impl Store {
    fn new() -> Self {
        let dir = crate::test_support::temp_dir("mn-context-quality-");
        let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
        Self { dir, storage }
    }

    fn whisper(&self) -> Whisper {
        Whisper::new(self.dir.path().join("out").join("CONTEXT.md"))
    }

    fn context_file(&self) -> std::path::PathBuf {
        self.dir.path().join("out").join("CONTEXT.md")
    }

    fn save(&self, entry: &MemoryEntry) -> String {
        self.storage.save(entry).unwrap();
        entry.id.clone()
    }

    fn capture(&self, kind: Capture, text: &str) -> String {
        self.save(&captured(kind, text))
    }

    fn compose(&self) -> Composed {
        let composed = self.whisper().compose(&self.storage).unwrap();
        assert!(!self.context_file().exists(), "compose writes no file");
        composed
    }

    fn project(&self, name: &str) -> String {
        self.storage
            .upsert_entity(&Entity {
                name: name.into(),
                entity_type: EntityType::Project,
            })
            .unwrap()
    }

    fn link(&self, id: &str, entity: &str) {
        self.storage.link_memory_entity(id, entity).unwrap();
    }

    fn confirm(&self, old: &str, new: &str) {
        self.storage
            .upsert_conflict(old, new, "p", "confirmed", Some(0.9), Some("r"))
            .unwrap();
    }

    /// The rows of memories, as they are, for a comparison before and
    /// after a composition.
    fn rows(&self) -> String {
        let conn = self.storage.conn.lock().unwrap();
        let mut stmt = conn
            .prepare(
                "SELECT id, title, content, tags, importance, COALESCE(access_count, 0),
                        COALESCE(last_accessed_at, '') FROM memories ORDER BY id",
            )
            .unwrap();
        let rows = stmt
            .query_map([], |r| {
                Ok(format!(
                    "{}|{}|{}|{}|{}|{}|{}",
                    r.get::<_, String>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                    r.get::<_, f64>(4)?,
                    r.get::<_, i64>(5)?,
                    r.get::<_, String>(6)?,
                ))
            })
            .unwrap();
        rows.map(|r| r.unwrap()).collect::<Vec<_>>().join("\n")
    }
}

fn hidden_of<'a>(composed: &'a Composed, id: &str) -> Option<&'a HiddenReason> {
    composed
        .hidden
        .iter()
        .find(|h| h.id == id)
        .map(|h| &h.reason)
}

fn shown_ids(composed: &Composed) -> Vec<&str> {
    composed.shown.iter().map(|s| s.id.as_str()).collect()
}

#[test]
fn context_quality_noise_is_out_of_every_section_and_listed() {
    let store = Store::new();
    let row = store.capture(Capture::Decision, ROW);
    let clean = store.capture(Capture::Decision, CLEAN);
    let order_row = store.capture(Capture::Correction, ORDER_ROW);
    let clean_order = store.capture(Capture::Correction, CLEAN_ORDER);
    let question = store.capture(Capture::Decision, "Are we going with the second option?");

    let composed = store.compose();
    assert!(composed.text.contains(CLEAN), "{}", composed.text);
    assert!(composed.text.contains(CLEAN_ORDER), "{}", composed.text);
    for hidden in [ROW, ORDER_ROW, "Are we going with the second option?"] {
        assert!(!composed.text.contains(hidden), "{}", composed.text);
    }
    assert_eq!(
        hidden_of(&composed, &row),
        Some(&HiddenReason::Noise(Noise::TableRow))
    );
    assert_eq!(
        hidden_of(&composed, &order_row),
        Some(&HiddenReason::Noise(Noise::TableRow))
    );
    assert_eq!(
        hidden_of(&composed, &question),
        Some(&HiddenReason::Noise(Noise::Question))
    );
    assert_eq!(composed.hidden.len(), 3);
    let shown = shown_ids(&composed);
    assert!(shown.contains(&clean.as_str()) && shown.contains(&clean_order.as_str()));
    assert!(!shown.contains(&row.as_str()));
    let sections: Vec<&str> = composed.shown.iter().map(|s| s.section).collect();
    assert!(sections.contains(&"decision") && sections.contains(&"feedback"));
}

#[test]
fn context_quality_a_replacement_that_is_noise_hides_both() {
    let store = Store::new();
    let old = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    let new = store.capture(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    store.confirm(&old, &new);

    let composed = store.compose();
    assert!(
        !composed.text.contains("Postgres for the ledger"),
        "{}",
        composed.text
    );
    assert!(
        !composed.text.contains("SQLite for the ledger"),
        "{}",
        composed.text
    );
    assert_eq!(
        hidden_of(&composed, &old),
        Some(&HiddenReason::ReplacedByNoise {
            replacement: new.clone()
        })
    );
    assert_eq!(
        hidden_of(&composed, &new),
        Some(&HiddenReason::Noise(Noise::TableRow))
    );
    assert!(shown_ids(&composed).is_empty());
}

#[test]
fn context_quality_a_chain_ends_in_noise_and_nothing_of_it_is_shown() {
    let store = Store::new();
    let a = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    let b = store.capture(Capture::Decision, "Decision: use MySQL for the ledger");
    let c = store.capture(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    store.confirm(&a, &b);
    store.confirm(&b, &c);

    let composed = store.compose();
    assert!(
        !composed.text.contains("for the ledger"),
        "{}",
        composed.text
    );
    assert_eq!(
        hidden_of(&composed, &a),
        Some(&HiddenReason::ReplacedByNoise {
            replacement: c.clone()
        })
    );
    assert_eq!(
        hidden_of(&composed, &b),
        Some(&HiddenReason::ReplacedByNoise {
            replacement: c.clone()
        })
    );
    assert_eq!(
        hidden_of(&composed, &c),
        Some(&HiddenReason::Noise(Noise::TableRow))
    );
}

#[test]
fn context_quality_two_replacements_of_one_decision() {
    // One clean and one noise: the clean one is shown, the old one is
    // history and is not listed.
    let store = Store::new();
    let a = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    let b = store.capture(Capture::Decision, "Decision: use MySQL for the ledger");
    let c = store.capture(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    store.confirm(&a, &b);
    store.confirm(&a, &c);
    let composed = store.compose();
    assert!(
        composed.text.contains("MySQL for the ledger"),
        "{}",
        composed.text
    );
    assert!(
        !composed.text.contains("Postgres for the ledger"),
        "{}",
        composed.text
    );
    assert_eq!(hidden_of(&composed, &a), None);
    assert_eq!(
        hidden_of(&composed, &c),
        Some(&HiddenReason::Noise(Noise::TableRow))
    );
    assert_eq!(shown_ids(&composed), [b.as_str()]);

    // Both noise: the old one is replaced by noise.
    let store = Store::new();
    let a = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    let b = store.capture(
        Capture::Decision,
        "| Decision: | use MySQL for the ledger |",
    );
    let c = store.capture(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    store.confirm(&a, &b);
    store.confirm(&a, &c);
    let composed = store.compose();
    assert!(shown_ids(&composed).is_empty(), "{}", composed.text);
    assert!(matches!(
        hidden_of(&composed, &a),
        Some(HiddenReason::ReplacedByNoise { replacement }) if *replacement == b || *replacement == c
    ));
}

#[test]
fn context_quality_a_replacement_outside_the_pool_behaves_as_before() {
    let store = Store::new();
    // The replacement is the oldest memory, and 200 newer ones push it out
    // of the pool of the newest 200.
    let mut new = captured(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    new.timestamp = Utc::now() - chrono::Duration::days(400);
    store.save(&new);
    for i in 0..200 {
        let mut filler = MemoryEntry::new(
            format!("Note {i}"),
            "a note",
            MemoryType::Note,
            EventSource::Socket,
        );
        filler.timestamp = Utc::now() - chrono::Duration::days(300) + chrono::Duration::minutes(i);
        store.save(&filler);
    }
    let old = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    store.confirm(&old, &new.id);

    let composed = store.compose();
    assert!(
        composed.text.contains("Postgres for the ledger"),
        "the old decision is kept, as today: {}",
        composed.text
    );
    assert_eq!(hidden_of(&composed, &old), None);
    assert!(composed.hidden.is_empty(), "{:?}", composed.hidden);
}

#[test]
fn context_quality_a_project_of_noise_alone_has_no_digest_unless_it_has_facts() {
    let store = Store::new();
    let entity = store.project("demoapp");
    for text in [ROW, "| Step | Decided |\n| 1 | yes |"] {
        let id = store.capture(Capture::Decision, text);
        store.link(&id, &entity);
    }
    let now = Utc::now();
    assert!(
        crate::digest::build_project_digest(&store.storage, "demoapp", now)
            .unwrap()
            .is_none()
    );
    let composed = store.compose();
    assert!(!composed.text.contains("### demoapp"), "{}", composed.text);
    // The two captures are still reported, from the flat section.
    assert_eq!(composed.hidden.len(), 2);

    crate::facts::store::apply(
        &store.storage,
        &crate::facts::store::FactWrite {
            project: Some("demoapp"),
            subject: "Widget",
            predicate: "price",
            value: Some("six"),
            actor: "test",
            ..Default::default()
        },
    )
    .unwrap();
    let digest = crate::digest::build_project_digest(&store.storage, "demoapp", now)
        .unwrap()
        .expect("the fact carries the page");
    assert!(
        !digest.markdown.contains("Standing decisions:"),
        "{}",
        digest.markdown
    );
    assert!(!digest.markdown.contains(ROW));
    assert_eq!(digest.hidden.len(), 2);
    assert!(
        digest
            .hidden
            .iter()
            .all(|h| h.reason == HiddenReason::Noise(Noise::TableRow))
    );
    // An open follow-up carries the page as well.
    let store = Store::new();
    let entity = store.project("demoapp");
    for text in [ROW, "| Step | Decided |\n| 1 | yes |"] {
        let id = store.capture(Capture::Decision, text);
        store.link(&id, &entity);
    }
    crate::followups::create(
        &store.storage,
        crate::followups::NewFollowup {
            project: "demoapp",
            title: "finish the parser",
            source_memory_id: None,
            authoritative: true,
            actor: "test",
            request_id: "r1",
        },
        now,
    )
    .unwrap();
    let digest = crate::digest::build_project_digest(&store.storage, "demoapp", now)
        .unwrap()
        .expect("the follow-up carries the page");
    assert!(
        digest.markdown.contains("finish the parser"),
        "{}",
        digest.markdown
    );
    assert!(
        !digest.markdown.contains("| Decided |"),
        "{}",
        digest.markdown
    );
    assert_eq!(digest.hidden.len(), 2);
}

#[test]
fn context_quality_a_decision_a_digest_hid_stays_out_of_the_flat_sections() {
    // The noisy replacement is the oldest memory: outside the newest 200
    // of the flat sections, inside the pool of its project.
    let store = Store::new();
    let entity = store.project("demoapp");
    let mut new = captured(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    new.timestamp = Utc::now() - chrono::Duration::days(400);
    store.save(&new);
    store.link(&new.id, &entity);
    for i in 0..200 {
        let mut filler = MemoryEntry::new(
            format!("Note {i}"),
            "a note",
            MemoryType::Note,
            EventSource::Socket,
        );
        filler.timestamp = Utc::now() - chrono::Duration::days(300) + chrono::Duration::minutes(i);
        store.save(&filler);
    }
    let old = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    store.link(&old, &entity);
    let other = store.capture(Capture::Decision, "Decision: keep the smaller index");
    store.link(&other, &entity);
    store.confirm(&old, &new.id);

    let composed = store.compose();
    assert!(composed.text.contains("### demoapp"), "{}", composed.text);
    assert!(
        !composed.text.contains("Postgres for the ledger"),
        "{}",
        composed.text
    );
    assert_eq!(
        hidden_of(&composed, &old),
        Some(&HiddenReason::ReplacedByNoise {
            replacement: new.id.clone()
        })
    );
    assert!(!shown_ids(&composed).contains(&old.as_str()));
}

#[test]
fn context_quality_a_noisy_replacement_past_the_cap_still_hides_its_old_decision() {
    let store = Store::new();
    let entity = store.project("demoapp");
    // Six old decisions, each replaced by a noisy one: more than the five
    // standing decisions a digest shows.
    let mut olds = Vec::new();
    for i in 0..6 {
        let old = store.capture(
            Capture::Decision,
            &format!("Decision: use store {i} for the ledger"),
        );
        let new = store.capture(
            Capture::Decision,
            &format!("| Decision: | use store {i} again |"),
        );
        store.link(&old, &entity);
        store.link(&new, &entity);
        store.confirm(&old, &new);
        olds.push(old);
    }
    let digest = crate::digest::build_project_digest(&store.storage, "demoapp", Utc::now())
        .unwrap()
        .expect("digest");
    assert!(
        !digest.markdown.contains("for the ledger"),
        "{}",
        digest.markdown
    );
    assert!(!digest.markdown.contains("again"), "{}", digest.markdown);
    for old in &olds {
        assert!(
            digest
                .hidden
                .iter()
                .any(|h| h.id == *old && matches!(h.reason, HiddenReason::ReplacedByNoise { .. })),
            "{old}"
        );
    }
    let composed = store.compose();
    assert!(
        !composed.text.contains("for the ledger"),
        "{}",
        composed.text
    );
}

#[test]
fn context_quality_a_digest_hides_a_chain_that_ends_in_noise() {
    let store = Store::new();
    let entity = store.project("demoapp");
    let a = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    let b = store.capture(Capture::Decision, "Decision: use MySQL for the ledger");
    let c = store.capture(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    let other = store.capture(Capture::Decision, "Decision: keep the smaller index");
    for id in [&a, &b, &c, &other] {
        store.link(id, &entity);
    }
    store.confirm(&a, &b);
    store.confirm(&b, &c);
    let digest = crate::digest::build_project_digest(&store.storage, "demoapp", Utc::now())
        .unwrap()
        .expect("digest");
    assert!(
        !digest.markdown.contains("for the ledger"),
        "{}",
        digest.markdown
    );
    assert!(digest.markdown.contains("keep the smaller index"));
    let reason = |id: &str| digest.hidden.iter().find(|h| h.id == id).map(|h| &h.reason);
    for old in [&a, &b] {
        assert_eq!(
            reason(old),
            Some(&HiddenReason::ReplacedByNoise {
                replacement: c.clone()
            }),
            "{old}"
        );
    }
    assert_eq!(reason(&c), Some(&HiddenReason::Noise(Noise::TableRow)));

    // Two replacements, one clean: the clean one stands, the old one is
    // history and is not listed.
    let store = Store::new();
    let entity = store.project("demoapp");
    let a = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    let b = store.capture(Capture::Decision, "Decision: use MySQL for the ledger");
    let c = store.capture(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    for id in [&a, &b, &c] {
        store.link(id, &entity);
    }
    store.confirm(&a, &b);
    store.confirm(&a, &c);
    let digest = crate::digest::build_project_digest(&store.storage, "demoapp", Utc::now())
        .unwrap()
        .expect("digest");
    assert!(
        digest.markdown.contains("MySQL for the ledger"),
        "{}",
        digest.markdown
    );
    assert!(
        !digest.markdown.contains("Postgres for the ledger"),
        "{}",
        digest.markdown
    );
    assert!(
        !digest.markdown.contains("SQLite for the ledger"),
        "{}",
        digest.markdown
    );
    assert!(
        digest.hidden.iter().all(|h| h.id != a),
        "{:?}",
        digest.hidden
    );
    assert!(digest.hidden.iter().any(|h| h.id == c));
}

#[test]
fn context_quality_a_digest_hides_a_replacement_that_is_noise_and_its_old_decision() {
    let store = Store::new();
    let entity = store.project("demoapp");
    let old = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    let new = store.capture(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    let other = store.capture(Capture::Decision, "Decision: keep the smaller index");
    for id in [&old, &new, &other] {
        store.link(id, &entity);
    }
    store.confirm(&old, &new);

    let digest = crate::digest::build_project_digest(&store.storage, "demoapp", Utc::now())
        .unwrap()
        .expect("digest");
    assert!(
        digest.markdown.contains("keep the smaller index"),
        "{}",
        digest.markdown
    );
    assert!(
        !digest.markdown.contains("for the ledger"),
        "{}",
        digest.markdown
    );
    assert!(!digest.source_ids.contains(&old) && !digest.source_ids.contains(&new));
    let reason = |id: &str| digest.hidden.iter().find(|h| h.id == id).map(|h| &h.reason);
    assert_eq!(
        reason(&old),
        Some(&HiddenReason::ReplacedByNoise {
            replacement: new.clone()
        })
    );
    assert_eq!(reason(&new), Some(&HiddenReason::Noise(Noise::TableRow)));

    // The context carries the digest's report, once per memory.
    let composed = store.compose();
    assert!(composed.text.contains("### demoapp"), "{}", composed.text);
    assert!(
        !composed.text.contains("for the ledger"),
        "{}",
        composed.text
    );
    assert_eq!(composed.hidden.iter().filter(|h| h.id == old).count(), 1);
    assert_eq!(
        hidden_of(&composed, &old),
        Some(&HiddenReason::ReplacedByNoise {
            replacement: new.clone()
        })
    );
}

#[test]
fn context_quality_the_topic_context_leaves_noise_out_after_the_search() {
    let store = Store::new();
    let embedder = ConstEmbedder;
    let save = |entry: &MemoryEntry| {
        let vector = crate::embedding::Embedder::embed(&embedder, "").unwrap();
        store
            .storage
            .save_with_embedding(entry, Some(&vector))
            .unwrap();
        entry.id.clone()
    };
    let noise = save(&captured(
        Capture::Decision,
        "| Option | SQLite | Decided |",
    ));
    let clean = save(&captured(Capture::Decision, CLEAN));
    let second = save(&captured(
        Capture::Decision,
        "Decision: use SQLite for the cache too",
    ));

    let whisper = store.whisper();
    let opts = crate::retrieval::HybridOptions {
        limit: usize::MAX,
        touch_access: false,
        ..Default::default()
    };
    let fused = crate::retrieval::hybrid_search(&store.storage, &embedder, "index", &opts).unwrap();
    let clean_order: Vec<&str> = fused
        .iter()
        .map(|hit| hit.entry.id.as_str())
        .filter(|id| *id != noise)
        .collect();
    assert_eq!(clean_order.len(), 2, "the search finds both clean hits");

    let composed = whisper
        .compose_topic(&store.storage, &embedder, "index", 10)
        .unwrap();
    assert!(!store.context_file().exists());
    assert_eq!(
        shown_ids(&composed),
        clean_order,
        "the order the search gave"
    );
    assert_eq!(
        hidden_of(&composed, &noise),
        Some(&HiddenReason::Noise(Noise::TableRow))
    );
    assert!(
        !composed.text.contains("| Option | SQLite | Decided |"),
        "{}",
        composed.text
    );
    // The body and the sources name the same memories.
    let (body, sources) = composed.text.split_once("## Sources").unwrap();
    for id in [&clean, &second] {
        assert!(sources.contains(&id[..8]), "{sources}");
    }
    assert!(!sources.contains(&noise[..8]), "{sources}");
    assert!(
        body.contains(CLEAN) && body.contains("for the cache too"),
        "{body}"
    );
    assert_eq!(body.matches("- [decision]").count(), 2, "{body}");

    // The limit is applied after the filter: one shown, and it is clean.
    let one = whisper
        .compose_topic(&store.storage, &embedder, "index", 1)
        .unwrap();
    assert_eq!(shown_ids(&one), [clean_order[0]]);
    assert!(one.hidden.iter().any(|h| h.id == noise));

    // Composing touches nothing; the context of a topic touches what it shows.
    let before = store.rows();
    let _ = whisper
        .compose_topic(&store.storage, &embedder, "index", 10)
        .unwrap();
    assert_eq!(store.rows(), before);
    let text = whisper
        .topic_context(&store.storage, &embedder, "index", 10)
        .unwrap();
    assert!(text.contains(CLEAN));
    assert!(!store.context_file().exists(), "the topic writes no file");
    let touched: Vec<_> = store
        .storage
        .recent_ranked(10)
        .unwrap()
        .into_iter()
        .filter(|r| r.access_count > 0)
        .map(|r| r.entry.id)
        .collect();
    assert_eq!(touched.len(), 2);
    assert!(!touched.contains(&noise));
}

#[test]
fn context_quality_a_memory_saved_on_purpose_with_the_text_of_noise_is_shown() {
    let store = Store::new();
    for source in [EventSource::Manual, EventSource::Socket] {
        let mut entry = MemoryEntry::new(ROW, ROW, MemoryType::Decision, source);
        entry.tags = vec!["decision".into(), "conversation".into()];
        store.save(&entry);
    }
    let composed = store.compose();
    assert!(composed.text.contains(ROW), "{}", composed.text);
    assert_eq!(
        composed
            .shown
            .iter()
            .filter(|s| s.section == "decision")
            .count(),
        2
    );
    assert!(composed.hidden.is_empty());
}

#[test]
fn context_quality_the_section_is_shorter_when_the_pool_holds_fewer_clean_decisions() {
    let store = Store::new();
    for i in 0..3 {
        store.capture(Capture::Decision, &format!("Decision: keep index {i}"));
    }
    for i in 0..12 {
        store.capture(Capture::Decision, &format!("| Option {i} | Decided |"));
    }
    let composed = store.compose();
    let decisions: Vec<&str> = composed
        .shown
        .iter()
        .filter(|s| s.section == "decision")
        .map(|s| s.id.as_str())
        .collect();
    assert_eq!(decisions.len(), 3);
    assert_eq!(composed.text.matches("- **Decision: keep index").count(), 3);
    assert!(!composed.text.contains("| Decided |"), "{}", composed.text);
    assert_eq!(composed.hidden.len(), 12);
}

#[test]
fn context_quality_the_report_of_what_is_hidden_names_each_with_its_reason() {
    let store = Store::new();
    let row = store.capture(Capture::Decision, ROW);
    let old = store.capture(Capture::Decision, "Decision: use Postgres for the ledger");
    let new = store.capture(
        Capture::Decision,
        "| Decision: | use SQLite for the ledger |",
    );
    store.confirm(&old, &new);
    store.capture(Capture::Decision, CLEAN);
    store.capture(Capture::Correction, "| stop | reason |");

    let before = store.rows();
    let composed = store.compose();
    assert_eq!(
        store.rows(),
        before,
        "no row changed, no access count moved"
    );
    assert!(!store.dir.path().join("out").exists(), "no folder was made");

    let report = hidden_report(&composed, false);
    assert!(
        report.starts_with("Hidden from the context: 4 memories"),
        "{report}"
    );
    assert!(report.contains("  table-row: 3\n"), "{report}");
    assert!(report.contains("  replaced-by-noise: 1\n"), "{report}");
    assert!(
        report.contains(&format!("  {}  decision  table-row  {ROW}", &row[..8])),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "  {}  decision  replaced-by-noise `{}`  Decision: use Postgres for the ledger",
            &old[..8],
            &new[..8]
        )),
        "{report}"
    );
    assert!(
        report.contains("  feedback  table-row  | stop | reason |"),
        "{report}"
    );
    assert!(!report.contains(CLEAN));

    let json: serde_json::Value = serde_json::from_str(&hidden_report(&composed, true)).unwrap();
    assert_eq!(json["counts"]["table-row"], 3);
    assert_eq!(json["counts"]["replaced-by-noise"], 1);
    let items = json["hidden"].as_array().unwrap();
    assert_eq!(items.len(), 4);
    let of_old = items.iter().find(|i| i["id"] == old).unwrap();
    assert_eq!(of_old["reason"], "replaced-by-noise");
    assert_eq!(of_old["replacement"], new);
    assert_eq!(of_old["type"], "decision");
    let of_row = items.iter().find(|i| i["id"] == row).unwrap();
    assert_eq!(of_row["reason"], "table-row");
    assert!(of_row.get("replacement").is_none());
}

#[test]
fn context_quality_the_report_cuts_a_long_title() {
    let store = Store::new();
    let long = format!("| {} | Decided |", "x".repeat(100));
    let mut entry = captured(Capture::Decision, &long);
    entry.title = long.clone();
    let id = store.save(&entry);
    let composed = store.compose();
    let report = hidden_report(&composed, false);
    let line = report.lines().find(|line| line.contains(&id[..8])).unwrap();
    let title = line.rsplit("  ").next().unwrap();
    assert_eq!(
        title.chars().count(),
        83,
        "80 characters and three dots: {title}"
    );
}

#[test]
fn context_quality_hidden_refuses_a_topic_and_json_needs_hidden() {
    use clap::Parser;
    assert!(crate::Cli::try_parse_from(["mnemonic", "context", "--hidden"]).is_ok());
    assert!(crate::Cli::try_parse_from(["mnemonic", "context", "--hidden", "--json"]).is_ok());
    assert!(
        crate::Cli::try_parse_from(["mnemonic", "context", "--hidden", "--topic", "x"]).is_err()
    );
    assert!(crate::Cli::try_parse_from(["mnemonic", "context", "--json"]).is_err());
    // The widget's button generates a context that is not recorded.
    assert!(crate::Cli::try_parse_from(["mnemonic", "context", "--no-record"]).is_ok());
    assert!(
        crate::Cli::try_parse_from(["mnemonic", "context", "--no-record", "--topic", "x"]).is_ok()
    );
}

#[test]
fn context_quality_recent_json_marks_noise() {
    let noise = captured(Capture::Decision, ROW);
    let clean = captured(Capture::Decision, CLEAN);
    assert_eq!(crate::recent_item_json(&noise)["noise"], "table-row");
    assert!(crate::recent_item_json(&clean).get("noise").is_none());
    assert_eq!(crate::recent_item_json(&clean)["title"], CLEAN);
}

#[test]
fn context_quality_generate_writes_what_compose_gives() {
    let store = Store::new();
    store.capture(Capture::Decision, ROW);
    store.capture(Capture::Decision, CLEAN);
    let whisper = store.whisper();
    let text = whisper.generate(&store.storage).unwrap();
    assert!(text.contains(CLEAN) && !text.contains(ROW));
    assert_eq!(std::fs::read_to_string(store.context_file()).unwrap(), text);
}
