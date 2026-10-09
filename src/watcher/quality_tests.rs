//! The rules of noise, tried on what capture really stores.
//!
//! A fixture is a reply. It goes the way a reply goes: a line of a
//! transcript in the format of a watcher, the parser of that watcher with
//! its redaction and its selection, the preparation of the queue, the
//! classifier, and the preparation the daemon gives its result. What comes
//! out is what the store would hold, and that is what the rules judge.
//!
//! Every reply is in the data file. None is written here.
use std::path::Path;

use super::codex::CodexWatcher;
use super::conversation::ConversationWatcher;
use super::ingress::ParsedTurn;
use crate::classifier::Classifier;
use crate::classifier::rules::RuleClassifier;
use crate::config::Config;
use crate::event::{EventKind, EventSource, MemoryEntry, MemoryType};
use crate::quality::{self, Capture, Noise};
use crate::quality_data as data;
use crate::quality_data::fixtures::{
    ATTACHMENTS, CORRECTION_STEPS, HIDDEN, KEPT, LINES, NO_CORRECTION, NO_DECISION, PASSES,
    PRIVATE_TEXT, REDACTED, STRICT_CORRECTIONS, Turn, WRONGLY_HIDDEN, limit_cases, long_kept,
};

type Parser = fn(&Path, &str) -> Option<ParsedTurn>;

const FORMATS: [(&str, Parser); 2] = [
    ("claude", ConversationWatcher::parse_turn),
    ("codex", CodexWatcher::parse_turn),
];

/// One line of a transcript that holds a reply, as the agent of `format`
/// writes it.
fn line(format: &str, role: &str, text: &str) -> String {
    let value = if format == "claude" {
        serde_json::json!({
            "type": role,
            "uuid": "m1",
            "sessionId": "demoapp-session",
            "timestamp": "2026-01-02T03:04:05Z",
            "message": {"content": [{"type": "text", "text": text}]},
        })
    } else {
        let block = if role == "user" {
            "input_text"
        } else {
            "output_text"
        };
        serde_json::json!({
            "type": "response_item",
            "timestamp": "2026-01-02T03:04:05Z",
            "payload": {
                "type": "message",
                "id": "m1",
                "role": role,
                "content": [{"type": block, "text": text}],
            },
        })
    };
    value.to_string()
}

/// What the store would hold for a reply, or nothing when capture lets the
/// reply go.
fn stored(format: &str, parse: Parser, role: &str, text: &str) -> Option<MemoryEntry> {
    let parsed = parse(
        Path::new("/transcripts/rollout-demoapp.jsonl"),
        &line(format, role, text),
    )?;
    let queued = crate::redaction::replay_event(parsed.event, crate::redaction::STRUCTURAL_KEYS)
        .expect("a fixture is refused by no guard")
        .into_event();
    let classifier = RuleClassifier::new(Config::default().classifier);
    classifier
        .classify(&queued)
        .and_then(crate::daemon::prepare_classified)
}

/// A reply in the user's role that begins the way the app begins what it
/// writes there (a slash command's wrapper, a command's output) never
/// reaches the store from a Claude transcript: the parser lets it go before
/// the rules are asked. The Codex format still stores it, and the rules
/// still name it there.
fn written_by_the_app(format: &str, turn: Turn) -> bool {
    format == "claude"
        && turn.role == "user"
        && crate::projects::facts::is_written_by_the_app(turn.text)
}

fn each(turns: impl IntoIterator<Item = Turn>, mut check: impl FnMut(&str, Turn, MemoryEntry)) {
    for turn in turns {
        for (format, parse) in FORMATS {
            if written_by_the_app(format, turn) {
                assert!(
                    stored(format, parse, turn.role, turn.text).is_none(),
                    "an app-written reply is stored ({format}): {}",
                    turn.text
                );
                continue;
            }
            let entry = stored(format, parse, turn.role, turn.text)
                .unwrap_or_else(|| panic!("capture lets a fixture go ({format}): {}", turn.text));
            check(format, turn, entry);
        }
    }
}

fn hidden() -> impl Iterator<Item = (Noise, Turn)> {
    HIDDEN.iter().chain(WRONGLY_HIDDEN).copied()
}

#[test]
fn quality_kept_turns_are_stored_and_are_not_noise() {
    each(KEPT.iter().copied(), |format, turn, entry| {
        assert!(
            quality::capture_kind(&entry).is_some(),
            "not a capture ({format}): {}",
            turn.text
        );
        assert_eq!(
            quality::judge(&entry),
            None,
            "a statement is named as noise ({format}): {}",
            turn.text
        );
    });
}

#[test]
fn quality_long_replies_are_stored_cut_and_are_not_noise() {
    for (role, text) in long_kept() {
        for (format, parse) in FORMATS {
            let entry = stored(format, parse, role, &text)
                .unwrap_or_else(|| panic!("capture lets a long reply go ({format})"));
            assert!(entry.content.chars().count() <= 1000, "({format})");
            assert!(entry.content.len() < text.len(), "({format})");
            assert_eq!(
                quality::capture_kind(&entry),
                Some(Capture::Decision),
                "({format})"
            );
            assert_eq!(quality::judge(&entry), None, "({format}): {}", entry.title);
        }
    }
}

#[test]
fn quality_hidden_turns_are_stored_and_named_by_their_rule() {
    for (rule, turn) in hidden() {
        each([turn], |format, turn, entry| {
            assert_eq!(
                quality::judge(&entry),
                Some(rule),
                "({format}): {}",
                turn.text
            );
        });
    }
}

#[test]
fn quality_known_junk_passes_and_the_limit_is_written_down() {
    each(PASSES.iter().copied(), |format, turn, entry| {
        assert_eq!(
            quality::judge(&entry),
            None,
            "a rule names this now; move it to the hidden turns ({format}): {}",
            turn.text
        );
    });
}

#[test]
fn quality_both_formats_store_the_same_for_a_turn() {
    for turn in KEPT
        .iter()
        .chain(PASSES)
        .copied()
        .chain(hidden().map(|(_, turn)| turn))
    {
        if written_by_the_app("claude", turn) {
            continue;
        }
        let [claude, codex] = FORMATS
            .map(|(format, parse)| stored(format, parse, turn.role, turn.text).expect("stored"));
        assert_eq!(claude.title, codex.title, "{}", turn.text);
        assert_eq!(claude.content, codex.content, "{}", turn.text);
        assert_eq!(claude.memory_type, codex.memory_type, "{}", turn.text);
        assert_eq!(claude.tags, codex.tags, "{}", turn.text);
        assert_eq!(claude.source, EventSource::ConversationWatcher);
        assert_eq!(codex.source, EventSource::CodexWatcher);
    }
}

#[test]
fn quality_a_turn_is_stored_as_a_decision_or_a_correction_with_its_tags() {
    let all = KEPT
        .iter()
        .chain(PASSES)
        .copied()
        .chain(hidden().map(|(_, turn)| turn));
    each(all, |_, turn, entry| match quality::capture_kind(&entry) {
        Some(Capture::Decision) => {
            assert_eq!(entry.memory_type, MemoryType::Decision);
            assert_eq!(entry.tags, ["decision", "conversation"]);
        }
        Some(Capture::Correction) => {
            assert_eq!(entry.memory_type, MemoryType::Feedback);
            assert_eq!(entry.tags, ["feedback", "correction"]);
            assert_eq!(turn.role, "user");
        }
        None => panic!("not a capture: {}", turn.text),
    });
}

#[test]
fn quality_a_memory_is_judged_the_same_after_it_was_stored() {
    let scratch = crate::test_support::temp_dir("mn-quality-");
    let storage = crate::storage::Storage::open_no_hnsw(&scratch.path().join("memories.db"))
        .expect("a store in a scratch folder");
    let all = KEPT
        .iter()
        .chain(PASSES)
        .copied()
        .chain(hidden().map(|(_, turn)| turn));
    let mut named = 0;
    each(all, |format, turn, entry| {
        storage.save(&entry).expect("saved");
        let back = storage
            .get_by_id(&entry.id)
            .expect("read")
            .unwrap_or_else(|| panic!("not in the store ({format}): {}", turn.text));
        assert_eq!(back.content, entry.content, "{}", turn.text);
        assert_eq!(
            quality::capture_kind(&back),
            quality::capture_kind(&entry),
            "({format}): {}",
            turn.text
        );
        assert_eq!(
            quality::judge(&back),
            quality::judge(&entry),
            "({format}): {}",
            turn.text
        );
        named += usize::from(quality::judge(&back).is_some());
    });
    // Two formats each, less the replies the Claude parser lets go as the
    // app's own.
    let app_written = hidden()
        .filter(|(_, turn)| written_by_the_app("claude", *turn))
        .count();
    assert_eq!(app_written, 3);
    assert_eq!(named, 2 * hidden().count() - app_written);
}

#[test]
fn quality_a_hidden_turn_from_any_other_source_is_not_judged() {
    each(hidden().map(|(_, turn)| turn), |_, turn, entry| {
        assert!(quality::judge(&entry).is_some(), "{}", turn.text);
        for source in [
            EventSource::Manual,
            EventSource::Socket,
            EventSource::GitWatcher,
            EventSource::FileWatcher,
        ] {
            let mut other = entry.clone();
            other.source = source;
            assert_eq!(quality::capture_kind(&other), None, "{}", turn.text);
            assert_eq!(quality::judge(&other), None, "{}", turn.text);
        }
        let mut other = entry.clone();
        other.tags = vec!["bugfix".into()];
        assert_eq!(quality::judge(&other), None, "{}", turn.text);
    });
}

#[test]
fn quality_capture_makes_two_kinds_of_event_of_a_conversation() {
    let mut corrections = 0;
    let mut decisions = 0;
    for turn in KEPT {
        for (format, parse) in FORMATS {
            let parsed = parse(
                Path::new("/transcripts/demoapp.jsonl"),
                &line(format, turn.role, turn.text),
            )
            .unwrap_or_else(|| panic!("capture lets a fixture go ({format}): {}", turn.text));
            match parsed.event.kind {
                EventKind::UserCorrection => {
                    assert_eq!(turn.role, "user", "{}", turn.text);
                    corrections += 1;
                }
                EventKind::Custom(_) => decisions += 1,
                other => panic!("({format}) {other:?}: {}", turn.text),
            }
        }
    }
    assert!(corrections >= 10 && decisions >= 10);
}

#[test]
fn quality_the_statement_line_and_the_headline_are_two_lines() {
    let mut differ = 0;
    for (turn, statement, headline, verdict) in LINES {
        each([*turn], |format, turn, entry| {
            let kind = quality::capture_kind(&entry).expect("a capture");
            assert_eq!(entry.title, *headline, "({format}) {}", turn.text);
            assert_eq!(
                quality::statement_line(kind, &entry.content).as_deref(),
                *statement,
                "({format}) {}",
                turn.text
            );
            assert_eq!(quality::judge(&entry), *verdict, "({format}) {}", turn.text);
            if kind == Capture::Correction {
                assert_eq!(entry.content, turn.text, "({format})");
            }
            differ += usize::from(statement.is_some_and(|line| line != *headline));
        });
    }
    // Three of the replies, in two formats each.
    assert_eq!(differ, 6);
}

#[test]
fn quality_what_follows_a_redaction_marker_is_what_the_line_says() {
    let mut marked = 0;
    for (turn, with_marker, verdict) in REDACTED {
        assert!(turn.text.contains(PRIVATE_TEXT), "{}", turn.text);
        each([*turn], |format, turn, entry| {
            assert!(
                !entry.content.contains(PRIVATE_TEXT) && !entry.title.contains(PRIVATE_TEXT),
                "({format}) {}",
                turn.text
            );
            assert_eq!(
                entry.content.contains(crate::redaction::PRIVATE_MARKER),
                *with_marker,
                "({format}) {}",
                entry.content
            );
            assert_eq!(quality::judge(&entry), *verdict, "({format}) {}", turn.text);
            marked += usize::from(*with_marker);
        });
    }
    assert!(marked >= 10);
}

#[test]
fn quality_what_the_tests_of_detection_hold_as_neither_is_let_go_by_capture() {
    for text in NO_CORRECTION {
        assert!(!ConversationWatcher::is_correction(text), "{text}");
        assert!(!quality::is_correction(text), "{text}");
        for (format, parse) in FORMATS {
            assert!(
                stored(format, parse, "user", text).is_none(),
                "({format}) {text}"
            );
        }
    }
    for text in NO_DECISION {
        assert!(!ConversationWatcher::is_decision(text), "{text}");
        assert!(!quality::holds_decision(text), "{text}");
        for (format, parse) in FORMATS {
            assert!(
                stored(format, parse, "assistant", text).is_none(),
                "({format}) {text}"
            );
        }
    }
}

#[test]
fn quality_the_strict_predicate_and_capture_agree_where_markers_are_words() {
    for (step, text, expected) in CORRECTION_STEPS {
        assert_eq!(quality::is_correction(text), *expected, "{step}: {text}");
        assert_eq!(
            ConversationWatcher::is_correction(text),
            *expected,
            "capture, {step}: {text}"
        );
    }
    for (text, expected) in limit_cases() {
        assert_eq!(quality::is_correction(&text), expected, "{}", text.len());
        assert_eq!(
            ConversationWatcher::is_correction(&text),
            expected,
            "capture, {}",
            text.len()
        );
    }
    // What the strict predicate takes among the kept replies, capture
    // takes too; the replies it alone takes are listed apart.
    let mut both = 0;
    for turn in KEPT.iter().filter(|turn| turn.role == "user") {
        if quality::is_correction(turn.text) {
            assert!(
                ConversationWatcher::is_correction(turn.text),
                "{}",
                turn.text
            );
            both += 1;
        }
    }
    assert!(both >= 40, "{both}");
}

#[test]
fn quality_a_marker_inside_a_longer_word_is_taken_by_capture_alone() {
    let mut corrections = 0;
    let mut decisions = 0;
    for (rule, turn) in HIDDEN {
        match rule {
            Noise::NotACorrection => {
                corrections += 1;
                assert!(
                    ConversationWatcher::is_correction(turn.text),
                    "{}",
                    turn.text
                );
                assert!(!quality::is_correction(turn.text), "{}", turn.text);
                assert!(!quality::holds_decision(turn.text), "{}", turn.text);
            }
            Noise::NotADecision => {
                decisions += 1;
                assert!(ConversationWatcher::is_decision(turn.text), "{}", turn.text);
                assert!(!quality::holds_decision(turn.text), "{}", turn.text);
            }
            _ => {}
        }
    }
    assert!(corrections >= 5 && decisions >= 5);
}

#[test]
fn quality_the_strict_predicate_takes_words_capture_does_not_reach() {
    for text in STRICT_CORRECTIONS {
        assert!(!ConversationWatcher::is_correction(text), "{text}");
        assert!(quality::is_correction(text), "{text}");
        assert_eq!(
            quality::judge_content(Capture::Correction, text),
            None,
            "{text}"
        );
    }
}

#[test]
fn quality_attachments_are_stripped_as_capture_strips_them() {
    let mut changed = 0;
    for text in ATTACHMENTS {
        let stripped = quality::strip_attachment_refs(text);
        assert_eq!(
            stripped,
            super::conversation::strip_attachment_refs_for_tests(text),
            "{text}"
        );
        changed += usize::from(stripped != *text);
    }
    assert!(changed >= 4);
}

#[test]
fn quality_the_lists_hold_what_capture_looks_for_and_these_words_more() {
    let (strong, negatable, negated, weak, decision) = super::conversation::lists_for_tests();
    for (strict, capture, added) in [
        (data::CORRECTION_STRONG, strong, data::ADDED_STRONG),
        (data::CORRECTION_NEGATABLE, negatable, data::ADDED_NEGATABLE),
        (data::CORRECTION_NEGATED, negated, &[][..]),
        (data::CORRECTION_WEAK, weak, &[][..]),
        (data::DECISION, decision, data::ADDED_DECISION),
    ] {
        for word in capture {
            assert!(strict.contains(word), "{word}");
        }
        let more: Vec<_> = strict
            .iter()
            .filter(|word| !capture.contains(word))
            .collect();
        assert_eq!(more, added.iter().collect::<Vec<_>>());
    }
}

#[test]
fn quality_every_word_added_to_a_list_stands_in_a_kept_reply() {
    // A word taken out of both lists at once would pass the comparison
    // of the lists; its reply would not.
    for word in data::ADDED_STRONG.iter().chain(data::ADDED_NEGATABLE) {
        let held = KEPT
            .iter()
            .filter(|turn| turn.role == "user")
            .map(|turn| turn.text)
            .chain(STRICT_CORRECTIONS.iter().copied())
            .any(|text| quality::holds(&text.to_lowercase(), word));
        assert!(held, "no kept reply holds {word}");
    }
    for word in data::ADDED_DECISION {
        let held = KEPT
            .iter()
            .any(|turn| quality::holds(&turn.text.to_lowercase(), word));
        assert!(held, "no kept reply holds {word}");
    }
}

#[test]
fn quality_every_word_added_to_a_list_turns_a_verdict() {
    // A reply made of the word alone. Without the word in its list the
    // strict predicate lets the reply go, or the phrase is no decision.
    for word in data::ADDED_STRONG.iter().chain(data::ADDED_NEGATABLE) {
        assert!(quality::is_correction(word), "{word}");
        assert_eq!(
            quality::judge_content(Capture::Correction, word),
            None,
            "{word}"
        );
    }
    // A negation refuses the longer forms of what it negates.
    for word in data::ADDED_NEGATABLE {
        for negation in data::CORRECTION_NEGATED {
            if let Some(stem) = data::CORRECTION_NEGATABLE
                .iter()
                .find(|stem| negation.ends_with(*stem) && word.starts_with(*stem))
            {
                let longer = format!("{negation}{}", &word[stem.len()..]);
                assert!(!quality::is_correction(&longer), "{longer}");
            }
        }
    }
    for word in data::ADDED_DECISION {
        assert!(quality::holds_decision(word), "{word}");
    }
}

/// The string literals of a Rust source, with the escapes of a newline, a
/// quote and a backslash undone. Raw strings and the rest of a line after
/// a comment marker are skipped.
fn literals(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let line = line.split("//").next().unwrap_or("");
        let mut rest = line;
        while let Some(start) = rest.find('"') {
            if rest[..start].ends_with('r') || rest[..start].ends_with('#') {
                rest = &rest[start + 1..];
                continue;
            }
            let body = &rest[start + 1..];
            let mut text = String::new();
            let mut chars = body.char_indices();
            let mut end = None;
            while let Some((at, c)) = chars.next() {
                match c {
                    '\\' => match chars.next() {
                        Some((_, 'n')) => text.push('\n'),
                        Some((_, 't')) => text.push('\t'),
                        Some((_, other)) => text.push(other),
                        None => break,
                    },
                    '"' => {
                        end = Some(at);
                        break;
                    }
                    other => text.push(other),
                }
            }
            let Some(end) = end else { break };
            out.push(text);
            rest = &body[end + 1..];
        }
    }
    out
}

#[test]
fn quality_every_reply_the_tests_of_detection_hold_is_a_fixture() {
    let sources = [
        ("watcher/conversation.rs", include_str!("conversation.rs")),
        ("watcher/ingress_tests.rs", include_str!("ingress_tests.rs")),
        ("watcher/history_tests.rs", include_str!("history_tests.rs")),
        ("watcher/scope_tests.rs", include_str!("scope_tests.rs")),
        ("watcher/origin_tests.rs", include_str!("origin_tests.rs")),
    ];
    // What capture takes is a statement to those tests, so it belongs to
    // the fixtures that are kept, in whole or as the part the test quotes.
    let long = long_kept();
    let fixtures: Vec<&str> = KEPT
        .iter()
        .map(|turn| turn.text)
        .chain(LINES.iter().map(|(turn, ..)| turn.text))
        .chain(REDACTED.iter().map(|(turn, ..)| turn.text))
        .chain(STRICT_CORRECTIONS.iter().copied())
        .chain(long.iter().map(|(_, text)| text.as_str()))
        .collect();
    let mut checked = 0;
    for (name, source) in sources {
        let tests = source.find("#[cfg(test)]").unwrap_or(0);
        for literal in literals(&source[tests..]) {
            // A reply, as the tests write one: text that capture takes, of
            // some length, and not a line of a transcript.
            let taken = ConversationWatcher::is_correction(&literal)
                || ConversationWatcher::is_decision(&literal);
            if !taken || literal.chars().count() < 12 || literal.starts_with('{') {
                continue;
            }
            // A reply the test builds by a format: the part before the
            // first placeholder is what the built reply holds.
            let quoted = literal.split('{').next().unwrap_or("");
            checked += 1;
            assert!(
                fixtures.iter().any(|text| text.contains(quoted)),
                "{name}: not among the fixtures: {literal:?}"
            );
        }
    }
    assert!(checked >= 20, "{checked}");
}
