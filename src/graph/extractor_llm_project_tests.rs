//! The optional registry clamp, through extraction, cache and legacy readers.
use super::*;
use crate::event::{EventSource, MemoryType};
use crate::storage::projects::AliasKind;
use crate::test_support::InTempDir;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

struct Backend {
    response: String,
    calls: Arc<AtomicUsize>,
}

impl LlmBackend for Backend {
    fn generate(&self, _: &str) -> anyhow::Result<String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(self.response.clone())
    }
}

fn storage() -> InTempDir<Arc<Storage>> {
    InTempDir::new("mn-curated-clamp-", |dir| {
        Arc::new(Storage::open(&dir.join("memory.db")).unwrap())
    })
}

fn memory() -> MemoryEntry {
    MemoryEntry::new(
        "Harbor board work",
        "The harbor board was tested.",
        MemoryType::Note,
        EventSource::Manual,
    )
}

fn extractor(
    storage: Arc<Storage>,
    enabled: bool,
    names: &[(&str, &str)],
) -> (LlmExtractor, Arc<AtomicUsize>) {
    let calls = Arc::new(AtomicUsize::new(0));
    let backend = Backend {
        calls: calls.clone(),
        response: serde_json::json!({
            "entities": names.iter().map(|(name, kind)| serde_json::json!({
                "name": name, "entity_type": kind,
            })).collect::<Vec<_>>(),
            "relations": [],
        })
        .to_string(),
    };
    let config = LlmConfig {
        enabled: true,
        endpoint: String::new(),
        model: "fixture".into(),
        timeout_secs: 1,
        min_chars: 0,
    };
    let extractor = LlmExtractor::new(Box::new(backend), storage, &config);
    (
        if enabled {
            extractor.with_project_registry(true)
        } else {
            extractor
        },
        calls,
    )
}

fn kind(result: &ExtractionResult, name: &str) -> EntityType {
    result
        .entities
        .iter()
        .find(|entity| entity.name == name)
        .expect("entity survived normal quality rules")
        .entity_type
        .clone()
}

#[test]
fn curated_graph_clamp_fresh_registry_names_and_aliases_only() {
    let storage = storage();
    let id = storage
        .project_upsert("harbor-board", "Harbor Display", None)
        .unwrap();
    storage
        .project_alias_set("Harbor Console", id, AliasKind::Name)
        .unwrap();
    storage
        .project_alias_set("archive-console", id, AliasKind::Scope)
        .unwrap();
    let (extractor, calls) = extractor(
        storage.clone(),
        true,
        &[
            ("Harbor Board", "project"),
            ("Harbor Display", "project"),
            ("Harbor Console", "project"),
            ("archive-console", "project"),
            ("phantom-board", "project"),
            ("rust", "tech"),
        ],
    );
    let result = extractor.extract(&memory());
    for name in ["harbor-board", "harbor-display", "harbor-console"] {
        assert_eq!(kind(&result, name), EntityType::Project);
    }
    for name in ["archive-console", "phantom-board"] {
        assert_eq!(kind(&result, name), EntityType::Concept);
    }
    assert_eq!(kind(&result, "rust"), EntityType::Tech);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn curated_graph_clamp_cache_reads_current_registry_and_aliases() {
    let storage = storage();
    let id = storage
        .project_upsert("harbor-board", "Harbor Board", None)
        .unwrap();
    let (extractor, calls) = extractor(
        storage.clone(),
        true,
        &[("harbor-console", "project"), ("new-board", "project")],
    );
    let entry = memory();
    let first = extractor.extract(&entry);
    assert_eq!(kind(&first, "harbor-console"), EntityType::Concept);
    assert_eq!(kind(&first, "new-board"), EntityType::Concept);
    storage
        .project_alias_set("harbor-console", id, AliasKind::Name)
        .unwrap();
    storage
        .project_upsert("new-board", "New Board", None)
        .unwrap();
    let second = extractor.extract(&entry);
    assert_eq!(kind(&second, "harbor-console"), EntityType::Project);
    assert_eq!(kind(&second, "new-board"), EntityType::Project);
    storage
        .project_alias_set("harbor-console", id, AliasKind::Scope)
        .unwrap();
    let third = extractor.extract(&entry);
    assert_eq!(kind(&third, "harbor-console"), EntityType::Concept);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}

#[test]
fn curated_graph_clamp_disabled_preserves_cache_and_existing_project_readers() {
    let storage = storage();
    let (unconfigured, calls) = extractor(storage.clone(), false, &[("phantom-board", "project")]);
    let first = memory();
    let result = unconfigured.extract(&first);
    assert_eq!(kind(&result, "phantom-board"), EntityType::Project);
    let mut second = memory();
    second.id = uuid::Uuid::new_v4().to_string();
    let cached = unconfigured.extract(&second);
    assert_eq!(kind(&cached, "phantom-board"), EntityType::Project);
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    for entry in [&first, &second] {
        storage.save(entry).unwrap();
        storage
            .save_graph(&entry.id, &result.entities, &result.edges)
            .unwrap();
    }
    assert!(storage.projects().unwrap().is_empty());
    let rows = storage.projects_overview(10, 5).unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].name, "phantom-board");
    assert_eq!(rows[0].mem_count, 2);
    let from = first.timestamp - chrono::Duration::seconds(1);
    let to = second.timestamp + chrono::Duration::seconds(1);
    let signals = storage.project_signals_in_window(from, to, 2).unwrap();
    assert_eq!(signals.len(), 1);
    assert_eq!(signals[0].project_key, rows[0].key);
    assert_eq!(
        storage
            .project_mem_times_in_window(from, to, 2)
            .unwrap()
            .len(),
        2
    );
    let (configured, configured_calls) = extractor(storage.clone(), true, &[]);
    assert_eq!(
        kind(&configured.extract(&first), "phantom-board"),
        EntityType::Concept
    );
    assert_eq!(configured_calls.load(Ordering::SeqCst), 0);
}

#[test]
fn curated_graph_clamp_registry_read_failure_downgrades_project_only() {
    let storage = storage();
    storage
        .conn
        .lock()
        .unwrap()
        .execute("ALTER TABLE project_aliases RENAME TO hidden_aliases", [])
        .unwrap();
    let (extractor, calls) = extractor(
        storage.clone(),
        true,
        &[("phantom-board", "project"), ("rust", "tech")],
    );
    let entry = memory();
    for _ in 0..2 {
        let result = extractor.extract(&entry);
        assert_eq!(kind(&result, "phantom-board"), EntityType::Concept);
        assert_eq!(kind(&result, "rust"), EntityType::Tech);
    }
    assert_eq!(calls.load(Ordering::SeqCst), 1);
}
