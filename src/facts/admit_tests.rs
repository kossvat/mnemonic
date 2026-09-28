//! What a fact may hold. Credential fixtures are assembled at run time;
//! assertions never print them.
use super::*;
use crate::redaction::is_clean;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

fn base<'a>() -> FactWrite<'a> {
    FactWrite {
        project: Some("demoapp"),
        subject: "api",
        predicate: "host",
        qualifier: Some("staging"),
        value: Some("db.internal"),
        as_of: Some("2026-01-02T03:04:05Z"),
        actor: "test",
        agent: Some("claude"),
        request_id: Some("req-1"),
        ..Default::default()
    }
}

/// The part a refused statement names, or "" for an admitted one.
fn part(write: &FactWrite<'_>) -> &'static str {
    match check(write) {
        Ok(()) => "",
        Err(refused) => {
            assert!(refused.code == RedactionError::SensitiveContent);
            refused.part
        }
    }
}

#[test]
fn redaction_state_fact_fields_are_each_judged_as_given() {
    assert!(check(&base()).is_ok());
    let retraction = FactWrite {
        value: None,
        ..base()
    };
    assert!(check(&retraction).is_ok());
    let token = token();
    let t = Some(token.as_str());
    let dirty = [
        (
            FactWrite {
                project: t,
                ..base()
            },
            "in what names the fact",
        ),
        (
            FactWrite {
                subject: &token,
                ..base()
            },
            "in the statement as it reads",
        ),
        (
            FactWrite {
                predicate: &token,
                ..base()
            },
            "in the statement as it reads",
        ),
        (
            FactWrite {
                qualifier: t,
                ..base()
            },
            "in the statement as it reads",
        ),
        (FactWrite { value: t, ..base() }, "in the value"),
        (FactWrite { as_of: t, ..base() }, "in what names the fact"),
        (
            FactWrite {
                actor: &token,
                ..base()
            },
            "in what names the fact",
        ),
        (FactWrite { agent: t, ..base() }, "in what names the fact"),
        (
            FactWrite {
                source_memory_id: t,
                ..base()
            },
            "in what names the fact",
        ),
        (
            FactWrite {
                request_id: t,
                ..base()
            },
            "in what names the fact",
        ),
    ];
    for (i, (write, expected)) in dirty.iter().enumerate() {
        assert!(part(write) == *expected, "field {i}");
        let error = admit(write).unwrap_err();
        assert!(crate::redaction::state::is_refused(&error), "field {i}");
        let text = format!("{error:#} {error:?}");
        assert!(!text.contains(&token), "field {i} is echoed");
        assert!(
            text.contains(&format!("fact refused (SENSITIVE_CONTENT): {expected}")),
            "field {i}"
        );
    }
}

/// A value is compared and stored lowercased as well (`value_norm`): it is
/// judged in that form too.
#[test]
fn redaction_state_fact_value_is_judged_lowercased_too() {
    let upper = token().to_uppercase();
    assert!(
        is_clean(&upper),
        "the fixture must be clean as it is written"
    );
    let write = FactWrite {
        value: Some(&upper),
        ..base()
    };
    assert!(part(&write) == "in the value");
    let named = FactWrite {
        subject: &upper,
        ..base()
    };
    assert!(part(&named) == "in the statement as it reads");
    let scoped = FactWrite {
        project: Some(&upper),
        ..base()
    };
    assert!(part(&scoped) == "in what names the fact");
}

/// A value is stored as given and as it is compared, which folds case and
/// every kind of space: a no-break space hides an assignment from the
/// policy as it is written, not as it is compared.
#[test]
fn redaction_state_fact_value_is_judged_as_it_is_compared_too() {
    let inside = format!("password\u{a0}={}", body(24));
    assert!(
        is_clean(&inside),
        "the fixture must be clean as it is written"
    );
    assert!(!is_clean(&keys::value_norm(&inside)));
    let write = FactWrite {
        value: Some(&inside),
        ..base()
    };
    assert!(part(&write) == "in the value");
    // Clean in both forms by itself; under a credential's name only the
    // form it is compared by shows the value right after the name.
    let after = format!("\u{a0}{}", body(24));
    assert!(is_clean(&after) && is_clean(&keys::value_norm(&after)));
    let named = FactWrite {
        predicate: "password",
        qualifier: None,
        value: Some(&after),
        ..base()
    };
    assert!(is_clean(&statement(&named).0) && is_clean(&statement(&named).1));
    assert!(part(&named) == "the value under one of the fact's names");
    let plain = FactWrite {
        value: Some(&after),
        ..base()
    };
    assert!(check(&plain).is_ok());
}

/// `check` judges four fields through the statement: it must show them.
#[test]
fn redaction_state_fact_statement_shows_every_field_it_stands_for() {
    let write = FactWrite {
        subject: " Widget ",
        predicate: " list price ",
        qualifier: Some(" eu "),
        value: Some("v2 build"),
        ..base()
    };
    for text in [statement(&write).0, statement(&write).1] {
        for field in ["Widget", "list price", "eu", "v2 build"] {
            assert!(text.contains(field), "{text:?} lacks {field:?}");
        }
    }
    let retracted = statement(&FactWrite {
        value: None,
        ..write
    });
    for text in [retracted.0, retracted.1] {
        for field in ["Widget", "list price", "eu"] {
            assert!(text.contains(field), "{text:?} lacks {field:?}");
        }
    }
}

/// Neither field is a credential on its own; under its name the value is,
/// whatever the statement puts between them.
#[test]
fn redaction_state_fact_value_is_judged_under_each_of_its_names() {
    let value = body(24);
    assert!(is_clean(&value));
    for name in ["password", "deploy token", "api_key", "token"] {
        assert!(is_clean(name));
    }
    let under = "the value under one of the fact's names";
    for write in [
        // With a qualifier between the name and the value, and without.
        FactWrite {
            predicate: "password",
            value: Some(&value),
            ..base()
        },
        FactWrite {
            predicate: "password",
            qualifier: None,
            value: Some(&value),
            ..base()
        },
        // A fact that names the credential in its subject and calls the
        // predicate something else.
        FactWrite {
            subject: "deploy token",
            predicate: "value",
            value: Some(&value),
            ..base()
        },
        FactWrite {
            qualifier: Some("api_key"),
            value: Some(&value),
            ..base()
        },
        // The command line shows the project in brackets before the value.
        FactWrite {
            project: Some("token"),
            value: Some(&value),
            ..base()
        },
    ] {
        let found = part(&write);
        assert!(found == under || found == "in the statement as it reads");
    }
    // The same value under names that are no credential's is a fact.
    let plain = FactWrite {
        predicate: "build id",
        value: Some(&value),
        ..base()
    };
    assert!(check(&plain).is_ok());
}

/// A fact about a credential is a fact: when it expires, when it was
/// rotated. Under a name the statement does not put right before it, a
/// time is not a credential.
#[test]
fn redaction_state_fact_about_a_credential_may_state_a_time() {
    for time in ["2026-12-01T00:00:00Z", "2026-12-01 10:30:00", "2026-12-01"] {
        for write in [
            FactWrite {
                subject: "GITHUB_TOKEN",
                predicate: "expires",
                value: Some(time),
                ..base()
            },
            FactWrite {
                subject: "deploy token",
                predicate: "rotated",
                value: Some(time),
                ..base()
            },
            FactWrite {
                qualifier: Some("api_key"),
                predicate: "rotated",
                value: Some(time),
                ..base()
            },
        ] {
            assert!(check(&write).is_ok(), "{time}");
            // What its memory would show is what was stated.
            let (title, content) = statement(&write);
            assert!(is_clean(&title) && is_clean(&content), "{time}");
        }
    }
    // A statement that reads as a credential is one, whatever its value
    // parses as: its memory could not show it as stated.
    let reads = FactWrite {
        predicate: "password",
        qualifier: None,
        value: Some("2026-12-01T00:00:00Z"),
        ..base()
    };
    assert!(part(&reads) == "in the statement as it reads");
    // What is not a time is judged as ever.
    let value = body(24);
    let write = FactWrite {
        subject: "GITHUB_TOKEN",
        predicate: "expires",
        value: Some(&value),
        ..base()
    };
    assert!(part(&write) == "the value under one of the fact's names");
}

/// Two names that are clean apart and a credential side by side, as the
/// statement's memory would show them.
#[test]
fn redaction_state_fact_statement_is_judged_whole() {
    let name = body(24);
    assert!(is_clean(&name) && is_clean("Bearer"));
    let write = FactWrite {
        subject: "Bearer",
        predicate: &name,
        qualifier: None,
        ..base()
    };
    assert!(part(&write) == "in the statement as it reads");
    let other = FactWrite {
        subject: "Carrier",
        ..write
    };
    assert!(check(&other).is_ok());
}

/// The names as the store files them: a key, an alias target, a project
/// as resolved, the spelling of the slot that is already there.
#[test]
fn redaction_state_fact_is_judged_as_the_store_would_file_it() {
    type Edit = fn(&mut Resolved);
    let value = body(24);
    let token = token();
    let resolved = |edit: Edit| {
        let mut r = Resolved {
            scope_key: "demoapp".into(),
            scope: "demoapp".into(),
            subject_key: "api".into(),
            subject_target: None,
            predicate: "host".into(),
            qualifier_key: "staging".into(),
            labels: vec![],
        };
        edit(&mut r);
        r
    };
    let write = FactWrite {
        value: Some(&value),
        ..base()
    };
    assert!(check(&write).is_ok());
    assert!(check_resolved(&write, &resolved(|_| {})).is_ok());
    let cases: [Edit; 6] = [
        |r| r.predicate = "api-key".into(),
        |r| r.qualifier_key = "api-key".into(),
        |r| r.subject_key = "token".into(),
        |r| r.scope = "secret".into(),
        |r| r.scope_key = "secret".into(),
        |r| r.labels = vec!["DEPLOY_TOKEN".into()],
    ];
    for (i, edit) in cases.into_iter().enumerate() {
        let found = check_resolved(&write, &resolved(edit)).unwrap_err();
        assert!(
            found.part == "the value under one of the fact's names",
            "case {i}"
        );
    }
    // A name that resolves to a credential refuses the statement whatever
    // its value, a retraction included.
    for value in [Some("db.internal"), None] {
        let write = FactWrite { value, ..base() };
        for field in 0..3 {
            let mut r = resolved(|_| {});
            match field {
                0 => r.scope = token.clone(),
                1 => r.subject_target = Some(token.clone()),
                _ => r.labels = vec![token.clone()],
            }
            let found = check_resolved(&write, &r).unwrap_err();
            assert!(
                found.part == "in a name as the store resolves it",
                "field {field}"
            );
        }
    }
}

#[test]
fn redaction_state_fact_evidence_is_prepared_within_its_cap() {
    let token = token();
    let kept = evidence(&format!("seen in the deploy log with {token}"));
    assert!(!kept.contains(&token) && kept.contains("[REDACTED:credential]"));
    let long = evidence(&"word ".repeat(400));
    assert!(long.chars().count() == MAX_EVIDENCE);
    assert!(evidence("plain") == "plain");
}
