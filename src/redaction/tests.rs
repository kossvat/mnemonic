//! Text recognition. Every credential-shaped fixture is assembled at run
//! time from harmless pieces, and assertions never print fixture values.

use super::*;

/// Letters and digits in a fixed pattern: shaped like a token, never one.
pub(super) fn body(n: usize) -> String {
    "a1B2c3D4e5F6".chars().cycle().take(n).collect()
}

pub(super) fn hex(n: usize) -> String {
    "0123456789abcdef".chars().cycle().take(n).collect()
}

pub(super) fn b64url(bytes: &[u8]) -> String {
    const ABC: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let n =
            chunk.iter().fold(0u32, |acc, &b| (acc << 8) | u32::from(b)) << (8 * (3 - chunk.len()));
        for k in 0..=chunk.len() {
            out.push(ABC[((n >> (18 - 6 * k)) & 63) as usize] as char);
        }
    }
    out
}

pub(super) fn provider_token() -> String {
    ["sk-", "proj-", &body(40)].concat()
}

/// One fixture: the class it counts as, the text holding it, and the part
/// of that text that must disappear.
pub(super) struct Shape {
    pub class: Class,
    pub text: String,
    pub secret: String,
}

fn shape(class: Class, text: String, secret: &str) -> Shape {
    let secret = secret.to_string();
    Shape {
        class,
        text,
        secret,
    }
}

fn token(class: Class, secret: String) -> Shape {
    shape(class, secret.clone(), &secret)
}

pub(super) fn shapes() -> Vec<Shape> {
    let jwt = [
        b64url(br#"{"alg":"HS256","typ":"JWT"}"#),
        b64url(br#"{"sub":"42"}"#),
        body(24),
    ]
    .join(".");
    let pem = [
        "-----BEGIN ",
        "RSA PRIVATE KEY",
        "-----\n",
        &body(64),
        "\n",
        &body(32),
        "\n-----END RSA ",
        "PRIVATE KEY-----",
    ]
    .concat();
    let pw = body(16);
    let bearer = body(32);
    let v = body(32);
    let fal = ["1a2b3c4d-1111-2222-3333-444455556666", ":", &hex(32)].concat();
    let ru = body(24);
    use Class::*;
    vec![
        token(ProviderToken, ["sk", "-", &body(24)].concat()),
        token(ProviderToken, provider_token()),
        token(ProviderToken, ["sk-", "ant-api03-", &body(40)].concat()),
        token(
            ProviderToken,
            ["sk-", "svcacct-", &body(24), "_", &body(30)].concat(),
        ),
        token(ProviderToken, ["sk-", "admin-", &body(40)].concat()),
        // Tails whose separators fall every few characters.
        token(
            ProviderToken,
            [
                "sk-",
                "ant-api03-",
                &body(15),
                "-",
                &body(15),
                "_",
                &body(15),
                "-",
                &body(15),
                "AA",
            ]
            .concat(),
        ),
        token(
            ProviderToken,
            [
                "sk-",
                "proj-",
                &body(12),
                "_",
                &body(12),
                "-",
                &body(12),
                "_",
                &body(12),
            ]
            .concat(),
        ),
        token(ProviderToken, ["sk-", "or-v1-", &hex(64)].concat()),
        token(
            ProviderToken,
            ["sk-", "lf-", "1a2b3c4d-1111-2222-3333-444455556666"].concat(),
        ),
        token(ProviderToken, ["gh", "p_", &body(36)].concat()),
        token(ProviderToken, ["github", "_pat_", &body(40)].concat()),
        token(ProviderToken, ["gl", "pat-", &body(20)].concat()),
        token(ProviderToken, ["np", "m_", &body(36)].concat()),
        token(
            ProviderToken,
            ["xo", "xb-", "1234567890-", &body(24)].concat(),
        ),
        token(ProviderToken, ["AK", "IA", "ABCDEFGH12345678"].concat()),
        token(ProviderToken, ["AS", "IA", "ABCDEFGH12345678"].concat()),
        token(ProviderToken, ["AI", "za", &body(35)].concat()),
        token(ProviderToken, ["dop", "_v1_", &hex(64)].concat()),
        token(ProviderToken, ["1234567890", ":", "AA", &body(33)].concat()),
        token(Jwt, jwt),
        token(PrivateKey, pem),
        shape(
            UrlPassword,
            format!("postgres://app:{pw}@db.internal:5432/app"),
            &pw,
        ),
        shape(
            BearerToken,
            format!("Authorization: Bearer {bearer}"),
            &bearer,
        ),
        shape(CredentialAssignment, format!("OPENAI_API_KEY={v}"), &v),
        shape(CredentialAssignment, format!("\"api_key\": \"{v}\""), &v),
        shape(
            CredentialAssignment,
            format!("api_key = {}", hex(32)),
            &hex(32),
        ),
        shape(CredentialAssignment, format!("FAL_KEY={fal}"), &fal),
        shape(CredentialAssignment, format!("пароль: {ru}"), &ru),
    ]
}

#[test]
fn redaction_matcher_every_class_in_en_and_ru() {
    for (i, s) in shapes().iter().enumerate() {
        for template in [
            "Deploy note: {} then restart.",
            "Заметка: {}, не публиковать.",
            "ключ {}",
        ] {
            let text = template.replace("{}", &s.text);
            let r = redact_text(&text);
            let expected = template.replace("{}", &s.text.replace(&s.secret, s.class.marker()));
            assert!(
                r.value == expected,
                "shape {i} ({}) in {template:?}",
                s.class.as_str()
            );
            assert!(!r.value.contains(&s.secret), "shape {i} leaked");
            assert!(r.changed);
            assert!(r.counts.get(s.class) == 1, "shape {i}");
            assert!(r.counts.total() == 1, "shape {i}");
        }
    }
}

#[test]
fn redaction_matcher_leaves_ordinary_text_alone() {
    let ordinary = [
        hex(40),
        hex(64),
        "1a2b3c4d-1111-2222-3333-444455556666".to_string(),
        "release v1.13.1 of the cli".to_string(),
        "cargo test --release --lib facts:: -- --test-threads=1".to_string(),
        "let token = parser.next_significant_token();".to_string(),
        "key = \"project:mnemonic:decisions-2026-09-24\"".to_string(),
        "subject_key: person-example-2026-01-02-long-identifier".to_string(),
        "idempotency_key=4f1c2e3a-1111-2222-3333-444455556666".to_string(),
        "use a Bearer token in the header".to_string(),
        "password: hunter2".to_string(),
        "token: correct horse battery staple 2024".to_string(),
        "https://user@host.internal/path:with@colon".to_string(),
        ["ri", "sk-", "assessment-framework-2026-final"].concat(),
        ["de", "sk-", "1234567890abcdefghij1234"].concat(),
        "the sk- prefix alone".to_string(),
        [
            "/Users/me/.claude/projects/-home-me-work-sk-",
            "platform-services-api/a.jsonl",
        ]
        .concat(),
        ["ri", "sk-", "platform-services-api-gateway-2026"].concat(),
        ["-Users-me-work-", "sk-", "ant-colony-sim-2026-final"].concat(),
        ["-code-", "sk-", "proj-management-tool-api-v2-final"].concat(),
        ["-code-", "sk-", "proj-v2a-b3-tool-2026-final-build-x9y8"].concat(),
        ["sk-", "lf-", "not-a-uuid-but-long-enough-text"].concat(),
        ["sk-", "lf-", "notahexs-uuid-here-plus-moreletterss"].concat(),
        "AKIA1234 is too short".to_string(),
        "if a == b && c != d { x::y() }".to_string(),
        "| token | value |".to_string(),
        "Ключ к успеху: 2026-планирование-проекта".to_string(),
        "eyJzzzz.eyJzzzz.abcdefghijklmnopqrst".to_string(),
        "<privateer> and <private/> are not tags".to_string(),
        "an unclosed <private attr=1 with no end".to_string(),
        CREDENTIAL_MARKER.to_string(),
        format!("token = {CREDENTIAL_MARKER}"),
    ];
    for (i, text) in ordinary.iter().enumerate() {
        let r = redact_text(text);
        assert!(!r.changed, "ordinary text {i} was changed");
        assert!(r.counts.is_empty(), "ordinary text {i}");
    }
}

#[test]
fn redaction_matcher_contextual_hex() {
    let bare = hex(32);
    assert!(is_clean(&bare), "a bare hash is not a credential");
    let r = redact_text(&format!("client_secret: \"{bare}\""));
    assert!(!r.value.contains(&bare));
    assert!(r.counts.get(Class::CredentialAssignment) == 1);
}

#[test]
fn redaction_matcher_is_idempotent() {
    for (i, s) in shapes().iter().enumerate() {
        let once = redact_text(&s.text);
        let twice = redact_text(&once.value);
        assert!(
            twice.value == once.value,
            "shape {i} changed on the second pass"
        );
        assert!(!twice.changed && twice.counts.is_empty(), "shape {i}");
    }
}

#[test]
fn redaction_matcher_private_regions() {
    let token = provider_token();
    let cases: [(&str, String, u32); 6] = [
        ("plain", "a <private>my notes</private> b".into(), 1),
        (
            "nested, any case, attributes",
            "x <PRIVATE>1 <private reason=\"x\">2</private> 3</Private > y".into(),
            1,
        ),
        ("unmatched opener", "keep <private>the rest of it".into(), 1),
        ("orphan closer", "a </private> b".into(), 0),
        (
            "credential inside",
            format!("<private>{token}</private>"),
            1,
        ),
        (
            "two regions",
            "<private>1</private> and <private>2</private>".into(),
            2,
        ),
    ];
    let expected = [
        "a [REDACTED:private] b".to_string(),
        "x [REDACTED:private] y".to_string(),
        "keep [REDACTED:private]".to_string(),
        "a [/private] b".to_string(),
        PRIVATE_MARKER.to_string(),
        "[REDACTED:private] and [REDACTED:private]".to_string(),
    ];
    for ((name, text, regions), expected) in cases.iter().zip(expected) {
        let r = redact_text(text);
        assert!(r.value == expected, "{name}");
        assert!(r.changed, "{name}");
        assert!(r.counts.get(Class::PrivateBlock) == *regions, "{name}");
        assert!(r.counts.total() == *regions, "{name}: nothing else counted");
    }
}

#[test]
fn redaction_matcher_private_region_goes_before_any_cut() {
    let text = format!(
        "{}<private>{}</private>tail",
        "A".repeat(100),
        "S".repeat(50)
    );
    let r = redact_text(&text);
    assert!(!r.value.contains('S'));
    assert!(r.value.ends_with("[REDACTED:private]tail"));
}

#[test]
fn redaction_matcher_markers_do_not_exempt_neighbours() {
    let token = provider_token();
    for text in [
        format!("{CREDENTIAL_MARKER}{token}"),
        format!("{CREDENTIAL_MARKER} {token} {CREDENTIAL_MARKER}"),
        format!("[REDACTED:credential {token}"),
        format!("[REDACTED:{token}]"),
    ] {
        let r = redact_text(&text);
        assert!(
            !r.value.contains(&token),
            "a marker-like wrapper exempted a token"
        );
        assert!(r.counts.total() == 1);
    }
}

#[test]
fn redaction_matcher_overlaps_count_once() {
    let jwt = shapes()
        .into_iter()
        .find(|s| s.class == Class::Jwt)
        .unwrap()
        .text;
    let r = redact_text(&format!("Authorization: Bearer {jwt}"));
    assert!(r.counts.get(Class::BearerToken) == 1);
    assert!(r.counts.total() == 1);

    let r = redact_text(&format!("OPENAI_API_KEY=\"{}\"", provider_token()));
    assert!(r.counts.get(Class::CredentialAssignment) == 1);
    assert!(r.counts.total() == 1);
    assert!(r.value == format!("OPENAI_API_KEY=\"{CREDENTIAL_MARKER}\""));
}

#[test]
fn redaction_matcher_overlap_never_leaves_a_tail() {
    use detect::{Span, resolve};
    let span = |start, end, class| Span { start, end, class };
    // The higher-priority candidate is the shorter one.
    let merged = resolve(vec![
        span(5, 20, Class::Jwt),
        span(0, 10, Class::BearerToken),
        span(30, 40, Class::ProviderToken),
    ]);
    assert!(
        merged
            == vec![
                span(0, 20, Class::BearerToken),
                span(30, 40, Class::ProviderToken)
            ]
    );

    // End to end: a JWT signature holding `_` after `Bearer`.
    let jwt = shapes()
        .into_iter()
        .find(|s| s.class == Class::Jwt)
        .unwrap()
        .text;
    let (head, _) = jwt.rsplit_once('.').unwrap();
    let tail = body(12);
    let r = redact_text(&format!("Authorization: Bearer {head}.{}_{tail}", body(10)));
    assert!(!r.value.contains(&tail), "part of the token survived");
    assert!(r.counts.total() == 1);
}

#[test]
fn redaction_matcher_finds_tokens_glued_to_words() {
    let token = provider_token();
    let jwt = shapes()
        .into_iter()
        .find(|s| s.class == Class::Jwt)
        .unwrap()
        .text;
    let bot = ["123456789", ":", "AA", &body(33)].concat();
    for (text, secret) in [
        (format!("tag-{token}"), token.clone()),
        (format!("backup_{token}.json"), token.clone()),
        (format!("x:\\n{token}"), token.clone()),
        (format!("{{\"m\":\"bad:\\n{token}\"}}"), token.clone()),
        (format!("id-{jwt}"), jwt.clone()),
        (
            format!("https://api.telegram.org/bot{bot}/sendMessage"),
            bot.clone(),
        ),
        (format!("data/{bot}/documents"), bot.clone()),
    ] {
        let r = redact_text(&text);
        assert!(!r.value.contains(&secret), "a glued credential survived");
        assert!(r.counts.total() == 1);
    }
}

#[test]
fn redaction_matcher_keeps_unaffected_unicode() {
    let token = provider_token();
    let r = redact_text(&format!("Привет 👋 {token} мир ✓ ключ{token}ё"));
    assert!(r.value == format!("Привет 👋 {CREDENTIAL_MARKER} мир ✓ ключ{CREDENTIAL_MARKER}ё"));
    assert!(r.counts.get(Class::ProviderToken) == 2);
}

#[test]
fn redaction_matcher_identity_checks() {
    assert!(check_identity("project:mnemonic") == Ok(()));
    assert!(check_identity("проект-альфа") == Ok(()));
    let token = provider_token();
    let err = check_identity(&format!("person:{token}")).unwrap_err();
    assert!(err == RedactionError::SensitiveContent);
    assert!(err.to_string() == "SENSITIVE_CONTENT");
    assert!(check_identity("a <private>b</private>") == Err(RedactionError::SensitiveContent));
    assert!(check_identity("a </private> b") == Err(RedactionError::SensitiveContent));
}

#[test]
fn redaction_matcher_dense_adversarial_input() {
    let pieces = [
        "sk-",
        "<private ",
        "</private",
        "-----BEGIN ",
        "://",
        "Bearer ",
        "==",
        "::",
        "eyJ",
        ".",
        "AKIA",
        ":",
        "=",
        "\"",
        "@",
        "пароль:",
        "<private>",
        "a1B2c",
        " ",
        "PRIVATE KEY-----",
        "xoxb-",
        "ghp_",
        "\n",
    ];
    let mut text = String::new();
    let mut k = 0;
    while text.len() < 64 << 10 {
        text.push_str(pieces[k % pieces.len()]);
        text.push_str(&body(k % 7));
        k += 1;
    }
    let once = redact_text(&text);
    let twice = redact_text(&once.value);
    assert!(
        twice.value == once.value && !twice.changed,
        "not idempotent"
    );
}

/// Timing evidence for the design's scaling note. Run with
/// `cargo test --release redaction_matcher_scaling_report -- --ignored --nocapture`.
/// Prints sizes and times only, never fixture text.
#[test]
#[ignore]
fn redaction_matcher_scaling_report() {
    let paragraph = format!(
        "Decision about the deploy, решение про выкладку. OPENAI_API_KEY={} {} ",
        body(32),
        "обычный текст plain text ".repeat(60)
    );
    for size in [1usize << 10, 64 << 10, 1 << 20] {
        let text: String = paragraph.chars().cycle().take(size).collect();
        let mut best = f64::MAX;
        for _ in 0..5 {
            let start = std::time::Instant::now();
            let r = redact_text(&text);
            std::hint::black_box(&r);
            best = best.min(start.elapsed().as_secs_f64());
        }
        let bytes = text.len();
        eprintln!(
            "redaction v{POLICY_VERSION}: {bytes} bytes in {:.3} ms ({:.1} MB/s)",
            best * 1e3,
            bytes as f64 / best / 1e6
        );
    }
}
