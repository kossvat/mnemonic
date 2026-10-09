use super::*;
use crate::storage::projects::ProjectState;

#[derive(Clone, Copy, Debug)]
enum Action {
    Archive,
    Unarchive,
    Merge,
    Unmerge,
}

impl Action {
    fn request(self) -> Request<Body> {
        let (action, body) = match self {
            Self::Archive => ("archive", None),
            Self::Unarchive => ("unarchive", None),
            Self::Merge => ("merge", Some(r#"{"into":"beta"}"#)),
            Self::Unmerge => ("unmerge", None),
        };
        post(&format!("/api/projects/alpha/{action}"), body)
    }

    fn alpha_visible(self) -> bool {
        matches!(self, Self::Unarchive | Self::Unmerge)
    }

    fn assert_stored(self, f: &Fixture) {
        let row = f.state.storage.project_by_key("alpha").unwrap().unwrap();
        assert_eq!(
            row.state,
            if matches!(self, Self::Archive) {
                ProjectState::Archived
            } else {
                ProjectState::Active
            }
        );
        assert_eq!(row.merged_into, matches!(self, Self::Merge).then_some(2));
    }
}

const ACTIONS: [Action; 4] = [
    Action::Archive,
    Action::Unarchive,
    Action::Merge,
    Action::Unmerge,
];

fn post(uri: &str, body: Option<&str>) -> Request<Body> {
    let mut request = request(uri, true, "localhost", body);
    *request.method_mut() = Method::POST;
    request
}

async fn snapshot(f: &Fixture) -> (Vec<u8>, Vec<u8>) {
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    let (status, headers, body) = call(
        &f.state,
        request("/api/graph/curated", true, "localhost", None),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
    (headers[header::ETAG].as_bytes().to_vec(), body)
}

fn assert_projects(body: &[u8], alpha_visible: bool) {
    let graph = json(body);
    let nodes = graph["n"].as_array().unwrap();
    assert_eq!(nodes.iter().any(|row| row[0] == "project:1"), alpha_visible);
    assert!(nodes.iter().any(|row| row[0] == "project:2"));
}

#[tokio::test]
async fn registry_mutations_refresh_the_cached_graph_before_replying() {
    let f = Fixture::new(3);
    let mut previous = snapshot(&f).await;
    assert_projects(&previous.1, true);
    for action in ACTIONS {
        let (status, _, response) = call(&f.state, action.request()).await;
        assert_eq!(status, StatusCode::OK, "{action:?}");
        assert_eq!(json(&response)["key"], "alpha");
        action.assert_stored(&f);
        let current = snapshot(&f).await;
        assert_projects(&current.1, action.alpha_visible());
        assert_ne!(current.0, previous.0, "{action:?}");
        assert_ne!(current.1, previous.1, "{action:?}");
        previous = current;
    }
}

#[tokio::test]
async fn registry_mutation_retries_repair_each_failed_refresh_without_another_write() {
    let f = Fixture::new(3);
    for action in ACTIONS {
        let old = snapshot(&f).await;
        // The registry write commits before graph refresh reaches slots.
        f.state
            .storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "ALTER TABLE graph_slots RENAME TO fixture_saved_graph_slots",
                [],
            )
            .unwrap();
        let (failed, _, _) = call(&f.state, action.request()).await;
        f.state
            .storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "ALTER TABLE fixture_saved_graph_slots RENAME TO graph_slots",
                [],
            )
            .unwrap();
        assert_eq!(failed, StatusCode::INTERNAL_SERVER_ERROR, "{action:?}");
        action.assert_stored(&f);
        assert_eq!(snapshot(&f).await, old);

        let (before, changes, files) = (f.dump(), f.changes(), f.files());
        let (status, _, response) = call(&f.state, action.request()).await;
        assert_eq!(status, StatusCode::OK, "{action:?}");
        assert_eq!(json(&response)["key"], "alpha");
        action.assert_stored(&f);
        assert_eq!(f.dump(), before);
        assert_eq!(f.changes(), changes);
        assert_eq!(f.files(), files);
        let repaired = snapshot(&f).await;
        assert_projects(&repaired.1, action.alpha_visible());
        assert_ne!(repaired.0, old.0, "{action:?}");
        assert_ne!(repaired.1, old.1, "{action:?}");
    }
}

#[tokio::test]
async fn merge_retry_accepts_only_an_already_merged_source_in_the_same_group() {
    let f = Fixture::new(3);
    let gamma = f
        .state
        .storage
        .project_upsert("gamma", "gamma", None)
        .unwrap();
    f.state.storage.project_merge(gamma, 2).unwrap();
    assert_eq!(
        call(&f.state, Action::Merge.request()).await.0,
        StatusCode::OK
    );
    let cached = snapshot(&f).await;
    let (before, changes, files) = (f.dump(), f.changes(), f.files());
    // Either name now resolves to the same canonical target. Neither retry
    // rewrites the redirect, even when it names another merged member.
    for into in ["beta", "gamma"] {
        let body = serde_json::json!({"into": into}).to_string();
        let (status, _, response) =
            call(&f.state, post("/api/projects/alpha/merge", Some(&body))).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(json(&response)["merged_into"], "beta");
    }
    // Direct self-merges and a canonical root back into its descendants
    // are still invalid, even when every name belongs to the same group.
    for (from, into) in [("alpha", "alpha"), ("beta", "alpha"), ("beta", "gamma")] {
        let body = serde_json::json!({"into": into}).to_string();
        let (status, _, _) = call(
            &f.state,
            post(&format!("/api/projects/{from}/merge"), Some(&body)),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
    }
    assert_eq!(f.dump(), before);
    assert_eq!(f.changes(), changes);
    assert_eq!(f.files(), files);
    assert_eq!(snapshot(&f).await, cached);
}
