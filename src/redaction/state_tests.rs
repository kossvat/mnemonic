//! Admission helpers for state that is not a memory. Credential fixtures
//! are assembled at run time; assertions never print them.
use super::state::*;
use super::*;

fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

fn token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

#[test]
fn redaction_state_identities_are_refused_with_a_fixed_code() {
    let token = token();
    assert!(check_identities(["demoapp", "api", "2026-01-02T03:04:05Z"]).is_ok());
    assert!(check_identities(std::iter::empty()).is_ok());
    for dirty in [
        token.clone(),
        format!("password={}", body(24)),
        format!("p <private>{}</private>", body(8)),
    ] {
        assert!(
            check_identities(["demoapp", dirty.as_str()]).unwrap_err()
                == RedactionError::SensitiveContent
        );
        let error = admit_identities("peer", [dirty.as_str()]).unwrap_err();
        assert!(is_refused(&error));
        let text = format!("{error:#} {error:?}");
        assert!(text.contains("peer refused (SENSITIVE_CONTENT)"));
        assert!(!text.contains(&dirty) && !text.contains(&body(8)));
    }
    assert!(!is_refused(&anyhow::anyhow!("the store failed")));
}

/// `password=<v>(arg)` reads as a call and stays; cut right after the value
/// it reads as a credential. The cut text is prepared again.
#[test]
fn redaction_state_narrative_is_prepared_again_after_its_cut() {
    let value = body(24);
    let text = format!("see password={value}(arg) for the rest");
    assert!(is_clean(&text), "the fixture must be clean before the cut");
    let cap = "see password=".len() + value.len();
    let prepared = prepare_capped(&text, cap);
    assert!(prepared.changed);
    assert!(!prepared.value.contains(&value));
    assert!(is_clean(&prepared.value));
    assert!(prepared.value.chars().count() <= cap);
    assert!(prepared.counts.get(Class::CredentialAssignment) == 1);
}

#[test]
fn redaction_state_clean_narrative_is_only_normalized() {
    let prepared = prepare_normalized("  two   words \n next line", |t| {
        t.lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    });
    assert!(prepared.value == "two words");
    assert!(!prepared.changed && prepared.counts.is_empty());
    let kept = prepare_capped("short", 300);
    assert!((kept.value.as_str(), kept.changed) == ("short", false));
}

#[test]
fn redaction_state_narrative_with_a_credential_keeps_the_rest() {
    let token = token();
    let prepared = prepare_capped(&format!("deploy with {token} on friday"), 300);
    assert!(prepared.value == format!("deploy with {CREDENTIAL_MARKER} on friday"));
    assert!(prepared.counts.get(Class::ProviderToken) == 1);
}

#[test]
fn redaction_state_a_message_shows_an_id_only_when_it_is_admitted() {
    let token = token();
    assert!(shown("3f2a-91") == "3f2a-91");
    assert!(!shown(&token).contains(&token));
    assert!(is_clean(shown(&token)));
}

/// Names are stored and matched without case; the shapes of the policy
/// are not. An identity is judged in lower case too.
#[test]
fn redaction_state_identities_are_judged_lowercased_too() {
    let upper = token().to_uppercase();
    assert!(
        is_clean(&upper),
        "the fixture must be clean as it is written"
    );
    assert!(!is_clean(&upper.to_lowercase()));
    assert!(check_identities([upper.as_str()]) == Err(RedactionError::SensitiveContent));
    assert!(!shown(&upper).contains(&upper));
    for plain in ["Demo App", "GITHUB_TOKEN", "README.md", "Widget-2"] {
        assert!(check_identities([plain]).is_ok());
    }
}
