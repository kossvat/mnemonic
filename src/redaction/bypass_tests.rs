//! Regressions from the adversarial review of policy version 1: inputs that
//! used to leak, to corrupt ordinary text, or to take quadratic time.

use serde_json::{Value, json};

use super::tests::{b64url, body, hex, provider_token};
use super::*;
use crate::event::{EventSource, MemoryEntry, MemoryType};

fn fal() -> String {
    ["1a2b3c4d-1111-2222-3333-444455556666", ":", &hex(32)].concat()
}

/// `text` loses `secret` and nothing else changes around the marker count.
fn leaks_nothing(text: &str, secret: &str) -> bool {
    let r = redact_text(text);
    !r.value.contains(secret) && redact_text(&r.value).value == r.value
}

#[test]
fn redaction_matcher_review_leaks_are_closed() {
    let v = body(32);
    let fal = fal();
    let pw = body(16);
    let cases: Vec<(&str, String, String)> = vec![
        (
            "authorization key",
            format!("Authorization: Key {fal}"),
            fal.clone(),
        ),
        (
            "curl header",
            format!("curl -H \"Authorization: Key {fal}\" x"),
            fal.clone(),
        ),
        (
            "bearer with colon",
            format!("Authorization: Bearer {fal}"),
            hex(32),
        ),
        (
            "bearer on next line",
            format!("Authorization: Bearer\n{v}"),
            v.clone(),
        ),
        (
            "bearer continuation",
            format!("Authorization: Bearer \\\n{v}"),
            v.clone(),
        ),
        (
            "escaped newline",
            format!("echo -e \"FOO=1\\nGITHUB_TOKEN={v}\" >> .env"),
            v.clone(),
        ),
        ("escaped tab", format!("x\\tpassword: {v}"), v.clone()),
        (
            "escaped json",
            format!("{{\\\"api_key\\\":\\\"{}\\\"}}", hex(40)),
            hex(40),
        ),
        (
            "docker escaped",
            format!("docker run -e \"OPENAI_API_KEY=\\\"{v}\\\"\" img"),
            v.clone(),
        ),
        (
            "default value url",
            format!("${{DATABASE_URL:-postgres://app:{pw}@db/app}}"),
            pw.clone(),
        ),
        (
            "dash scheme",
            format!("x -https://app:{pw}@db/app"),
            pw.clone(),
        ),
        (
            "digit scheme",
            format!("use 1.redis://:{pw}@cache:6379"),
            pw.clone(),
        ),
        (
            "paren password",
            format!("postgres://admin:Se{pw}(2024)x@db.internal/app"),
            pw.clone(),
        ),
        (
            "quote password",
            format!("postgres://admin:it's{pw}@db.internal/app"),
            pw.clone(),
        ),
        ("cli flag", format!("mytool --token={v}"), v.clone()),
        (
            "cli api key",
            format!("mytool --api-key={}", hex(40)),
            hex(40),
        ),
        ("makefile", format!("API_TOKEN ?= {v}"), v.clone()),
        ("backticks", format!("`GITHUB_TOKEN`: `{v}`"), v.clone()),
        ("backtick value", format!("FAL_KEY=`{fal}`"), fal.clone()),
        (
            "subscript",
            format!("os.environ[\"FAL_KEY\"] = \"{fal}\""),
            fal.clone(),
        ),
        (
            "header subscript",
            format!("headers[\"X-Api-Key\"] = \"{}\"", hex(40)),
            hex(40),
        ),
        ("guillemets", format!("«токен»: {v}"), v.clone()),
        ("slack xoxe", ["xo", "xe-1-", &body(60)].concat(), body(60)),
        ("slack xoxc", ["xo", "xc-", &body(60)].concat(), body(60)),
        ("slack app", ["xa", "pp-1-", &body(40)].concat(), body(40)),
        (
            "akia glued",
            ["AK", "IA", "ABCDEFGH12345678", "_prod"].concat(),
            "ABCDEFGH12345678".into(),
        ),
        (
            "connection string",
            format!("Server=db;Uid=app;Pwd={v};"),
            v.clone(),
        ),
        (
            "paren enclosed url",
            format!("(postgres://app:{pw}@db)"),
            pw.clone(),
        ),
        (
            "markdown link, paren password",
            format!("[db](postgres://app:Se{pw}(2024)x@db.internal/app)"),
            pw.clone(),
        ),
        ("chained", format!("password=token=\"{v}\""), v.clone()),
        (
            "inner name after an exempt path",
            format!("PWD=/tmp?api_key={v}"),
            v.clone(),
        ),
        (
            "newline before separator",
            format!("{{\n  \"api_key\"\n  : \"{v}\"\n}}"),
            v.clone(),
        ),
        (
            "dotted property name",
            format!("spring.datasource.password={v}"),
            v.clone(),
        ),
        (
            "escaped quote in password",
            format!("dsn = 'postgres://app:pa\\'ss{pw}@db/app'"),
            pw.clone(),
        ),
        (
            "doubled scheme",
            format!("Authorization: Bearer Bearer {v}"),
            v.clone(),
        ),
        (
            "letters-only token",
            "Authorization: abcdefghijklmnopqrstuvwx next".into(),
            "abcdefghijklmnopqrstuvwx".into(),
        ),
        (
            "folded basic",
            format!("Authorization: Basic\r\n\t{v}"),
            v.clone(),
        ),
        (
            "folded key",
            format!("Authorization: Key\n  {fal}"),
            fal.clone(),
        ),
        (
            "value on next line",
            format!("\"api_key\":\n\"{v}\""),
            v.clone(),
        ),
        ("yaml next line", format!("password:\n  {v}"), v.clone()),
        ("guillemet value", format!("пароль: «{v}»"), v.clone()),
        (
            "tokens glued by _",
            [&["gh", "p_"].concat(), &body(36), "_", &provider_token()].concat(),
            provider_token(),
        ),
        (
            "first glued token",
            [&["gh", "p_"].concat(), &body(36), "_x"].concat(),
            body(36),
        ),
    ];
    for (name, text, secret) in cases {
        assert!(leaks_nothing(&text, &secret), "{name}");
    }
}

#[test]
fn redaction_matcher_review_jwt_variants() {
    let spaced = b64url(br#"{ "alg": "HS256" }"#);
    let leading = b64url(br#" {"alg":"HS256"}"#);
    let tabbed = b64url(b"{\n\t\"alg\": \"HS256\"\n}");
    let payload = b64url(br#"{"sub":"1234"}"#);
    let padded = format!("{payload}==");
    let sig = body(24);
    for (name, header, payload) in [
        ("spaced header", &spaced, &payload),
        ("leading space header", &leading, &payload),
        ("newline header", &tabbed, &payload),
        ("padded payload", &b64url(br#"{"alg":"HS256"}"#), &padded),
    ] {
        let jwt = format!("{header}.{payload}.{sig}");
        let r = redact_text(&format!("token {jwt} end"));
        assert!(!r.value.contains(&sig), "{name}");
        assert!(r.counts.get(Class::Jwt) == 1, "{name}");
    }
}

#[test]
fn redaction_matcher_review_pem_footer_matches_its_label() {
    let (k1, k2) = (body(48), hex(48));
    let text = [
        "-----BEGIN RSA ",
        "PRIVATE KEY-----\n",
        &k1,
        "\n-----END RSA PRIVATE KEY-----\nmiddle stays\n-----BEGIN EC ",
        "PRIVATE KEY-----\n",
        &k2,
        "\n-----END EC PRIVATE KEY-----\nafter",
    ]
    .concat();
    let r = redact_text(&text);
    assert!(!r.value.contains(&k1) && !r.value.contains(&k2));
    assert!(r.value.contains("middle stays") && r.value.ends_with("after"));
    assert!(r.counts.get(Class::PrivateKey) == 2);

    // A header whose footer never comes swallows everything after it,
    // including a later block and an unrelated END line.
    let text = [
        "-----BEGIN RSA ",
        "PRIVATE KEY-----\nabc\n-----END CERTIFICATE-----\n-----BEGIN EC ",
        "PRIVATE KEY-----\n",
        &k2,
        "\n-----END EC PRIVATE KEY-----",
    ]
    .concat();
    assert!(leaks_nothing(&text, &k2));
}

#[test]
fn redaction_matcher_review_private_tags() {
    for text in [
        "<private reason=\"a > </private>\">hidden prose</private> tail",
        "<private reason = \"personal\">hidden prose</private> tail",
        "<private a='1'  b = 2>hidden prose</private> tail",
    ] {
        let r = redact_text(text);
        assert!(!r.value.contains("hidden") && r.value.ends_with(" tail"));
    }

    for placeholder in [
        "ssh -i <private key> ubuntu@<private IP>\nThen run make deploy.",
        "openssl rsa -in <PRIVATE KEY FILE> -pubout. Next step: commit.",
    ] {
        assert!(is_clean(placeholder), "a placeholder deleted text");
    }

    // A dropped orphan closer must not splice its neighbours into a tag.
    let oversized = format!(
        "<private a=\"{}\">hidden</private> rest",
        ["sk-", &body(600)].concat()
    );
    for text in [
        "x <priv</private>ate> hello",
        "x </priv</private>ate> y",
        "prefix <private</private>>hidden prose",
        "<private </private>>hidden prose",
        oversized.as_str(),
    ] {
        let once = redact_text(text);
        assert!(
            redact_text(&once.value).value == once.value,
            "not idempotent"
        );
    }
}

#[test]
fn redaction_matcher_review_code_and_paths_stay() {
    for text in [
        "token = base64.urlsafe_b64encode(os.urandom(32))",
        "token = oauth2_session.fetch_token(url)",
        "let secret = hmac_sha256_derive_key(&master, b\"ctx\");",
        "secret: process.env.AUTH0_CLIENT_SECRET,",
        "api_key=settings.openai_api_key_v2",
        "def install_npm_dependencies_offline():",
        "scripts/run_npm_install_with_retries.sh",
        "npm_config_global_prefix=/opt/homebrew",
        "PWD=/home/runner/work/app2/app2",
        "Authorization: Bearer token",
        "Authorization: Bearer $OPENAI_API_KEY",
        "some_ghs_foo_bar_baz_qux_quux_corge",
        "('http://localhost:8080','user@localhost')",
        "f(http://localhost:8080,user@localhost)",
        "'http://localhost:8080'+'user@localhost'",
        "(http://localhost:8080)(user@localhost)",
    ] {
        assert!(is_clean(text));
    }
}

#[test]
fn redaction_matcher_review_json_rules() {
    let d = Limits::default();
    let masked = |value: Value| {
        let r = redact_json(&value, &[], d).unwrap();
        let text = serde_json::to_string(&r.value).unwrap();
        (r.changed, text)
    };
    let (changed, text) = masked(json!({"пароль": body(32)}));
    assert!(changed && !text.contains(&body(32)));
    let url = format!("postgres://user:{};remaining@db/app", body(12));
    let (changed, text) = masked(json!({ "secret": url }));
    assert!(
        changed && !text.contains("remaining"),
        "part of a URL password survived"
    );
    let (changed, text) = masked(json!({"пароль": format!("«{}»", body(32))}));
    assert!(
        changed && !text.contains(&body(32)),
        "guillemets hid a value"
    );
    let (changed, text) = masked(json!({"api_key": format!("\n  {}", hex(32))}));
    assert!(
        changed && !text.contains(&hex(32)),
        "leading whitespace hid a value"
    );
    let (changed, text) = masked(json!({"Authorization": format!("Key {}", fal())}));
    assert!(changed && !text.contains(&hex(32)) && text.contains("Key "));
    for value in [
        json!({"api_key": [hex(40)]}),
        json!({"client_secret": [[fal()]]}),
        json!({"password": {"value": body(24)}}),
    ] {
        let (changed, text) = masked(value);
        assert!(changed && !text.contains(&body(24)[..20]) && !text.contains(&hex(32)));
    }
    let (changed, _) = masked(json!({"token": "parser.next_significant_token(1)"}));
    assert!(!changed, "code under a credential name");
    let (changed, _) = masked(json!({"pwd": "/srv/app2/current"}));
    assert!(!changed, "a working directory");

    // Structural beats credential: refuse, never rewrite an identity.
    let r = redact_json(&json!({"token": body(32)}), &["token"], d);
    assert!(r.unwrap_err() == RedactionError::SensitiveContent);

    // Every node counts, whatever key it sits under.
    let one = Limits { max_nodes: 1, ..d };
    let r = redact_json(&json!({"api_key": "clean"}), &[], one);
    assert!(r.unwrap_err() == RedactionError::TooLarge);
    let flat = Limits { max_depth: 0, ..d };
    let r = redact_json(&json!({"project": "p"}), &["project"], flat);
    assert!(r.unwrap_err() == RedactionError::TooDeep);
}

fn with_summary(summary: Value) -> Value {
    let mut map = serde_json::Map::new();
    map.insert(SUMMARY_KEY.to_string(), summary);
    Value::Object(map)
}

fn entry(id: &str, metadata: Value) -> MemoryEntry {
    let mut e = MemoryEntry::new("t", "c", MemoryType::Note, EventSource::Manual);
    e.id = id.to_string();
    e.metadata = metadata;
    e
}

#[test]
fn redaction_matcher_review_entry_identity_and_summary() {
    let dirty_id = entry(&format!("id-{}", provider_token()), json!({}));
    assert!(check_entry(&dirty_id, &[]) == Err(RedactionError::SensitiveContent));
    assert!(prepare_entry(dirty_id, &[]).unwrap_err() == RedactionError::SensitiveContent);

    let forged =
        json!({"policy_version": 1, "changed": true, "counts": {"private_key": 5}, "extra": 1});
    let e = entry("ok", with_summary(forged));
    assert!(check_entry(&e, &[]) == Err(RedactionError::InvalidSummary));
    let old_policy = json!({"policy_version": 0, "changed": true, "counts": {"jwt": 1}});
    let e = entry("ok", with_summary(old_policy));
    assert!(check_entry(&e, &[]) == Err(RedactionError::InvalidSummary));

    let mut dirty = entry("ok", Value::Null);
    dirty.content = provider_token();
    let prepared = prepare_entry(dirty, &[]).unwrap();
    assert!(check_entry(prepared.entry(), &[]) == Ok(()));
}

/// Each pattern that used to rescan the rest of the input per match. A
/// quadratic scan of 256 KiB takes minutes; linear takes milliseconds. The
/// bound is loose on purpose: it catches the complexity class, not speed,
/// and looser still in a build that is not optimized.
#[test]
fn redaction_matcher_review_adversarial_inputs_stay_linear() {
    let bound = std::time::Duration::from_secs(if cfg!(debug_assertions) { 20 } else { 2 });
    let n = 256 << 10;
    let fill = |unit: &str| unit.repeat(n / unit.len());
    let jwt_header = format!("eyJ{}", "-eyJ".repeat(n / 8));
    let inputs = [
        format!("{}1", fill("token:")),
        fill("token="),
        fill("-----BEGIN "),
        format!("{}_", fill("xoxb-")),
        format!("{jwt_header}.{}.{}", "A".repeat(n / 2), body(20)),
        fill("<private a=\"x "),
        fill("://"),
        fill("Bearer "),
        fill("a="),
        fill("ghp_a"),
        fill("\\nGITHUB_TOKEN="),
        fill("sk-proj-"),
        format!("{}x", fill("sk-ant-api03-")),
        fill("sk-proj-a1B2c3D4-"),
    ];
    for (i, text) in inputs.iter().enumerate() {
        let start = std::time::Instant::now();
        std::hint::black_box(redact_text(text));
        let elapsed = start.elapsed();
        assert!(elapsed < bound, "input {i} took {elapsed:?}");
    }
}
