//! The registry over a fixture workspace. Every name here is invented.
use super::*;
use crate::graph::{Entity, EntityType};
use crate::storage::projects::{ProjectRow, ProjectState};
use std::sync::Arc;

/// A mark placed in the fixture's locations and in no name of a project.
const SENTINEL: &str = "zq-sentinel";

struct Workspace {
    dir: PathBuf,
    root: PathBuf,
    storage: Arc<Storage>,
}

impl Workspace {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("mn-registry-{}", uuid::Uuid::new_v4().simple()));
        let root = dir.join(format!("{SENTINEL}-workspace/{SENTINEL}-folder"));
        std::fs::create_dir_all(&root).unwrap();
        let storage = Arc::new(Storage::open(&dir.join("memory.db")).unwrap());
        Self { dir, root, storage }
    }

    fn folder(&self, relative: &str) -> PathBuf {
        let path = self.root.join(relative);
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn file(&self, relative: &str) {
        let path = self.root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    fn config(&self) -> ProjectsConfig {
        ProjectsConfig {
            workspace: Some(self.dir.join(format!("{SENTINEL}-workspace"))),
            folder: Some(PathBuf::from(format!("{SENTINEL}-folder"))),
            ..ProjectsConfig::default()
        }
    }

    fn sync(&self) -> (ProjectMap, SyncReport) {
        self.sync_with(&self.config())
    }

    fn sync_with(&self, config: &ProjectsConfig) -> (ProjectMap, SyncReport) {
        sync(&self.storage, config).unwrap().expect("a workspace")
    }

    fn slugs(&self) -> Vec<String> {
        let mut slugs: Vec<String> = self
            .storage
            .projects()
            .unwrap()
            .into_iter()
            .map(|row| row.slug)
            .collect();
        slugs.sort();
        slugs
    }

    fn row(&self, key: &str) -> ProjectRow {
        self.storage.project_by_key(key).unwrap().expect(key)
    }

    /// Every value of a table, row by row, as text.
    fn dump(&self, table: &str) -> Vec<String> {
        let conn = self.storage.conn.lock().unwrap();
        crate::test_support::rows(&conn, &format!("SELECT * FROM {table} ORDER BY 1, 2"))
    }
}

impl Drop for Workspace {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn registry_a_workspace_yields_one_row_for_every_child_folder() {
    let w = Workspace::new();
    w.folder("demoapp/.git");
    w.folder("notes"); // no marker of a project at all
    w.folder(".hidden");
    w.file("README.md");
    let (_, report) = w.sync();
    assert_eq!(w.slugs(), ["demoapp", "notes"]);
    assert_eq!(
        report,
        SyncReport {
            projects: 2,
            refused: 0,
            unset: 0
        }
    );

    // A project only just begun, made after the first listing.
    w.folder("Side Project");
    w.sync();
    assert_eq!(w.slugs(), ["demoapp", "notes", "side-project"]);
    let fresh = w.row("side-project");
    assert_eq!(
        fresh.name, "Side Project",
        "a project carries its folder's name"
    );
    assert_eq!(fresh.state, ProjectState::Active);
    assert!(fresh.slot > w.row("demoapp").slot.max(w.row("notes").slot));

    // Listing again changes nothing.
    let before = w.dump("projects");
    w.sync();
    assert_eq!(w.dump("projects"), before);
}

#[test]
fn registry_a_project_inside_a_project_names_its_parent() {
    let w = Workspace::new();
    w.file("demoapp/tools/Cargo.toml");
    w.folder("demoapp/site/.git");
    w.folder("demoapp/plans/openspec");
    w.folder("demoapp/docs"); // a plain folder
    w.file("demoapp/.cache/package.json"); // hidden
    w.sync();
    assert_eq!(
        w.slugs(),
        ["demoapp", "demoapp:plans", "demoapp:site", "demoapp:tools"]
    );
    let parent = w.row("demoapp");
    assert_eq!(parent.parent_id, None);
    for inner in ["demoapp:plans", "demoapp:site", "demoapp:tools"] {
        assert_eq!(w.row(inner).parent_id, Some(parent.id), "{inner}");
    }
    assert_eq!(w.row("demoapp:tools").name, "tools");
}

#[test]
fn registry_an_alias_resolves_and_a_former_name_maps_its_folder() {
    let w = Workspace::new();
    w.file("demoapp/tools/Cargo.toml");
    w.folder("notes");
    let mut config = w.config();
    config.aliases.insert("Demo".into(), "demoapp".into());
    config
        .aliases
        .insert("ghost".into(), "nothing-there".into());
    // A name another project carries is not given away.
    config.aliases.insert("notes".into(), "demoapp".into());
    config.former.insert("oldapp".into(), "demoapp".into());
    let (map, report) = w.sync_with(&config);
    assert_eq!(report.unset, 2);

    let demoapp = w.row("demoapp");
    assert_eq!(w.row("demo").id, demoapp.id);
    assert_eq!(w.row("oldapp").id, demoapp.id);
    assert_ne!(w.row("notes").id, demoapp.id);
    assert!(w.storage.project_by_key("ghost").unwrap().is_none());

    let at = |relative: &str| map.project_of(&w.root.join(relative).to_string_lossy());
    assert_eq!(at("demoapp/src/main.rs"), Some(demoapp.id));
    assert_eq!(at("oldapp/src/main.rs"), Some(demoapp.id));
    // The deepest folder that holds the path.
    assert_eq!(
        at("demoapp/tools/src/lib.rs"),
        Some(w.row("demoapp:tools").id)
    );
    let elsewhere = w.dir.join("elsewhere/file.rs");
    assert_eq!(map.project_of(&elsewhere.to_string_lossy()), None);
    // A folder whose name only begins like a project's is not that project.
    assert_eq!(at("demoapp-old/file.rs"), None);
}

#[test]
fn registry_no_table_holds_a_folder_or_a_path() {
    let w = Workspace::new();
    w.file("demoapp/tools/Cargo.toml");
    w.folder("notes");
    let mut config = w.config();
    config.aliases.insert("demo".into(), "demoapp".into());
    config.former.insert("oldapp".into(), "demoapp".into());
    w.sync_with(&config);
    let root = w.root.to_string_lossy().to_string();
    let rows: Vec<String> = ["projects", "project_aliases"]
        .iter()
        .flat_map(|table| w.dump(table))
        .collect();
    assert_eq!(rows.len(), 3 + 2);
    for row in rows {
        assert!(!row.contains(SENTINEL), "a location was stored: {row}");
        assert!(!row.contains(&root));
        assert!(!row.contains(std::path::MAIN_SEPARATOR));
    }
}

/// A name the identity policy refuses. Assembled at run time and never
/// printed: the assertions name counts, not the fixture.
fn credential(seed: &str) -> String {
    let body: String = seed.chars().cycle().take(40).collect();
    ["sk-", "proj-", &body].concat()
}

#[test]
fn registry_a_name_the_policy_refuses_is_not_registered_and_is_counted() {
    let w = Workspace::new();
    w.folder("demoapp");
    // Refused as it arrived.
    w.folder(&credential("a1B2c3D4e5F6"));
    // Clean as it is written, refused in the lower case it would be keyed in.
    let upper = credential("g7H8i9J0k1L2").to_uppercase();
    assert!(crate::redaction::is_clean(&upper));
    w.folder(&upper);
    // A project inside a project is judged the same way.
    w.file(&format!(
        "demoapp/{}/Cargo.toml",
        credential("m3N4o5P6q7R8")
    ));

    let (map, report) = w.sync();
    assert_eq!(report.projects, 1);
    assert_eq!(report.refused, 3);
    assert_eq!(w.slugs(), ["demoapp"]);
    assert_eq!(map.folders(), 1);
    assert_eq!(w.dump("projects").len(), 1);

    // The store judges a name too, whoever calls it.
    let refused = w
        .storage
        .project_upsert(&slug_of(&credential("s9T0u1V2w3X4")), "named", None);
    assert!(refused.is_err_and(|e| crate::redaction::state::is_refused(&e)));
    assert_eq!(w.dump("projects").len(), 1);
}

#[test]
fn registry_a_merge_redirects_and_moves_no_row() {
    let w = Workspace::new();
    for name in ["demoapp", "notes", "fresh"] {
        w.folder(name);
        w.storage
            .upsert_entity(&Entity {
                name: name.into(),
                entity_type: EntityType::Concept,
            })
            .unwrap();
    }
    w.storage
        .conn
        .lock()
        .unwrap()
        .execute_batch(
            "INSERT INTO edges (id, source_entity, target_entity, relation)
             VALUES ('e1', 'demoapp', 'notes', 'uses');
             INSERT INTO memory_entities (memory_id, entity_id) VALUES ('m1', 'demoapp');",
        )
        .unwrap();
    w.sync();
    let (a, b, c) = (w.row("demoapp").id, w.row("notes").id, w.row("fresh").id);

    // What a merge must leave as it is, row by row.
    let history = |w: &Workspace| -> Vec<Vec<String>> {
        [
            "entities",
            "edges",
            "memory_entities",
            "entity_aliases",
            "project_aliases",
        ]
        .iter()
        .map(|table| w.dump(table))
        .collect()
    };
    let rows = |w: &Workspace| -> Vec<ProjectRow> {
        w.storage
            .projects()
            .unwrap()
            .into_iter()
            .map(|row| ProjectRow {
                merged_into: None,
                ..row
            })
            .collect()
    };
    let (history_before, rows_before) = (history(&w), rows(&w));
    let groups_before = w.storage.project_groups().unwrap();
    assert_eq!(groups_before.len(), 3);

    w.storage.project_merge(a, b).unwrap();
    assert_eq!(w.storage.project_canonical(a).unwrap(), b);
    assert_eq!(w.storage.project_canonical(b).unwrap(), b);
    let groups = w.storage.project_groups().unwrap();
    assert_eq!(groups.len(), 2);
    assert_eq!(
        groups[&b],
        vec![a, b],
        "a reader sums both under the canonical row"
    );
    assert_eq!(
        w.row("demoapp").merged_into,
        Some(b),
        "the key still names its own row"
    );
    assert_eq!(history(&w), history_before, "a row of history changed");
    assert_eq!(
        rows(&w),
        rows_before,
        "a row of the registry changed beyond the redirect"
    );

    // A merge of the canonical row carries what was merged into it.
    w.storage.project_merge(b, c).unwrap();
    assert_eq!(w.storage.project_canonical(a).unwrap(), c);
    assert_eq!(w.storage.project_groups().unwrap()[&c].len(), 3);

    // Nothing may lead back to where it began.
    assert!(w.storage.project_merge(c, a).is_err());
    assert!(w.storage.project_merge(c, c).is_err());
    assert!(w.storage.project_merge(a, 9_999).is_err());
    assert_eq!(w.storage.project_canonical(a).unwrap(), c);

    // Undone, every answer is as before.
    assert!(w.storage.project_unmerge(b).unwrap());
    assert!(w.storage.project_unmerge(a).unwrap());
    assert!(!w.storage.project_unmerge(a).unwrap());
    assert_eq!(w.storage.project_groups().unwrap(), groups_before);
    assert_eq!(history(&w), history_before);
    assert_eq!(w.storage.projects().unwrap(), rows_before);
}

#[test]
fn registry_an_upsert_sets_the_type_of_the_entity_and_keeps_the_earlier_one() {
    let w = Workspace::new();
    for (name, entity_type) in [
        ("demoapp", EntityType::Concept),
        ("notes", EntityType::Project),
    ] {
        w.folder(name);
        w.storage
            .upsert_entity(&Entity {
                name: name.into(),
                entity_type,
            })
            .unwrap();
    }
    w.folder("fresh");
    let entity_type = |name: &str| -> String {
        w.storage
            .conn
            .lock()
            .unwrap()
            .query_row(
                "SELECT entity_type FROM entities WHERE name = ?1",
                [name],
                |r| r.get(0),
            )
            .unwrap()
    };
    w.sync();
    assert_eq!(entity_type("demoapp"), "project");
    assert_eq!(entity_type("notes"), "project");
    assert_eq!(
        w.row("demoapp").prev_entity_type.as_deref(),
        Some("concept")
    );
    assert_eq!(w.row("notes").prev_entity_type, None);
    assert!(w.row("demoapp").entity_id.is_some());
    assert_eq!(
        w.row("fresh").entity_id,
        None,
        "no entity carries that name yet"
    );

    // The earlier type is kept through later passes.
    w.sync();
    assert_eq!(
        w.row("demoapp").prev_entity_type.as_deref(),
        Some("concept")
    );

    // An entity that appears later is tied at the next pass.
    w.storage
        .upsert_entity(&Entity {
            name: "fresh".into(),
            entity_type: EntityType::Tech,
        })
        .unwrap();
    w.sync();
    assert!(w.row("fresh").entity_id.is_some());
    assert_eq!(w.row("fresh").prev_entity_type.as_deref(), Some("tech"));
    assert_eq!(entity_type("fresh"), "project");
}

#[test]
fn registry_a_slot_is_never_given_twice() {
    let w = Workspace::new();
    for name in ["demoapp", "notes", "fresh"] {
        w.folder(name);
    }
    w.sync();
    let slots = |w: &Workspace| -> Vec<i64> {
        w.storage
            .projects()
            .unwrap()
            .iter()
            .map(|row| row.slot)
            .collect()
    };
    assert_eq!(slots(&w), [0, 1, 2]);
    let kept = w.row("notes").slot;

    // An archived project and a merged one keep their slots; a folder that
    // is gone keeps its row.
    w.storage
        .project_set_state(w.row("demoapp").id, ProjectState::Archived)
        .unwrap();
    w.storage
        .project_merge(w.row("fresh").id, w.row("notes").id)
        .unwrap();
    std::fs::remove_dir_all(w.root.join("fresh")).unwrap();
    w.folder("later");
    w.folder("another");
    w.sync();
    assert_eq!(slots(&w), [0, 1, 2, 3, 4]);
    assert_eq!(w.row("notes").slot, kept);
    assert_eq!(
        w.row("demoapp").state,
        ProjectState::Archived,
        "a pass keeps the owner's state"
    );
}

#[test]
fn registry_without_the_section_nothing_is_discovered() {
    let w = Workspace::new();
    w.folder("demoapp");
    assert_eq!(crate::config::Config::default().projects.root(), None);
    assert!(
        sync(&w.storage, &ProjectsConfig::default())
            .unwrap()
            .is_none()
    );
    assert!(w.storage.projects().unwrap().is_empty());
    assert!(spawn_worker_is_off(&w));

    // The section as the configuration file gives it.
    let section: ProjectsConfig = toml::from_str(
        "workspace = \"/w\"\nfolder = \"projects\"\n[aliases]\ndemo = \"demoapp\"\n[former]\noldapp = \"demoapp\"\n",
    )
    .unwrap();
    assert_eq!(section.root(), Some(PathBuf::from("/w/projects")));
    assert_eq!(section.aliases["demo"], "demoapp");
    assert_eq!(section.former["oldapp"], "demoapp");
    // A workspace without a folder: its own children are the projects.
    let bare: ProjectsConfig = toml::from_str("workspace = \"/w\"\n").unwrap();
    assert_eq!(bare.root(), Some(PathBuf::from("/w")));
    // An empty section writes nothing into a generated configuration.
    assert_eq!(toml::to_string(&ProjectsConfig::default()).unwrap(), "");
}

/// Without a workspace no worker is started.
fn spawn_worker_is_off(w: &Workspace) -> bool {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            spawn_worker(
                w.storage.clone(),
                ProjectsConfig::default(),
                600,
                SharedMap::default(),
                None,
            )
            .is_none()
        })
}

#[test]
fn registry_the_workspace_is_listed_again_only_when_a_change_time_moved() {
    let w = Workspace::new();
    w.folder("demoapp");
    w.folder("notes");
    let still = stamp(&w.root);
    assert_eq!(stamp(&w.root), still);
    assert_eq!(still.len(), 3, "the folder of projects and each project");

    // The clocks of a file system are coarse: step past them.
    std::thread::sleep(std::time::Duration::from_millis(20));
    w.folder("fresh");
    let grown = stamp(&w.root);
    assert_ne!(grown, still, "a new project moved no change time");

    std::thread::sleep(std::time::Duration::from_millis(20));
    w.folder("demoapp/tools");
    assert_ne!(
        stamp(&w.root),
        grown,
        "a new folder inside a project moved no change time"
    );
}

#[test]
fn registry_a_key_is_the_name_in_lower_case_with_dashes() {
    for (name, slug) in [
        ("demoapp", "demoapp"),
        ("Side Project", "side-project"),
        ("  my_app.v2  ", "my-app-v2"),
        ("--x--", "x"),
        ("...", ""),
    ] {
        assert_eq!(slug_of(name), slug, "{name}");
    }
}

#[test]
fn registry_an_alias_is_judged_as_it_was_written_before_it_is_keyed() {
    let w = Workspace::new();
    w.folder("demoapp");
    // Making a key drops the syntax of an assignment and keeps its value.
    let secret: String = "0123456789abcdef".repeat(2);
    let assignment = format!("api_key = {secret}");
    assert!(!crate::redaction::is_clean(&assignment));
    let mut config = w.config();
    config.aliases.insert(assignment.clone(), "demoapp".into());
    config.former.insert(assignment, "demoapp".into());
    let (_, report) = w.sync_with(&config);
    assert_eq!(report.unset, 2);
    assert!(w.dump("project_aliases").is_empty());
    assert!(w.dump("projects").iter().all(|row| !row.contains(&secret)));
}

#[test]
fn registry_a_renamed_folder_keeps_its_history_under_the_new_name() {
    let w = Workspace::new();
    w.folder("oldapp");
    w.folder("notes");
    w.sync();
    let old = w.row("oldapp").id;
    // The folder is renamed; its row stays, as every row does.
    std::fs::rename(w.root.join("oldapp"), w.root.join("newapp")).unwrap();
    let mut config = w.config();
    config.former.insert("oldapp".into(), "newapp".into());
    let (map, report) = w.sync_with(&config);
    assert_eq!(report.unset, 0);
    let new = w.row("newapp").id;
    assert_ne!(new, old);
    assert_eq!(
        w.storage.project_canonical(old).unwrap(),
        new,
        "the old row redirects"
    );
    let at = |relative: &str| map.project_of(&w.root.join(relative).to_string_lossy());
    assert_eq!(at("oldapp/src/main.rs"), Some(new));
    assert_eq!(at("newapp/src/main.rs"), Some(new));
    // Again: nothing more to do, and nothing undone.
    let (_, report) = w.sync_with(&config);
    assert_eq!(report.unset, 0);
    assert_eq!(w.storage.project_canonical(old).unwrap(), new);

    // Two folders that are both there are two projects: the configuration
    // cannot make one the former name of the other.
    let mut config = w.config();
    config.former.insert("notes".into(), "newapp".into());
    let (map, report) = w.sync_with(&config);
    assert_eq!(report.unset, 1);
    assert_eq!(
        map.project_of(&w.root.join("notes/a").to_string_lossy()),
        Some(w.row("notes").id)
    );
}

#[test]
fn registry_a_marker_added_inside_a_project_moves_the_stamp() {
    let w = Workspace::new();
    w.folder("demoapp/tools");
    let still = stamp(&w.root);
    w.sync();
    assert_eq!(w.slugs(), ["demoapp"]);
    std::thread::sleep(std::time::Duration::from_millis(20));
    w.file("demoapp/tools/Cargo.toml");
    assert_ne!(stamp(&w.root), still, "a new marker moved no change time");
    w.sync();
    assert_eq!(w.slugs(), ["demoapp", "demoapp:tools"]);
}

#[test]
fn registry_an_entity_that_appears_later_is_tied_without_a_change_of_folders() {
    let w = Workspace::new();
    w.folder("demoapp");
    w.sync();
    assert_eq!(w.row("demoapp").entity_id, None);
    assert_eq!(
        w.storage.projects_link_missing(canonicalize_name).unwrap(),
        0
    );
    w.storage
        .upsert_entity(&Entity {
            name: "demoapp".into(),
            entity_type: EntityType::Concept,
        })
        .unwrap();
    // What the worker does on a pass where no folder moved.
    assert_eq!(
        w.storage.projects_link_missing(canonicalize_name).unwrap(),
        1
    );
    assert!(w.row("demoapp").entity_id.is_some());
    assert_eq!(
        w.row("demoapp").prev_entity_type.as_deref(),
        Some("concept")
    );
    assert_eq!(
        w.storage.projects_link_missing(canonicalize_name).unwrap(),
        0
    );
}

#[test]
fn registry_an_alias_can_name_a_project_inside_a_project() {
    let w = Workspace::new();
    w.file("demoapp/tools/Cargo.toml");
    let mut config = w.config();
    config
        .aliases
        .insert("builder".into(), "demoapp:tools".into());
    config
        .former
        .insert("oldtools".into(), "Demoapp:Tools".into());
    let (map, report) = w.sync_with(&config);
    assert_eq!(report.unset, 0);
    let tools = w.row("demoapp:tools").id;
    assert_eq!(w.row("builder").id, tools);
    assert_eq!(
        map.project_of(&w.root.join("oldtools/a.rs").to_string_lossy()),
        Some(tools)
    );
}

#[test]
fn registry_a_long_chain_of_merges_resolves_to_its_end() {
    let w = Workspace::new();
    let ids: Vec<i64> = (0..70)
        .map(|n| {
            let name = format!("project-{n:02}");
            w.storage.project_upsert(&name, &name, None).unwrap()
        })
        .collect();
    for pair in ids.windows(2) {
        w.storage.project_merge(pair[0], pair[1]).unwrap();
    }
    let last = *ids.last().unwrap();
    assert_eq!(w.storage.project_canonical(ids[0]).unwrap(), last);
    let groups = w.storage.project_groups().unwrap();
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[&last].len(), 70);
    // The end of the chain cannot be merged back into its start.
    assert!(w.storage.project_merge(last, ids[0]).is_err());
    assert_eq!(w.storage.project_canonical(ids[0]).unwrap(), last);
}

#[test]
fn registry_the_stamp_of_redirects_tells_merges_that_swapped_their_targets() {
    let dir = crate::test_support::temp_dir("mn-registry-stamp-");
    let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
    let ids: Vec<i64> = ["one", "two", "three", "four"]
        .iter()
        .map(|name| storage.project_upsert(name, name, None).unwrap())
        .collect();
    assert_eq!(storage.project_redirects_stamp().unwrap(), "");
    storage.project_merge(ids[0], ids[2]).unwrap();
    storage.project_merge(ids[1], ids[3]).unwrap();
    let straight = storage.project_redirects_stamp().unwrap();
    // The same two rows redirect, each to the other's target.
    storage.project_unmerge(ids[0]).unwrap();
    storage.project_unmerge(ids[1]).unwrap();
    storage.project_merge(ids[0], ids[3]).unwrap();
    storage.project_merge(ids[1], ids[2]).unwrap();
    let crossed = storage.project_redirects_stamp().unwrap();
    assert_ne!(straight, crossed);
    assert_ne!(crossed, "");
}
