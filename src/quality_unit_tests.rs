//! The rules of noise on texts, without capture.
//!
//! No text here is in a script other than Latin. Those are in the data
//! file, and a test below holds this file to it.

use super::*;
use crate::quality_data::fixtures::{
    CORRECTION_STEPS, NO_CORRECTION, NO_DECISION, STORED_EARLIER, is_cyrillic, limit_cases,
    window_cases,
};
use crate::redaction::{CREDENTIAL_MARKER, PRIVATE_MARKER};

fn memory(
    source: EventSource,
    memory_type: MemoryType,
    tags: &[&str],
    content: &str,
) -> MemoryEntry {
    MemoryEntry {
        id: "m1".into(),
        timestamp: chrono::DateTime::from_timestamp(1_767_322_800, 0).expect("a time"),
        title: "a title".into(),
        content: content.into(),
        memory_type,
        tags: tags.iter().map(|tag| tag.to_string()).collect(),
        source,
        importance: 0.7,
        metadata: serde_json::json!({}),
    }
}

fn shapes(cases: &[(Capture, &str, Option<Noise>)]) {
    for (said_as, line, expected) in cases {
        assert_eq!(shape(*said_as, line), *expected, "{line}");
    }
}

#[test]
fn quality_the_names_of_the_rules_are_these_nine() {
    assert_eq!(
        Noise::ALL.map(Noise::as_str),
        [
            "table-row",
            "markup",
            "image-link",
            "link",
            "data",
            "list-step",
            "question",
            "not-a-decision",
            "not-a-correction",
        ]
    );
}

/// Every file of the rules and of their tests under src/, by name, with
/// the data file apart.
fn files_of_the_rules() -> (Vec<(String, String)>, String) {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut others = Vec::new();
    let mut data = None;
    let mut pending = vec![src];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).expect("src is readable") {
            let path = entry.expect("an entry").path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            if !name.starts_with("quality") || !name.ends_with(".rs") {
                continue;
            }
            let text = std::fs::read_to_string(&path).expect("read");
            if name == "quality_data.rs" {
                data = Some(text);
            } else {
                others.push((path.to_string_lossy().into_owned(), text));
            }
        }
    }
    (others, data.expect("the data file"))
}

#[test]
fn quality_texts_of_other_scripts_are_in_the_data_file_only() {
    let (others, data) = files_of_the_rules();
    assert!(others.len() >= 3, "{others:?}");
    for (name, source) in &others {
        for (number, line) in source.lines().enumerate() {
            assert!(!is_cyrillic(line), "{name}:{}: {line}", number + 1);
            // Nor written as an escape: the blocks of Cyrillic, by the
            // first digits of their code points.
            for block in ["4", "04", "5", "05", "2de", "a6", "1c8"] {
                let escape = format!("\\u{{{block}");
                assert!(
                    !line.to_lowercase().contains(&escape),
                    "{name}:{}: {line}",
                    number + 1
                );
            }
        }
    }
    assert!(is_cyrillic(&data));
}

/// A rough guard: the names by which the rules could reach a clock, the
/// store, a file or the network, and the forms of import that could hide
/// one. A helper of the crate that reaches them by another name is not
/// caught; the design says so.
#[test]
fn quality_the_rules_name_no_clock_no_store_and_no_file() {
    let rules = include_str!("quality.rs");
    for name in [
        "crate::storage",
        "crate::config",
        "crate::profile",
        "crate::daemon",
        "rusqlite",
        "chrono",
        "libc",
        "std::time",
        "std::fs",
        "std::env",
        "std::net",
        "std::process",
        "std::io",
        "tokio",
        "SystemTime",
        "Instant",
    ] {
        assert!(!rules.contains(name), "{name}");
    }
    let uses: Vec<&str> = rules
        .lines()
        .filter(|line| line.starts_with("use "))
        .collect();
    assert_eq!(
        uses,
        [
            "use crate::event::{EventSource, MemoryEntry, MemoryType};",
            "use crate::quality_data as data;",
        ]
    );
}

#[test]
fn quality_a_word_ends_where_nothing_of_a_word_touches_it() {
    for (haystack, needle, expected) in [
        ("stop", "stop", true),
        ("please stop now", "stop", true),
        ("(stop)", "stop", true),
        ("\"stop\"", "stop", true),
        ("stop.", "stop", true),
        ("stop, now", "stop", true),
        ("stop - words", "stop", true),
        ("stop -words", "stop", true),
        ("stop?", "stop", true),
        // A longer word.
        ("stopped", "stop", false),
        ("unstop", "stop", false),
        // A digit is part of a word.
        ("stop2", "stop", false),
        ("3decided", "decided", false),
        // A compound, a path, the name of a file.
        ("stop-words", "stop", false),
        ("non-stop", "stop", false),
        ("stop_words", "stop", false),
        ("stop.rs", "stop", false),
        ("notes/stop", "stop", false),
        ("stop/notes", "stop", false),
        ("/stop", "stop", false),
        ("stop/", "stop", false),
        // A flag, a hidden file, a name: a joiner right before a word.
        ("-stop", "stop", false),
        ("--stop", "stop", false),
        (".decided", "decided", false),
        ("_stop", "stop", false),
        ("stop-", "stop", true),
        // What is written into a word without showing.
        ("un\u{ad}decided", "decided", false),
        ("decided\u{200d}x", "decided", false),
        ("decided\u{200c}", "decided", false),
        ("decided\u{301}", "decided", false),
        ("decided\u{1ab0}", "decided", false),
        ("decided\u{1dc0}", "decided", false),
        ("decided\u{20d0}", "decided", false),
        ("decided\u{fe20}", "decided", false),
        // A needle that ends by itself needs no boundary on its right.
        ("stack: rust", "stack:", true),
        ("stack:rust", "stack:", true),
        ("callstack: rust", "stack:", false),
        ("call-stack: rust", "stack:", false),
        ("", "stop", false),
        ("stop", "", false),
    ] {
        assert_eq!(holds(haystack, needle), expected, "{haystack}");
    }
    // A particle glued by a hyphen is not a longer word.
    for particle in data::PARTICLES {
        assert!(holds(&format!("stop-{particle} now"), "stop"), "{particle}");
        assert!(holds(&format!("stop-{particle}"), "stop"), "{particle}");
        assert!(!holds(&format!("stop-{particle}x"), "stop"), "{particle}");
        assert!(!holds(&format!("stop-x{particle}"), "stop"), "{particle}");
        // Behind an underscore or a dot it is part of a name.
        assert!(!holds(&format!("stop_{particle}"), "stop"), "{particle}");
        assert!(!holds(&format!("stop.{particle}"), "stop"), "{particle}");
    }
    // Every place is looked at, not the first alone.
    assert_eq!(
        occurrences("stopped, stop and stop", "stop").collect::<Vec<_>>(),
        [9, 18]
    );
}

#[test]
fn quality_a_weak_marker_counts_as_a_word_within_the_window() {
    for (text, marker, expected) in window_cases() {
        assert_eq!(
            holds_within(&plain(&text), marker, data::CORRECTION_HEAD_CHARS),
            expected,
            "{text}"
        );
        assert_eq!(is_correction(&text), expected, "{text}");
    }
}

#[test]
fn quality_the_length_of_a_correction_is_counted_in_bytes() {
    for (text, expected) in limit_cases() {
        assert_eq!(is_correction(&text), expected, "{} bytes", text.len());
    }
}

#[test]
fn quality_every_step_of_the_predicate_turns_on_a_reply() {
    for (step, text, expected) in CORRECTION_STEPS {
        assert_eq!(is_correction(text), *expected, "{step}: {text}");
    }
}

#[test]
fn quality_an_apostrophe_of_any_shape_is_an_apostrophe() {
    let apostrophes = ['\u{2019}', '\u{2bc}', '\u{2018}'];
    assert_eq!(data::APOSTROPHES, apostrophes);
    for apostrophe in apostrophes {
        let strong = format!("that{apostrophe}s wrong, use the other one");
        assert!(is_correction(&strong), "{strong}");
        let weak = format!("don{apostrophe}t rename the column");
        assert!(is_correction(&weak), "{weak}");
        let reminder = format!("please don{apostrophe}t forget the tag");
        assert!(!is_correction(&reminder), "{reminder}");
    }
}

#[test]
fn quality_a_negation_refuses_wherever_its_letters_stand() {
    // Capture refuses by the letters. A strict list that refused by the
    // word would accept what capture refuses.
    assert!(!is_correction("we cannot forget the tag"));
    assert!(is_correction("we can forget the tag"));
}

#[test]
fn quality_a_strong_marker_inside_an_everyday_phrase_does_not_count() {
    let marker = data::CORRECTION_STRONG
        .iter()
        .find(|marker| marker.is_ascii())
        .expect("a marker in Latin letters");
    for idiom in data::CORRECTION_IDIOMS {
        assert!(!holds_strong(idiom), "{idiom}");
        assert!(!is_correction(idiom), "{idiom}");
        // A marker next to the phrase counts as ever.
        assert!(holds_strong(&format!("{idiom} x; {marker}")), "{idiom}");
        // The phrase is a word of its own: with letters glued to its end
        // it is another word, and the marker inside it counts.
        if !idiom.ends_with(',') {
            assert!(holds_strong(&format!("{idiom}xy")), "{idiom}");
        }
    }
}

#[test]
fn quality_attachments_are_not_read() {
    for (text, expected) in [
        ("@\"notes/stop.txt\" sort the lines", " sort the lines"),
        ("compare with @\"notes/stop", "compare with "),
        ("@\"a.txt\" and @\"b.txt\" stop here", " and  stop here"),
        ("a reply without references", "a reply without references"),
    ] {
        assert_eq!(strip_attachment_refs(text), expected);
    }
    // In the name of a file a marker may stand as a word.
    assert!(!is_correction(
        "@\"old notes stop here.txt\" sort the lines"
    ));
    assert!(!holds_decision("@\"what we decided.md\" sort the lines"));
    assert!(is_correction("old notes stop here.txt, sort the lines"));
}

#[test]
fn quality_a_row_of_a_table_has_bars() {
    shapes(&[
        (
            Capture::Decision,
            "| decided | yes |",
            Some(Noise::TableRow),
        ),
        (Capture::Decision, "|decided", Some(Noise::TableRow)),
        (
            Capture::Decision,
            "decided | yes | no",
            Some(Noise::TableRow),
        ),
        (
            Capture::Decision,
            "\u{2502} decided \u{2502}",
            Some(Noise::TableRow),
        ),
        (Capture::Decision, "\u{2503} decided", Some(Noise::TableRow)),
        // One bar inside a sentence is no table.
        (Capture::Decision, "we decided: a | b", None),
        (Capture::Correction, "| stop | now |", Some(Noise::TableRow)),
        // Two bars that are an operator, and bars inside code.
        (Capture::Decision, "decided: use a || b", None),
        (Capture::Decision, "decided: use `a | b | c`", None),
        (
            Capture::Decision,
            "decided: `a` | b | `c`",
            Some(Noise::TableRow),
        ),
        (Capture::Decision, "decided: a || b || c", None),
        (Capture::Decision, "decided: a | b || c", None),
        (
            Capture::Decision,
            "decided: a | b | c || d",
            Some(Noise::TableRow),
        ),
        (Capture::Decision, "|| we decided", Some(Noise::TableRow)),
        // A span of code opens and closes with a run of the same length.
        (Capture::Decision, "decided: use ``a | b | c`` here", None),
        (Capture::Decision, "decided: use ``a ` | b | c`` here", None),
        (
            Capture::Decision,
            "decided: `a | b` | c | d",
            Some(Noise::TableRow),
        ),
    ]);
}

#[test]
fn quality_markup_opens_with_a_tag() {
    shapes(&[
        (Capture::Decision, "<b>decided</b>", Some(Noise::Markup)),
        (Capture::Decision, "</b> we decided", Some(Noise::Markup)),
        (Capture::Decision, "<!-- decided -->", Some(Noise::Markup)),
        (Capture::Decision, "<?xml decided ?>", Some(Noise::Markup)),
        (Capture::Decision, "<br/> we decided", Some(Noise::Markup)),
        (Capture::Decision, "<b we decided", Some(Noise::Markup)),
        (Capture::Decision, "<a> we decided <b>", Some(Noise::Markup)),
        (
            Capture::Decision,
            "<a> we decided </b>",
            Some(Noise::Markup),
        ),
        // One tag that opens, and nothing of a decision behind it.
        (
            Capture::Decision,
            "<decided> as the run says",
            Some(Noise::Markup),
        ),
        // One tag that opens, and a statement behind it.
        (Capture::Decision, "<Header> we decided to keep", None),
        (Capture::Correction, "<Header> stop, it is wrong", None),
        // A comparison is not a tag.
        (Capture::Decision, "<Header> we decided: a < b", None),
        (Capture::Decision, "<Header> we decided: a <3", None),
        // A bracket inside quotes does not close the tag.
        (Capture::Decision, "<a href=\"> 1.\"> we decided", None),
        (Capture::Decision, "<a href='> 1.'> we decided", None),
        (Capture::Decision, "<a href=\"x\"> we decided", None),
        (
            Capture::Decision,
            "<a href=\"x> we decided",
            Some(Noise::Markup),
        ),
        // No tag at all.
        (Capture::Decision, "< 3 items, we decided", None),
        (Capture::Decision, "<3 we decided", None),
    ]);
}

#[test]
fn quality_a_link_opens_with_brackets_that_a_target_follows() {
    shapes(&[
        (Capture::Decision, "[decided](a.md)", Some(Noise::Link)),
        (Capture::Decision, "[decided][1]", Some(Noise::Link)),
        (Capture::Decision, "[[decided]]", Some(Noise::Link)),
        (
            Capture::Decision,
            "[a [decided] b](c.md)",
            Some(Noise::Link),
        ),
        (Capture::Decision, "[a](b(c)decided)", Some(Noise::Link)),
        (
            Capture::Decision,
            "[decided](a.md) is the page",
            Some(Noise::Link),
        ),
        (
            Capture::Decision,
            "![decided](a.png)",
            Some(Noise::ImageLink),
        ),
        (Capture::Decision, "![decided][1]", Some(Noise::ImageLink)),
        (
            Capture::Decision,
            "![a](b.png) as decided?",
            Some(Noise::Question),
        ),
        // A statement behind a link is a statement.
        (Capture::Decision, "[a](b.md) we decided to keep it", None),
        (
            Capture::Decision,
            "[[a]] we decided to keep it, see [[b]]",
            None,
        ),
        (Capture::Decision, "[a](b(c)d) we decided to keep it", None),
        (Capture::Decision, "![a](b.png) we decided to keep it", None),
        (Capture::Correction, "[a](b.md) stop, it is wrong", None),
        // A label in brackets and the box of a task are taken off when a
        // statement stands behind them.
        (Capture::Decision, "[x] we decided to keep it", None),
        (Capture::Decision, "[ ] we decided to keep it", None),
        (Capture::Decision, "[decided] {a: 1}", None),
        (
            Capture::Decision,
            "[decided] 1. going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "[backend] decided | yes | no",
            Some(Noise::TableRow),
        ),
        (
            Capture::Decision,
            "[backend] 1. we are going with it",
            Some(Noise::ListStep),
        ),
        // Brackets that are not closed, or not opened, are text.
        (Capture::Decision, "[decided", None),
        (Capture::Decision, "]decided[", None),
        (Capture::Decision, "[a](b we decided", None),
        (Capture::Decision, "[[a] we decided", None),
        (Capture::Decision, "!decided", None),
    ]);
}

#[test]
fn quality_data_opens_as_data_does() {
    shapes(&[
        (Capture::Decision, "{\"decided\": true}", Some(Noise::Data)),
        (Capture::Decision, "{decided}", Some(Noise::Data)),
        (
            Capture::Decision,
            "[{\"decided\": true}]",
            Some(Noise::Data),
        ),
        (
            Capture::Decision,
            "[ \"decided\", \"open\"]",
            Some(Noise::Data),
        ),
        (Capture::Decision, "['decided', 'open']", Some(Noise::Data)),
        (Capture::Decision, "\"decided\": true,", Some(Noise::Data)),
        (Capture::Decision, "\"decided\" : true", Some(Noise::Data)),
        (Capture::Decision, "'decided': true", Some(Noise::Data)),
        (Capture::Decision, "\"\": \"decided\"", Some(Noise::Data)),
        (
            Capture::Decision,
            "\"a\\\"b\": \"decided\"",
            Some(Noise::Data),
        ),
        (Capture::Decision, "\"decided\",", Some(Noise::Data)),
        (Capture::Decision, "\"decided\"", Some(Noise::Data)),
        (
            Capture::Decision,
            "\"status\": \"decided\",",
            Some(Noise::Data),
        ),
        (
            Capture::Decision,
            "\"status\": \"decided\"",
            Some(Noise::Data),
        ),
        (
            Capture::Correction,
            "{\"event\": \"stop\"}",
            Some(Noise::Data),
        ),
        (
            Capture::Correction,
            "\"event\": \"stop\",",
            Some(Noise::Data),
        ),
        (
            Capture::Decision,
            "\"decided\", \"id\": 1",
            Some(Noise::Data),
        ),
        (Capture::Decision, "\"decided\", 2", Some(Noise::Data)),
        (Capture::Decision, "\"decided\", true", Some(Noise::Data)),
        (
            Capture::Decision,
            "\"decided\", \"x\": \"y z\"",
            Some(Noise::Data),
        ),
        (Capture::Decision, "\"decided\", 2 of us", None),
        (Capture::Decision, "\"decided\", \"x", None),
        (
            Capture::Decision,
            "\"SQLite\", 2 replicas are enough; we decided to keep that setup.",
            None,
        ),
        (Capture::Decision, "\"decided\", [1]", Some(Noise::Data)),
        (Capture::Decision, "\"decided\"}", Some(Noise::Data)),
        (Capture::Decision, "\"decided\" ]", Some(Noise::Data)),
        (
            Capture::Decision,
            "\"status\": \"decided\", \"id\": 1",
            Some(Noise::Data),
        ),
        // A quoted key, and a statement behind its colon.
        (
            Capture::Correction,
            "\"updated_at\": stop, it is wrong",
            None,
        ),
        (Capture::Decision, "\"cache\": we decided to keep it", None),
        // A text in quotes that a sentence follows.
        (Capture::Decision, "\"decided\" is what the page says", None),
        (Capture::Decision, "\"decided\", he said", None),
        (Capture::Decision, "\"we decided", None),
        // The quotes have to be of one kind.
        (Capture::Decision, "\"decided': true", None),
    ]);
}

#[test]
fn quality_a_step_of_a_list_opens_with_its_number() {
    shapes(&[
        (
            Capture::Decision,
            "1. we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "999. we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "1) we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "1000) we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "(1) we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "a) we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "1.2. we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "1.2.3) we are going with it",
            Some(Noise::ListStep),
        ),
        (Capture::Decision, "2026.1) we are going with it", None),
        (Capture::Decision, "1.2026) we are going with it", None),
        (
            Capture::Decision,
            "**1.** we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "__2)__ we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "- 3. we are going with it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Correction,
            "1. the import has to stop",
            Some(Noise::ListStep),
        ),
        // A year, a version, an abbreviation, a number in a sentence.
        (Capture::Decision, "2026. We decided to keep it", None),
        (Capture::Decision, "1.2026. we decided to keep it", None),
        (Capture::Decision, "1.5 we decided to keep it", None),
        (Capture::Decision, "1.5.2 we decided to keep it", None),
        (Capture::Decision, "e.g. we decided to keep it", None),
        (Capture::Decision, "3 of us decided to keep it", None),
        (Capture::Decision, "1.we decided to keep it", None),
        (Capture::Decision, "(a) we decided to keep it", None),
        (Capture::Decision, "() we decided to keep it", None),
        // A numbered statement: a label of a decision, or a marker of a
        // correction, whichever kind the line is judged as.
        (Capture::Decision, "1. Decision: keep the old index", None),
        (Capture::Decision, "2) stack: rust and sqlite", None),
        (
            Capture::Decision,
            "1. **Decision:** keep the old index",
            None,
        ),
        (
            Capture::Decision,
            "1. stop caching; we decided to keep it",
            None,
        ),
        (Capture::Correction, "1. stop the import", None),
        (Capture::Correction, "1. Stop, it is wrong", None),
        (
            Capture::Correction,
            "1. Decision: stop caching errors",
            None,
        ),
        (Capture::Correction, "2. don't rename the column", None),
        (Capture::Correction, "2. forget the second option", None),
        // A label needs its colon, and a marker has to be a word.
        (
            Capture::Decision,
            "1. decided to keep it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Correction,
            "1. stopped, do not go on",
            Some(Noise::ListStep),
        ),
        (
            Capture::Decision,
            "1. as said, decision: keep it",
            Some(Noise::ListStep),
        ),
        (
            Capture::Correction,
            "1. the column, don't rename it",
            Some(Noise::ListStep),
        ),
    ]);
}

#[test]
fn quality_a_question_is_the_clause_that_holds_the_phrase() {
    shapes(&[
        (Capture::Decision, "decided?", Some(Noise::Question)),
        (
            Capture::Decision,
            "are we going with it?",
            Some(Noise::Question),
        ),
        (
            Capture::Decision,
            "are we going with it? yes or no",
            Some(Noise::Question),
        ),
        (Capture::Decision, "**decided?**", Some(Noise::Question)),
        (Capture::Decision, "(decided?)", Some(Noise::Question)),
        (
            Capture::Decision,
            "\"decided?\" he asked",
            Some(Noise::Question),
        ),
        (
            Capture::Decision,
            "decided? or are we switching to it?",
            Some(Noise::Question),
        ),
        // A statement, and then a question.
        (Capture::Decision, "we decided. ok?", None),
        (Capture::Decision, "we decided! ok?", None),
        (Capture::Decision, "we decided... or not?", None),
        (Capture::Decision, "we decided, ok?", None),
        (Capture::Decision, "we decided; sure?", None),
        (Capture::Decision, "`decided.` can you do it?", None),
        // A question, and then a statement.
        (
            Capture::Decision,
            "decided? yes, we are going with it.",
            None,
        ),
        // A mark inside a name ends no clause.
        (
            Capture::Decision,
            "we decided on demo.example?",
            Some(Noise::Question),
        ),
        (Capture::Decision, "we decided on v1.5 of it. ok?", None),
        (
            Capture::Decision,
            "we decided on a,b?",
            Some(Noise::Question),
        ),
        // A correction is not judged by what it asks.
        (Capture::Correction, "stop, why is it red?", None),
        (Capture::Correction, "stop, are we going with it?", None),
        (Capture::Decision, "is it red?", None),
    ]);
}

#[test]
fn quality_what_stands_in_front_of_a_line_is_taken_off() {
    let marks = ['#', '-', '*', '_', '>', '+', '\u{2022}'];
    assert_eq!(data::LINE_MARKS, marks);
    // A shape that is known by the character a line opens with.
    for mark in marks {
        for front in [
            format!("{mark} "),
            format!("{mark}{mark}"),
            format!(" {mark}\t"),
        ] {
            let data = format!("{front}{{\"decided\": true}}");
            assert_eq!(shape(Capture::Decision, &data), Some(Noise::Data), "{data}");
            let step = format!("{front}1. we are going with it");
            assert_eq!(
                shape(Capture::Decision, &step),
                Some(Noise::ListStep),
                "{step}"
            );
        }
    }
    shapes(&[
        (
            Capture::Decision,
            "- | decided | yes |",
            Some(Noise::TableRow),
        ),
        (Capture::Decision, "## [decided](a.md)", Some(Noise::Link)),
        (Capture::Decision, "+ <b>decided</b>", Some(Noise::Markup)),
        (Capture::Decision, "  \t- - decided?", Some(Noise::Question)),
        // A space of any kind.
        (
            Capture::Decision,
            "-\u{a0}{\"decided\": true}",
            Some(Noise::Data),
        ),
        (Capture::Decision, "", None),
        (Capture::Decision, "- ", None),
    ]);
    // References to attachments are taken out of the whole content first.
    for (content, expected) in [
        ("@\"a.md\" {\"decided\": true}", Some(Noise::Data)),
        ("@\"a.md\" 1. we are going with it", Some(Noise::ListStep)),
        ("@\"a.md\" @\"b.md\" - [decided](c.md)", Some(Noise::Link)),
        ("@\"a.md 1. we are going with it", Some(Noise::NotADecision)),
        ("@\"a.md\" we decided to keep it", None),
        // One that is not closed takes the lines behind it too.
        ("we decided to keep it\n@\"a.md\n| decided | yes |", None),
        (
            "| decided | yes |\n@\"a.md\nwe decided to keep it",
            Some(Noise::TableRow),
        ),
    ] {
        assert_eq!(
            judge_content(Capture::Decision, content),
            expected,
            "{content}"
        );
    }
}

#[test]
fn quality_a_clause_ends_before_the_marks_that_close_it() {
    let marks = [
        '*', '_', '`', ')', ']', '"', '\'', '\u{bb}', '\u{201d}', '\u{2019}',
    ];
    assert_eq!(data::CLOSING_MARKS, marks);
    for mark in marks {
        let asked = format!("is it decided?{mark} and then more");
        assert_eq!(
            shape(Capture::Decision, &asked),
            Some(Noise::Question),
            "{asked}"
        );
        let said = format!("it is decided.{mark} or is it?");
        assert_eq!(shape(Capture::Decision, &said), None, "{said}");
        let said = format!("it is decided,{mark} or is it?");
        assert_eq!(shape(Capture::Decision, &said), None, "{said}");
    }
    assert_eq!(data::CLAUSE_ENDS, ['.', '!', '?', ',', ';']);
    for (line, expected) in [
        ("", vec![]),
        ("and more", vec![("and more", None)]),
        ("decided?", vec![("decided", Some('?'))]),
        ("decided!", vec![("decided", Some('!'))]),
        (
            "we decided. ok?",
            vec![("we decided", Some('.')), (" ok", Some('?'))],
        ),
        (
            "we decided, ok?",
            vec![("we decided", Some(',')), (" ok", Some('?'))],
        ),
        (
            "we decided; ok?",
            vec![("we decided", Some(';')), (" ok", Some('?'))],
        ),
        (
            "on demo.example? yes",
            vec![("on demo.example", Some('?')), (" yes", None)],
        ),
        (
            "v1.5 of it. ok?",
            vec![("v1.5 of it", Some('.')), (" ok", Some('?'))],
        ),
        ("a,b? x", vec![("a,b", Some('?')), (" x", None)]),
        ("decided?x", vec![("decided?x", None)]),
        ("decided?\" x", vec![("decided", Some('?')), ("\" x", None)]),
        ("decided,", vec![("decided", Some(','))]),
    ] {
        assert_eq!(clauses(line), expected, "{line}");
    }
}

#[test]
fn quality_a_redaction_marker_stands_for_text_that_was_there() {
    for marker in [PRIVATE_MARKER, CREDENTIAL_MARKER] {
        for (said_as, rest, expected) in [
            (
                Capture::Decision,
                "| decided | yes |",
                Some(Noise::TableRow),
            ),
            (Capture::Decision, "{\"decided\": true}", Some(Noise::Data)),
            (Capture::Decision, "(a.md) decided?", Some(Noise::Question)),
            (Capture::Decision, "we decided to keep it", None),
            (Capture::Correction, "stop, it is wrong", None),
        ] {
            // Also behind a mark of a list, and with one behind it: the
            // fronts come off in turn.
            for line in [
                format!("{marker} {rest}"),
                format!("- {marker}{rest}"),
                format!("{marker} - {rest}"),
                format!("- {marker} - {marker} {rest}"),
            ] {
                assert_eq!(shape(said_as, &line), expected, "{line}");
            }
        }
        // A label behind a number and a marker is a label.
        let numbered = format!("1. {marker} Decision: keep the old index");
        assert_eq!(shape(Capture::Decision, &numbered), None, "{numbered}");
    }
}

#[test]
fn quality_a_line_of_very_many_fronts_is_left_open() {
    let fronts = |count: usize| format!("{}decided?", "[x] ".repeat(count));
    assert_eq!(
        shape(Capture::Decision, &fronts(data::SHAPE_ROUNDS - 1)),
        Some(Noise::Question)
    );
    assert_eq!(shape(Capture::Decision, &fronts(data::SHAPE_ROUNDS)), None);
}

#[test]
fn quality_a_memory_is_noise_when_no_line_of_it_makes_a_statement() {
    for (kind, content, expected) in [
        // One line without a shape, wherever it stands.
        (
            Capture::Decision,
            "| a | decided |\nwe decided to keep it",
            None,
        ),
        (
            Capture::Decision,
            "we decided to keep it\n| a | decided |",
            None,
        ),
        (
            Capture::Decision,
            "1. we decided\n2. going with it\nDecision: keep it",
            None,
        ),
        // Every line has a shape: the first names the noise.
        (
            Capture::Decision,
            "| a | decided |\n1. we are going with it",
            Some(Noise::TableRow),
        ),
        (
            Capture::Decision,
            "1. we are going with it\n| a | decided |",
            Some(Noise::ListStep),
        ),
        // A line that holds no phrase is not looked at.
        (
            Capture::Decision,
            "the plan of the week\n| a | decided |\nthat is all",
            Some(Noise::TableRow),
        ),
        (
            Capture::Decision,
            "the plan of the week",
            Some(Noise::NotADecision),
        ),
        (Capture::Decision, "", Some(Noise::NotADecision)),
        (Capture::Correction, "", Some(Noise::NotACorrection)),
        (
            Capture::Correction,
            "the plan of the week",
            Some(Noise::NotACorrection),
        ),
        // The marker that accepts sits in a pasted row, the correction
        // stands above it.
        (
            Capture::Correction,
            "don't touch it, the run says:\n| stop | now |",
            None,
        ),
        (
            Capture::Correction,
            "the run says:\n| stop | now |",
            Some(Noise::TableRow),
        ),
        (
            Capture::Correction,
            "the run says:\n{\"event\": \"stop\"}\n| stop | now |",
            Some(Noise::Data),
        ),
        // The marker that accepts sits in a row, the decision above it.
        (
            Capture::Correction,
            "Decision: keep it.\n| stop | reason |",
            None,
        ),
        (
            Capture::Correction,
            "1. Decision: stop caching errors.",
            None,
        ),
        // A line with a phrase is judged as a decision even in a
        // correction: it may ask, and a link in front of it is a front.
        (
            Capture::Correction,
            "| stop | now |\nare we going with it? yes",
            Some(Noise::TableRow),
        ),
        (
            Capture::Correction,
            "| stop | now |\n[a](b.md) we decided to keep it",
            None,
        ),
        // The whole content is the inside of an element.
        (
            Capture::Decision,
            "abc1234 switching to tokio for the runtime\n</local-command-stdout>",
            Some(Noise::Markup),
        ),
        (
            Capture::Decision,
            "abc1234 switching to tokio for the runtime\n</local-command-stdout>\n",
            Some(Noise::Markup),
        ),
        (
            Capture::Correction,
            "<stdout>\nstop the import\n</stdout>",
            Some(Noise::Markup),
        ),
        (Capture::Decision, "we decided to keep it\n</b>x", None),
        (Capture::Decision, "we decided to keep it\n</b c>", None),
        (Capture::Decision, "we decided to keep it\n</3>", None),
        (Capture::Decision, "</b>\nwe decided to keep it", None),
        (Capture::Decision, "</b>", Some(Noise::NotADecision)),
        (
            Capture::Decision,
            "we decided\n</a.b-c_d:e>",
            Some(Noise::Markup),
        ),
        (Capture::Decision, "we decided\n</b", None),
        (Capture::Decision, "we decided\n</b>>", None),
        // A statement outside the element is a statement.
        (
            Capture::Correction,
            "stop, keep it\n<x>\nfield=old\n</x>",
            None,
        ),
        // Elements of the same name nested inside are counted.
        (
            Capture::Correction,
            "<x>\nstop internal\n<x>\ndata\n</x>\n</x>",
            Some(Noise::Markup),
        ),
        (
            Capture::Correction,
            "stop, keep it\n<x>\n<x>\ndata\n</x>\n</x>",
            None,
        ),
        (
            Capture::Correction,
            "stop, keep it\n<x>\n<x>data\n</x>\n</x>",
            None,
        ),
        (
            Capture::Correction,
            "stop, keep it\n<x>\n<x>data</x>\n</x>",
            None,
        ),
        (
            Capture::Correction,
            "stop, keep it\n<x a=\"1\">\ndata\n</x>",
            None,
        ),
        (
            Capture::Correction,
            "stop internal\n<x>\ndata\n</x>\n</x>",
            Some(Noise::Markup),
        ),
        // A closing tag inside the quotes of an attribute is no tag, and
        // a tab behind the name is a space.
        (
            Capture::Correction,
            "stop, keep it\n<x data=\"</x>\">\ndata\n</x>",
            None,
        ),
        (
            Capture::Correction,
            "stop, keep it\n<x\tdata=\"1\">\ndata\n</x>",
            None,
        ),
        (
            Capture::Correction,
            "stop, keep it\n<xy>\n<x>\ndata\n</x>",
            None,
        ),
        (
            Capture::Correction,
            "<x>\nstop the import\n</x>\nstop, keep it",
            None,
        ),
        (
            Capture::Decision,
            "<x a=\"1\">\nwe decided\n</x>\nDecision: keep it",
            None,
        ),
        (
            Capture::Decision,
            "| decided |\n<x>\nwe decided\n</x>",
            Some(Noise::TableRow),
        ),
        (
            Capture::Decision,
            "<x>\nwe decided\n</x>\n| decided |",
            None,
        ),
    ] {
        assert_eq!(judge_content(kind, content), expected, "{content}");
    }
}

#[test]
fn quality_a_reply_taken_for_a_correction_may_state_a_decision() {
    for (content, expected) in [
        ("The build stopped; let's use SQLite.", None),
        ("The build stopped.\nDecision: keep the old index.", None),
        (
            "The build stopped.\n| let's use | sqlite |",
            Some(Noise::TableRow),
        ),
        // It is judged as a decision, so what it asks is a question.
        (
            "The build stopped; are we going with it?",
            Some(Noise::Question),
        ),
        ("The build stopped.", Some(Noise::NotACorrection)),
    ] {
        assert!(!is_correction(content), "{content}");
        assert_eq!(
            judge_content(Capture::Correction, content),
            expected,
            "{content}"
        );
    }
}

#[test]
fn quality_the_statement_line_is_the_first_line_that_states() {
    for (kind, content, expected) in [
        (
            Capture::Decision,
            "| a | decided |\nwe decided to keep it\nDecision: keep it",
            Some("we decided to keep it"),
        ),
        (Capture::Decision, "| a | decided |", None),
        (Capture::Decision, "the plan of the week", None),
        (
            Capture::Correction,
            "the run says:\n| stop | now |\n  stop, it is wrong  ",
            Some("  stop, it is wrong  "),
        ),
        (
            Capture::Correction,
            "The build stopped.\nDecision: keep it",
            Some("Decision: keep it"),
        ),
        // A line inside an element states nothing.
        (
            Capture::Decision,
            "Decision: keep it\n<x>\nwe decided\n</x>",
            Some("Decision: keep it"),
        ),
        (Capture::Decision, "<x>\nwe decided\n</x>", None),
        (
            Capture::Decision,
            "| decided |\n<x>\nwe decided\n</x>",
            None,
        ),
    ] {
        assert_eq!(
            statement_line(kind, content).as_deref(),
            expected,
            "{content}"
        );
    }
}

#[test]
fn quality_stored_contents_of_earlier_shapes_get_these_verdicts() {
    for (kind, content, expected) in STORED_EARLIER {
        assert_eq!(judge_content(*kind, content), *expected, "{content}");
    }
}

#[test]
fn quality_what_is_neither_is_named_by_the_rule_of_wording() {
    for text in NO_CORRECTION {
        assert!(!is_correction(text), "{text}");
        assert_eq!(
            judge_content(Capture::Correction, text),
            Some(Noise::NotACorrection),
            "{text}"
        );
    }
    for text in NO_DECISION {
        assert!(!holds_decision(text), "{text}");
        assert_eq!(
            judge_content(Capture::Decision, text),
            Some(Noise::NotADecision),
            "{text}"
        );
    }
}

#[test]
fn quality_only_a_capture_from_a_conversation_is_judged() {
    let row = "| option | decided |";
    let order = "| stop | now |";
    for source in [EventSource::ConversationWatcher, EventSource::CodexWatcher] {
        let decision = memory(
            source.clone(),
            MemoryType::Decision,
            &["decision", "conversation"],
            row,
        );
        assert_eq!(capture_kind(&decision), Some(Capture::Decision));
        assert_eq!(judge(&decision), Some(Noise::TableRow));
        let correction = memory(
            source.clone(),
            MemoryType::Feedback,
            &["feedback", "correction"],
            order,
        );
        assert_eq!(capture_kind(&correction), Some(Capture::Correction));
        assert_eq!(judge(&correction), Some(Noise::TableRow));

        // Tags the owner added later change nothing.
        let tagged = memory(
            source.clone(),
            MemoryType::Decision,
            &["demoapp", "conversation", "decision"],
            row,
        );
        assert_eq!(judge(&tagged), Some(Noise::TableRow));

        // The type and the tag go together.
        for (memory_type, tags) in [
            (MemoryType::Decision, &["decision"][..]),
            (MemoryType::Decision, &["feedback", "correction"][..]),
            (MemoryType::Decision, &["conversations"][..]),
            (MemoryType::Decision, &[][..]),
            (MemoryType::Feedback, &["feedback"][..]),
            (MemoryType::Feedback, &["decision", "conversation"][..]),
            (MemoryType::Note, &["decision", "conversation"][..]),
            (MemoryType::Note, &["feedback", "correction"][..]),
            (
                MemoryType::SessionSummary,
                &["decision", "conversation"][..],
            ),
            (MemoryType::Security, &["feedback", "correction"][..]),
        ] {
            let other = memory(source.clone(), memory_type, tags, row);
            assert_eq!(capture_kind(&other), None, "{tags:?}");
            assert_eq!(judge(&other), None, "{tags:?}");
        }
    }
    // What was saved on purpose, sent over the socket, captured from git or
    // from files, or fused by reflection from captures (it keeps their type
    // and their tags, and its source is the one of a save).
    for source in [
        EventSource::Manual,
        EventSource::Socket,
        EventSource::GitWatcher,
        EventSource::FileWatcher,
    ] {
        for (memory_type, tags, content) in [
            (MemoryType::Decision, ["decision", "conversation"], row),
            (MemoryType::Feedback, ["feedback", "correction"], order),
        ] {
            let mut other = memory(source.clone(), memory_type, &tags, content);
            other.metadata = serde_json::json!({"consolidated_from": ["a", "b"]});
            assert_eq!(capture_kind(&other), None);
            assert_eq!(judge(&other), None);
        }
    }
}
