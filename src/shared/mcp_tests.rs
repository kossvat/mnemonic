use super::*;
use std::io::Cursor;
use std::path::PathBuf;

struct Fixture {
    store: SharedStore,
    dir: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let dir =
            std::env::temp_dir().join(format!("mnemonic-shared-mcp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&dir).unwrap();
        let store = SharedStore::open(&dir.join("shared.db")).unwrap();
        store
            .publish(
                "alpha",
                "offer",
                "Offer",
                "Public product details",
                "catalog:v1",
            )
            .unwrap();
        store
            .publish(
                "beta",
                "offer",
                "Other",
                "CONFIDENTIAL_OTHER_PROJECT",
                "internal:v1",
            )
            .unwrap();
        Self { store, dir }
    }
    fn server(&self, writes: bool) -> SharedMcp<'_> {
        let mut server = SharedMcp::new(
            &self.store,
            Policy {
                version: 1,
                project_id: "alpha".into(),
                agent_id: "researcher".into(),
                allow_observations: writes,
                max_observations_per_session: 2,
                principal_id: None,
                expires_at: None,
                max_session_secs: None,
            },
        );
        let init = server.respond(br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#).unwrap();
        assert!(init.get("error").is_none());
        assert!(
            server
                .respond(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
                .is_none()
        );
        server
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn call(server: &mut SharedMcp<'_>, name: &str, args: Value) -> Value {
    let req = json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":name,"arguments":args}});
    server.respond(&serde_json::to_vec(&req).unwrap()).unwrap()
}
fn data(response: &Value) -> Value {
    serde_json::from_str(response["result"]["content"][0]["text"].as_str().unwrap()).unwrap()
}
fn observation(request: &str) -> Value {
    json!({"request_id":request,"title":"Research result","content":"Needs checking","source":"https://example.test/source"})
}

#[test]
fn project_identity_is_fixed_for_all_read_paths_and_unknown_fields_fail() {
    let f = Fixture::new();
    let mut server = f.server(false);
    for (name, args) in [
        ("shared_context", json!({})),
        ("shared_search", json!({"query":"CONFIDENTIAL"})),
        ("shared_get", json!({"key":"offer"})),
    ] {
        let response = call(&mut server, name, args.clone());
        assert_eq!(response["result"]["isError"], false);
        assert!(!response.to_string().contains("CONFIDENTIAL_OTHER_PROJECT"));
        let mut forged = args;
        forged["project_id"] = json!("beta");
        assert_eq!(call(&mut server, name, forged)["result"]["isError"], true);
    }
    let context = data(&call(&mut server, "shared_context", json!({})));
    assert_eq!(context["context"]["project_id"], "alpha");
    assert_eq!(context["context"]["records"].as_array().unwrap().len(), 1);
}

#[test]
fn owner_tools_and_legacy_direct_methods_are_unreachable() {
    let f = Fixture::new();
    let mut server = f.server(true);
    for name in [
        "memory_save",
        "memory_context",
        "memory_graph",
        "shared_publish",
        "shared_revoke",
        "shared_inbox",
        "shared_review",
    ] {
        assert_eq!(call(&mut server, name, json!({}))["error"]["code"], -32602);
        let req = json!({"jsonrpc":"2.0","id":3,"method":name,"params":{}});
        assert_eq!(
            server.respond(&serde_json::to_vec(&req).unwrap()).unwrap()["error"]["code"],
            -32601
        );
    }
}

#[test]
fn observations_preserve_fixed_writer_and_never_enter_published_context() {
    let f = Fixture::new();
    let mut server = f.server(true);
    let before = f.store.context("alpha", 100).unwrap();
    for field in [
        "writer_id",
        "project_id",
        "memory_type",
        "status",
        "approved",
    ] {
        let mut forged = observation("event-1");
        forged[field] = json!("owner");
        assert_eq!(
            call(&mut server, "shared_observe", forged)["result"]["isError"],
            true
        );
    }
    let first = data(&call(&mut server, "shared_observe", observation("event-1")));
    let retry = data(&call(&mut server, "shared_observe", observation("event-1")));
    assert_eq!(first["id"], retry["id"]);
    assert_eq!(first["writer_id"], "researcher");
    assert_eq!(first["trust"], "untrusted_observation");
    assert_eq!(f.store.inbox("alpha", 100).unwrap().len(), 1);
    let after = f.store.context("alpha", 100).unwrap();
    assert_eq!(before.revision, after.revision);
    assert_eq!(before.records.len(), after.records.len());
    assert_eq!(
        call(&mut server, "shared_observe", observation("event-2"))["result"]["isError"],
        false
    );
    assert_eq!(
        call(&mut server, "shared_observe", observation("event-3"))["result"]["isError"],
        true
    );
    assert_eq!(
        call(&mut server, "shared_observe", observation("event-1"))["result"]["isError"],
        false
    );
}

#[test]
fn read_only_policy_hides_and_denies_observation_tool() {
    let f = Fixture::new();
    let mut server = f.server(false);
    assert!(!server.tools().to_string().contains("shared_observe"));
    assert_eq!(
        call(&mut server, "shared_observe", observation("event"))["error"]["code"],
        -32602
    );
    assert!(f.store.inbox("alpha", 10).unwrap().is_empty());
}

#[test]
fn notifications_do_not_execute_mutations_and_requests_need_initialization() {
    let f = Fixture::new();
    let mut server = f.server(true);
    let notification = json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"shared_observe","arguments":observation("event")}});
    assert!(
        server
            .respond(&serde_json::to_vec(&notification).unwrap())
            .is_none()
    );
    assert!(f.store.inbox("alpha", 10).unwrap().is_empty());
    server.ready = false;
    assert_eq!(
        call(&mut server, "shared_context", json!({}))["error"]["code"],
        -32600
    );
}

#[test]
fn bounded_framing_and_invalid_json_do_not_echo_request_content() {
    let f = Fixture::new();
    let mut server = f.server(true);
    let oversized = vec![b'x'; MAX_FRAME_BYTES + 2];
    let mut output = Vec::new();
    assert!(server.serve(Cursor::new(oversized), &mut output).is_err());
    assert!(output.is_empty());
    let response = server.respond(b"{SECRET_TEXT").unwrap();
    assert_eq!(response["error"]["code"], -32700);
    assert!(!response.to_string().contains("SECRET_TEXT"));
    assert_eq!(
        server
            .respond(br#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#)
            .unwrap()["error"]["code"],
        -32600
    );
}

#[test]
fn revocation_is_visible_to_existing_mcp_process() {
    let f = Fixture::new();
    let mut server = f.server(false);
    let before = data(&call(&mut server, "shared_context", json!({})));
    f.store.revoke("alpha", "offer").unwrap();
    let after = data(&call(&mut server, "shared_context", json!({})));
    assert!(
        after["context"]["revision"].as_i64().unwrap()
            > before["context"]["revision"].as_i64().unwrap()
    );
    assert!(after["context"]["records"].as_array().unwrap().is_empty());
    assert!(data(&call(&mut server, "shared_get", json!({"key":"offer"})))["record"].is_null());
}

#[test]
fn context_budget_marks_truncation_and_keeps_records_whole() {
    let f = Fixture::new();
    for n in 0..10 {
        f.store
            .publish(
                "alpha",
                &format!("long-{n}"),
                "Long fact",
                &"x".repeat(30000),
                "manual:v1",
            )
            .unwrap();
    }
    let mut server = f.server(false);
    let value = data(&call(&mut server, "shared_context", json!({"limit":100})));
    assert!(value.to_string().len() <= MAX_RESPONSE_BYTES);
    assert_eq!(value["context"]["truncated"], true);
    assert!(!value["context"]["records"].as_array().unwrap().is_empty());
}

#[test]
fn policy_is_strict_and_cannot_default_to_owner_authority() {
    assert!(
        toml::from_str::<Policy>(
            "version=1\nproject_id='alpha'\nagent_id='researcher'\nadmin=true"
        )
        .is_err()
    );
    let policy: Policy =
        toml::from_str("version=1\nproject_id='alpha'\nagent_id='researcher'").unwrap();
    assert!(!policy.allow_observations);
    assert!(policy.validate().is_ok());
    let policy: Policy =
        toml::from_str("version=1\nproject_id='../private'\nagent_id='researcher'").unwrap();
    assert!(policy.validate().is_err());
}

#[test]
fn standard_request_metadata_does_not_override_scope() {
    let f = Fixture::new();
    let mut server = f.server(false);
    let req = json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{
        "name":"shared_context","arguments":{},"_meta":{"progressToken":"trace-1","project_id":"beta"}
    }});
    let response = server.respond(&serde_json::to_vec(&req).unwrap()).unwrap();
    assert_eq!(data(&response)["context"]["project_id"], "alpha");
    assert!(!response.to_string().contains("CONFIDENTIAL_OTHER_PROJECT"));
}

#[test]
fn wire_budget_includes_escaped_tool_text_and_rpc_id() {
    let f = Fixture::new();
    for n in 0..4 {
        f.store
            .publish(
                "alpha",
                &format!("escaped-{n}"),
                "Escaped",
                &"\\".repeat(32768),
                "manual:v1",
            )
            .unwrap();
    }
    let mut server = f.server(false);
    for name in ["shared_context", "shared_get"] {
        let args = if name == "shared_get" {
            json!({"key":"escaped-0"})
        } else {
            json!({"limit":100})
        };
        let req = json!({"jsonrpc":"2.0","id":"x".repeat(200),"method":"tools/call","params":{"name":name,"arguments":args}});
        let response = server.respond(&serde_json::to_vec(&req).unwrap()).unwrap();
        assert_eq!(response["result"]["isError"], false);
        assert!(serde_json::to_vec(&response).unwrap().len() <= MAX_RESPONSE_BYTES);
    }
    let req = json!({"jsonrpc":"2.0","id":"x".repeat(300),"method":"ping"});
    assert_eq!(
        server.respond(&serde_json::to_vec(&req).unwrap()).unwrap()["error"]["code"],
        -32600
    );
}

fn policy_v2(principal: &str, agent: &str, writes: bool) -> Policy {
    Policy {
        version: 2,
        project_id: "alpha".into(),
        agent_id: agent.into(),
        allow_observations: writes,
        max_observations_per_session: 5,
        principal_id: Some(principal.into()),
        expires_at: None,
        max_session_secs: None,
    }
}

fn ready(store: &SharedStore, policy: Policy) -> SharedMcp<'_> {
    let mut server = SharedMcp::new(store, policy);
    server.respond(br#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#).unwrap();
    server.respond(br#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#);
    server
}

#[test]
fn policy_version_two_binds_every_agent_to_a_person() {
    assert!(policy_v2("ann", "ann-claude", false).validate().is_ok());
    // The writer id must carry its person, or two people could collide.
    for agent in ["claude", "ann", "ann-", "ben-claude", "annabel-claude"] {
        assert!(
            policy_v2("ann", agent, false).validate().is_err(),
            "{agent}"
        );
    }
    let mut missing = policy_v2("ann", "ann-claude", false);
    missing.principal_id = None;
    assert!(missing.validate().is_err());

    // `ann` + `ops-claude` and `ann-ops` + `claude` would both read as
    // `ann-ops-claude` and share one writer id, its quota and its request
    // ids. Only the second spelling is accepted.
    assert!(
        policy_v2("ann", "ann-ops-claude", false)
            .validate()
            .is_err()
    );
    assert!(
        policy_v2("ann-ops", "ann-ops-claude", false)
            .validate()
            .is_ok()
    );

    // Version 1 files cannot smuggle in version 2 fields, and stay strict.
    assert!(
        toml::from_str::<Policy>("version=1\nproject_id='alpha'\nagent_id='a'\nprincipal_id='ann'")
            .unwrap()
            .validate()
            .is_err()
    );
    let mut bad_time = policy_v2("ann", "ann-claude", false);
    bad_time.expires_at = Some("next friday".into());
    assert!(bad_time.validate().is_err());
    let mut bad_age = policy_v2("ann", "ann-claude", false);
    bad_age.max_session_secs = Some(0);
    assert!(bad_age.validate().is_err());
}

#[test]
fn revoking_a_person_ends_their_live_session_and_no_one_elses() {
    let f = Fixture::new();
    let mut ben = ready(&f.store, policy_v2("ben", "ben-claude", false));
    let ann = ready(&f.store, policy_v2("ann", "ann-claude", false));
    assert!(ben.check_access().is_ok());
    assert!(
        !data(&call(&mut ben, "shared_context", json!({})))["context"]["records"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    f.store
        .revoke_principal("alpha", "ben", Some("ann"))
        .unwrap();
    assert!(ben.check_access().is_err());
    assert!(ann.check_access().is_ok());

    // Through the real loop: a revoked person gets not one byte back.
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"shared_context","arguments":{}}}"#,
        "\n"
    );
    let mut output = Vec::new();
    let mut revoked = SharedMcp::new(&f.store, policy_v2("ben", "ben-codex", false));
    assert!(revoked.serve(Cursor::new(input), &mut output).is_err());
    assert!(output.is_empty(), "{}", String::from_utf8_lossy(&output));

    let mut output = Vec::new();
    let mut allowed = SharedMcp::new(&f.store, policy_v2("ann", "ann-codex", false));
    allowed.serve(Cursor::new(input), &mut output).unwrap();
    assert!(String::from_utf8_lossy(&output).contains("Public product details"));
}

#[test]
fn expired_policies_and_old_sessions_stop_working() {
    let f = Fixture::new();
    let mut expired = policy_v2("ann", "ann-claude", false);
    expired.expires_at = Some("2020-01-01T00:00:00Z".into());
    assert!(SharedMcp::new(&f.store, expired).check_access().is_err());

    let mut fresh = policy_v2("ann", "ann-claude", false);
    fresh.expires_at = Some("2999-01-01T00:00:00Z".into());
    fresh.max_session_secs = Some(1);
    let server = SharedMcp::new(&f.store, fresh);
    assert!(server.check_access().is_ok());
    std::thread::sleep(std::time::Duration::from_millis(1100));
    assert!(server.check_access().is_err());
}

#[test]
fn a_pending_draft_is_invisible_to_every_agent_of_both_people() {
    let f = Fixture::new();
    let mut writer = ready(&f.store, policy_v2("ben", "ben-contrib", true));
    let receipt = data(&call(
        &mut writer,
        "shared_observe",
        json!({"request_id":"r1","title":"Draft","content":"UNPROMOTED_SECRET_DRAFT","source":"call:1"}),
    ));
    assert_eq!(receipt["trust"], "untrusted_observation");
    assert!(!receipt.to_string().contains("UNPROMOTED_SECRET_DRAFT"));
    assert_eq!(
        f.store.inbox("alpha", 10).unwrap()[0]
            .principal_id
            .as_deref(),
        Some("ben")
    );

    for (principal, agent) in [("ann", "ann-claude"), ("ben", "ben-claude")] {
        let mut reader = ready(&f.store, policy_v2(principal, agent, false));
        for (tool, args) in [
            ("shared_context", json!({})),
            ("shared_search", json!({"query":"UNPROMOTED"})),
            ("shared_get", json!({"key":"Draft"})),
        ] {
            let reply = call(&mut reader, tool, args).to_string();
            assert!(
                !reply.contains("UNPROMOTED_SECRET_DRAFT"),
                "{principal} {tool}"
            );
        }
        // Curator verbs do not exist for agents, as tools or as methods.
        for name in [
            "shared_promote",
            "shared_pin",
            "shared_access_revoke",
            "shared_inbox",
        ] {
            assert_eq!(call(&mut reader, name, json!({}))["error"]["code"], -32602);
        }
    }
}

#[test]
fn serve_refuses_a_policy_for_another_project_on_a_pinned_database() {
    let dir = std::env::temp_dir().join(format!("mnemonic-shared-pin-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&dir).unwrap();
    let store = SharedStore::open(&dir.join("shared.db")).unwrap();
    store.pin_project("alpha").unwrap();
    let mut typo = policy_v2("ann", "ann-claude", false);
    typo.project_id = "alpah".into();
    let mut output = Vec::new();
    assert!(
        SharedMcp::new(&store, typo)
            .serve(Cursor::new("{}\n"), &mut output)
            .is_err()
    );
    assert!(output.is_empty());
    let _ = std::fs::remove_dir_all(dir);
}
