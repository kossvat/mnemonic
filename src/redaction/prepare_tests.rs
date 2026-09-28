//! Structured values and prepared entries.

use serde_json::{Value, json};

use super::tests::{body, hex, provider_token, shapes};
use super::*;
use crate::event::{EventSource, MemoryEntry, MemoryType};

fn no_fixture_in(value: &impl serde::Serialize) -> bool {
    let text = serde_json::to_string(value).unwrap();
    shapes().iter().all(|s| !text.contains(&s.secret))
}

#[test]
fn redaction_matcher_json_nested_values() {
    let token = provider_token();
    let metadata = json!({
        "source": "claude",
        "count": 3,
        "items": [{"note": format!("see {token}")}, [format!("OPENAI_API_KEY={}", body(32))]],
        "api_key": hex(40),
    });
    let r = redact_json(&metadata, &[], Limits::default()).unwrap();
    assert!(r.changed);
    assert!(no_fixture_in(&r.value));
    assert!(!serde_json::to_string(&r.value).unwrap().contains(&hex(40)));
    assert!(r.counts.get(Class::ProviderToken) == 1);
    assert!(r.counts.get(Class::CredentialAssignment) == 2);
    assert!(r.value["source"] == "claude");
    assert!(r.value["count"] == 3);
    assert!(r.value["api_key"] == CREDENTIAL_MARKER);
}

#[test]
fn redaction_matcher_json_sensitive_object_key_is_refused() {
    let token = provider_token();
    for key in [token.as_str(), "<private>a</private>"] {
        let mut map = serde_json::Map::new();
        map.insert(key.to_string(), json!("x"));
        let r = redact_json(&Value::Object(map), &[], Limits::default());
        assert!(r.unwrap_err() == RedactionError::SensitiveContent);
    }
}

#[test]
fn redaction_matcher_json_limits_refuse_the_whole_value() {
    let mut deep = json!("leaf");
    for _ in 0..40 {
        deep = json!([deep]);
    }
    let r = redact_json(&deep, &[], Limits::default());
    assert!(r.unwrap_err() == RedactionError::TooDeep);

    let many = json!((0..20).collect::<Vec<_>>());
    let few_nodes = Limits {
        max_nodes: 10,
        ..Limits::default()
    };
    assert!(redact_json(&many, &[], few_nodes).unwrap_err() == RedactionError::TooLarge);

    let little_text = Limits {
        max_text_bytes: 8,
        ..Limits::default()
    };
    let long = json!({"note": "more than eight bytes"});
    assert!(redact_json(&long, &[], little_text).unwrap_err() == RedactionError::TooLarge);
}

#[test]
fn redaction_matcher_json_structural_keys_are_refused_not_masked() {
    let token = provider_token();
    let dirty = json!({"project": format!("p-{token}")});
    assert!(
        redact_json(&dirty, &["project"], Limits::default()).unwrap_err()
            == RedactionError::SensitiveContent
    );
    let clean = json!({"project": "mnemonic"});
    let r = redact_json(&clean, &["project"], Limits::default()).unwrap();
    assert!(!r.changed && r.value == clean);
    // Not declared structural: it is narrative and gets masked.
    let r = redact_json(&dirty, &[], Limits::default()).unwrap();
    assert!(r.changed && no_fixture_in(&r.value));
}

#[test]
fn redaction_matcher_json_credential_context_needs_a_token_shape() {
    for value in [
        json!({"password": "hunter2"}),
        json!({"token": "correct horse battery staple 2024"}),
        json!({"token": CREDENTIAL_MARKER}),
        json!({"token": 12345678901234567890u64}),
        json!({"key": "mnemonic/decision/2026-09-24-long"}),
        json!({"idempotency_key": "4f1c2e3a-1111-2222-3333-444455556666"}),
    ] {
        let r = redact_json(&value, &[], Limits::default()).unwrap();
        assert!(!r.changed, "{value} was changed");
    }
}

fn entry(title: String, content: String, tags: Vec<String>, metadata: Value) -> MemoryEntry {
    let mut e = MemoryEntry::new(title, content, MemoryType::Note, EventSource::Manual);
    e.tags = tags;
    e.metadata = metadata;
    e
}

#[test]
fn redaction_matcher_prepare_entry_redacts_every_field() {
    let token = provider_token();
    let forged = json!({"policy_version": 1, "changed": false, "counts": {"jwt": 99}});
    let dirty = entry(
        format!("deploy with {token}"),
        format!("<private>notes</private> then OPENAI_API_KEY={}", body(32)),
        vec!["ok".into(), format!("tag-{token}")],
        json!({"redaction": forged, "note": "kept"}),
    );
    let prepared = prepare_entry(dirty.clone(), &[]).unwrap();
    assert!(no_fixture_in(prepared.entry()));
    let summary = prepared.summary();
    assert!(summary.changed());
    assert!(summary.counts().get(Class::ProviderToken) == 2);
    assert!(summary.counts().get(Class::PrivateBlock) == 1);
    assert!(summary.counts().get(Class::CredentialAssignment) == 1);
    assert!(
        summary.counts().get(Class::Jwt) == 0,
        "the forged count is gone"
    );
    let stored = &prepared.entry().metadata;
    assert!(stored[SUMMARY_KEY] == summary.to_json());
    assert!(stored[SUMMARY_KEY]["policy_version"] == POLICY_VERSION);
    assert!(stored["note"] == "kept");
    assert!(prepared.entry().id == dirty.id);
    assert!(prepared.entry().tags[0] == "ok");
}

/// The classifier's cut can turn a clean text into a credential shape:
/// preparing its result again masks the cut title, keeps the content, and
/// keeps and extends the summary admission wrote.
#[test]
fn redaction_matcher_reprepare_entry_masks_a_cut_title_and_keeps_the_summary() {
    let v = body(32);
    let content = format!(
        "nope, set password={v}_from_entropy() first, not {}",
        provider_token()
    );
    let admitted = prepare_entry(entry("t".into(), content, vec![], Value::Null), &[]).unwrap();
    let summary = admitted.entry().metadata[SUMMARY_KEY].clone();
    assert!(summary["counts"]["provider_token"] == 1);
    let mut classified = admitted.into_entry();
    assert!(
        classified.content.contains(&v),
        "a call is code, not a credential"
    );
    // The title is the content cut before the call's parenthesis.
    classified.title = format!("nope, set password={v}_from_entropy");
    let again = reprepare_entry(classified, &[]).unwrap();
    assert!(!again.entry().title.contains(&v));
    assert!(again.entry().content.contains(&v));
    let kept = &again.entry().metadata[SUMMARY_KEY];
    assert!(
        kept["counts"]["provider_token"] == 1,
        "the admission count survives"
    );
    assert!(
        kept["counts"]["credential_assignment"] == 1,
        "the cut title is counted"
    );
    assert!(check_entry(again.entry(), &[]) == Ok(()));
    // Nothing to do on a clean entry: the summary is kept as it is.
    let clean = entry("t".into(), "c".into(), vec![], summary_only(&summary));
    let same = reprepare_entry(clean, &[]).unwrap();
    assert!(same.entry().metadata[SUMMARY_KEY] == summary);
    // A forged summary does not survive either path.
    let forged = json!({"policy_version": 1, "changed": true, "counts": {"jwt": 9}, "x": 1});
    let e = entry("t".into(), "c".into(), vec![], summary_only(&forged));
    assert!(
        reprepare_entry(e, &[])
            .unwrap()
            .entry()
            .metadata
            .get(SUMMARY_KEY)
            .is_none()
    );
}

fn summary_only(summary: &Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(SUMMARY_KEY.to_string(), summary.clone());
    Value::Object(map)
}

/// What preparation admits passes the same limits at the write, summary
/// included: the limits leave room for it.
#[test]
fn redaction_matcher_prepared_entries_pass_the_write_check_at_the_limit() {
    let token = provider_token();
    let d = Limits::default();
    let fits = json!({"filler": vec![Value::Null; d.max_nodes - 20]});
    let prepared = prepare_entry(entry("t".into(), token.clone(), vec![], fits), &[]).unwrap();
    assert!(prepared.summary().changed());
    assert!(check_entry(prepared.entry(), &[]) == Ok(()));
    let full = json!({"filler": vec![Value::Null; d.max_nodes - 5]});
    assert!(
        prepare_entry(entry("t".into(), token, vec![], full), &[]).unwrap_err()
            == RedactionError::TooLarge
    );
}

#[test]
fn redaction_matcher_forged_summary_on_a_clean_entry_is_dropped() {
    let forged = json!({"policy_version": 1, "changed": true, "counts": {"jwt": 1}});
    let clean = entry("t".into(), "c".into(), vec![], json!({"redaction": forged}));
    let prepared = prepare_entry(clean, &[]).unwrap();
    assert!(!prepared.summary().changed());
    assert!(prepared.entry().metadata.get(SUMMARY_KEY).is_none());
}

#[test]
fn redaction_matcher_null_metadata_gets_a_summary() {
    let dirty = entry("t".into(), provider_token(), vec![], Value::Null);
    let prepared = prepare_entry(dirty, &[]).unwrap();
    let summary = &prepared.entry().metadata[SUMMARY_KEY];
    assert!(summary["counts"]["provider_token"] == 1);
    assert!(summary["changed"] == true);
}

#[test]
fn redaction_matcher_check_entry_refuses_and_accepts_prepared() {
    let dirty = entry("t".into(), provider_token(), vec![], json!({}));
    assert!(check_entry(&dirty, &[]) == Err(RedactionError::SensitiveContent));
    let dirty_meta = entry("t".into(), "c".into(), vec![], json!({"api_key": body(24)}));
    assert!(check_entry(&dirty_meta, &[]) == Err(RedactionError::SensitiveContent));
    let dirty_tag = entry("t".into(), "c".into(), vec![provider_token()], json!({}));
    assert!(check_entry(&dirty_tag, &[]) == Err(RedactionError::SensitiveContent));

    let prepared = prepare_entry(dirty, &[]).unwrap();
    assert!(check_entry(prepared.entry(), &[]) == Ok(()));
    // Preparing again finds nothing new: markers are not counted twice.
    let again = prepare_entry(prepared.into_entry(), &[]).unwrap();
    assert!(!again.summary().changed() && again.summary().counts().is_empty());
}

#[test]
fn redaction_matcher_summary_holds_no_matched_text() {
    let mut all = String::new();
    for s in shapes() {
        all.push_str(&s.text);
        all.push('\n');
    }
    let prepared = prepare_entry(entry("t".into(), all, vec![], Value::Null), &[]).unwrap();
    let summary = prepared.summary().to_json();
    let keys: Vec<&String> = summary.as_object().unwrap().keys().collect();
    assert!(keys == ["changed", "counts", "policy_version"]);
    assert!(no_fixture_in(&summary));
    for (name, n) in summary["counts"].as_object().unwrap() {
        assert!(
            Class::ALL.iter().any(|c| c.as_str() == name),
            "unknown class"
        );
        assert!(n.as_u64().is_some());
    }
}

/// A marker can be longer than what it replaces; preparation charges the
/// prepared size too, so what it admits passes the write's check.
#[test]
fn redaction_matcher_json_limits_count_the_prepared_text() {
    let key = ["AKIA", "ABCDEFGHJKLMNPQR"].concat();
    assert!(key.len() == 20);
    let exact = Limits {
        max_text_bytes: key.len(),
        ..Limits::default()
    };
    assert!(redact_json(&json!(key), &[], exact).unwrap_err() == RedactionError::TooLarge);
    let room = Limits {
        max_text_bytes: key.len() + 1,
        ..Limits::default()
    };
    let out = redact_json(&json!(key), &[], room).unwrap();
    assert!(out.value == json!("[REDACTED:credential]"));
}

fn pair(title: &str, content: &str) -> MemoryEntry {
    MemoryEntry::new(title, content, MemoryType::Note, EventSource::Manual)
}

/// A memory is read as a whole: by the model it is embedded by, by a
/// sink, by whoever recalls it. A title and a content that are each clean
/// can be a credential together, and the content is prepared as it reads
/// after its title.
#[test]
fn redaction_matcher_entry_is_prepared_as_it_reads() {
    let value = body(32);
    assert!(is_clean(&value) && is_clean("password:") && is_clean("Token"));
    // A name and its separator, and a name alone: a title says what the
    // content is.
    for title in ["password:", "Deploy password:", "Token", "the api_key"] {
        let prepared = prepare_entry(pair(title, &value), STRUCTURAL_KEYS).unwrap();
        let entry = prepared.entry();
        assert!(entry.title == title);
        assert!(entry.content == CREDENTIAL_MARKER);
        assert!(prepared.summary().counts().get(Class::CredentialAssignment) == 1);
        assert!(entry.metadata[SUMMARY_KEY]["counts"]["credential_assignment"] == 1);
        for join in [" ", "\n", ": "] {
            assert!(is_clean(&format!("{}{join}{}", entry.title, entry.content)));
        }
        check_entry(entry, STRUCTURAL_KEYS).unwrap();
        // Prepared again it is as it is, and counted once.
        let again = reprepare_entry(entry.clone(), STRUCTURAL_KEYS).unwrap();
        assert!(
            serde_json::to_value(again.entry()).unwrap() == serde_json::to_value(entry).unwrap()
        );
    }
    // The rest of the content is kept.
    let text = format!("{value} is what the staging deploy uses");
    let prepared = prepare_entry(pair("password:", &text), STRUCTURAL_KEYS).unwrap();
    let kept = format!("{CREDENTIAL_MARKER} is what the staging deploy uses");
    assert!(prepared.entry().content == kept);
}

/// What reads as nothing together is kept as it is: another title, a
/// value that is a time, a content that is a call.
#[test]
fn redaction_matcher_entry_that_reads_clean_is_unchanged() {
    let value = body(32);
    let kept = [
        pair("Build id", &value),
        pair("password policy", &value),
        pair("token:", "2026-12-01T00:00:00Z"),
        pair("Token expires", "2026-12-01"),
        pair("password:", "see the vault"),
        pair("password:", &format!("{value}(x)")),
    ];
    for entry in kept {
        let prepared = prepare_entry(entry.clone(), STRUCTURAL_KEYS).unwrap();
        assert!(prepared.entry().title == entry.title);
        assert!(prepared.entry().content == entry.content);
        assert!(!prepared.summary().changed());
        assert!(prepared.entry().metadata.get(SUMMARY_KEY).is_none());
        check_entry(prepared.entry(), STRUCTURAL_KEYS).unwrap();
    }
}

/// A word that announces a credential, at the end of a title: what
/// follows it in the content is masked, and the title is kept.
#[test]
fn redaction_matcher_entry_whose_title_announces_what_the_content_holds() {
    let token = body(32);
    for title in ["Authorization: Bearer", "Authorization:", "Bearer"] {
        assert!(is_clean(title) && is_clean(&token));
        let prepared = prepare_entry(pair(title, &token), STRUCTURAL_KEYS).unwrap();
        let entry = prepared.entry();
        assert!(entry.title == title && entry.content == CREDENTIAL_MARKER);
        // Found once, counted once.
        assert!(prepared.summary().counts().total() == 1);
        check_entry(entry, STRUCTURAL_KEYS).unwrap();
    }
}

/// A sink writes the content under the title, a line apart. To the policy
/// a line break between them is a space: what reads clean one way reads
/// clean the other, and what is a title alone opens nothing (a key block
/// or a private region that is begun is not clean alone).
#[test]
fn redaction_matcher_a_line_between_title_and_content_reads_as_a_space() {
    let value = body(32);
    let titles = [
        "password:",
        "password",
        "password =",
        "Token",
        "token=",
        "api_key:",
        "SECRET_KEY",
        "DB_PASSWORD=",
        "Authorization:",
        "Authorization: Bearer",
        "Bearer",
        "Build id",
        "see https://user:",
        "Notes",
    ];
    let contents = [
        value.clone(),
        format!("= {value}"),
        format!(": {value}"),
        format!("Bearer {value}"),
        format!("{value}@host.example/path"),
        format!("{value} and more words"),
        "plain words".to_string(),
    ];
    for title in titles {
        for content in &contents {
            let spaced = is_clean(&format!("{title} {content}"));
            let lined = is_clean(&format!("{title}\n{content}"));
            assert!(spaced == lined, "a line break reads otherwise than a space");
        }
    }
    let begun = [
        ["-----BEGIN ", "PRIVATE KEY-----"].concat(),
        "note <private>start".to_string(),
    ];
    for title in begun {
        assert!(!is_clean(&title));
    }
}

/// The check at a write refuses what was not prepared as it reads.
#[test]
fn redaction_matcher_check_entry_refuses_a_pair_that_reads_as_a_credential() {
    let value = body(32);
    for title in ["password:", "Token"] {
        let refused = check_entry(&pair(title, &value), STRUCTURAL_KEYS);
        assert!(refused == Err(RedactionError::SensitiveContent));
    }
    check_entry(&pair("Build id", &value), STRUCTURAL_KEYS).unwrap();
    check_entry(&pair("token:", "2026-12-01T00:00:00Z"), STRUCTURAL_KEYS).unwrap();
}
