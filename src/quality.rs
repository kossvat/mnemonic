//! Naming noise among memories captured from conversations.
//!
//! Capture takes a reply for a decision or a correction when it holds one of
//! a list of phrases. What it stores is sometimes not a statement at all: a
//! row of a table, a piece of output, a link, or a request in which a marker
//! sits inside a longer word. The rules here name such a memory.
//!
//! They are deterministic and read the stored memory alone: no store, no
//! clock, no model. Only a memory captured from a conversation is ever
//! noise. What the owner or an agent saved on purpose is not judged.
//!
//! The rules lean toward keeping. A memory is noise only when no line of it
//! makes a statement: every line that holds a phrase or a marker as a word
//! has the shape of something else, or no line holds one at all.
//!
//! Naming is all this module does. Nothing here hides, changes or removes a
//! memory.

use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::quality_data as data;

/// Why a captured memory is not a statement. The names are stable: they are
/// shown to the owner and carried in lists of memories.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Noise {
    TableRow,
    Markup,
    ImageLink,
    Link,
    Data,
    ListStep,
    Question,
    NotADecision,
    NotACorrection,
}

impl Noise {
    /// Every rule, in the order the rules are tried.
    #[cfg_attr(not(test), allow(dead_code))]
    pub const ALL: [Noise; 9] = [
        Noise::TableRow,
        Noise::Markup,
        Noise::ImageLink,
        Noise::Link,
        Noise::Data,
        Noise::ListStep,
        Noise::Question,
        Noise::NotADecision,
        Noise::NotACorrection,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            Noise::TableRow => "table-row",
            Noise::Markup => "markup",
            Noise::ImageLink => "image-link",
            Noise::Link => "link",
            Noise::Data => "data",
            Noise::ListStep => "list-step",
            Noise::Question => "question",
            Noise::NotADecision => "not-a-decision",
            Noise::NotACorrection => "not-a-correction",
        }
    }
}

/// What capture took a reply for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capture {
    Decision,
    Correction,
}

/// A memory that a composition of the context left out, and why. The
/// rules name the noise; the composition reports it.
#[derive(Debug, Clone, PartialEq)]
pub struct Hidden {
    pub id: String,
    pub memory_type: MemoryType,
    pub title: String,
    pub reason: HiddenReason,
}

impl Hidden {
    pub fn of(entry: &MemoryEntry, reason: HiddenReason) -> Self {
        Self {
            id: entry.id.clone(),
            memory_type: entry.memory_type.clone(),
            title: entry.title.clone(),
            reason,
        }
    }
}

/// Why a composition left a memory out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HiddenReason {
    /// The memory is noise by this rule.
    Noise(Noise),
    /// A decision that a confirmed conflict replaced, and the replacement
    /// is noise. The replacement is not shown, and the decision it
    /// replaced stays not shown, as it would with the replacement shown:
    /// the conflict holds whatever the text of the newer decision.
    ReplacedByNoise { replacement: String },
}

impl HiddenReason {
    /// The name shown to the owner: the rule, or replaced-by-noise.
    pub fn as_str(&self) -> &'static str {
        match self {
            HiddenReason::Noise(rule) => rule.as_str(),
            HiddenReason::ReplacedByNoise { .. } => "replaced-by-noise",
        }
    }
}

/// Whether `entry` was captured from a conversation, and as what. Anything
/// else (an explicit save, a fact, a summary, a capture from git or files, a
/// memory that reflection fused from others) gives `None` and is never
/// judged.
pub fn capture_kind(entry: &MemoryEntry) -> Option<Capture> {
    if !matches!(
        entry.source,
        EventSource::ConversationWatcher | EventSource::CodexWatcher
    ) {
        return None;
    }
    let tagged = |tag: &str| entry.tags.iter().any(|t| t == tag);
    match entry.memory_type {
        MemoryType::Decision if tagged("conversation") => Some(Capture::Decision),
        MemoryType::Feedback if tagged("correction") => Some(Capture::Correction),
        _ => None,
    }
}

/// The rule that names `entry` as noise, if one does.
pub fn judge(entry: &MemoryEntry) -> Option<Noise> {
    judge_content(capture_kind(entry)?, &entry.content)
}

/// [`judge`] for a stored content whose kind is known.
///
/// The lines that could be the statement are looked at, each by the rules
/// of shape in the order of [`Noise::ALL`]. One line without a shape and the
/// memory is kept. When every one has a shape the first names the noise,
/// and when there is no such line the rule of wording does.
pub(crate) fn judge_content(kind: Capture, content: &str) -> Option<Noise> {
    let clean = strip_attachment_refs(content);
    let lines = statement_lines(kind, &clean);
    let Some((said_as, at, first)) = lines.first() else {
        return Some(match kind {
            Capture::Decision => Noise::NotADecision,
            Capture::Correction => Noise::NotACorrection,
        });
    };
    let element = element_at_end(&clean);
    let shape_of = |said_as: Capture, at: usize, line: &str| {
        if element.as_ref().is_some_and(|inside| inside.contains(&at)) {
            Some(Noise::Markup)
        } else {
            shape(said_as, line)
        }
    };
    let named = shape_of(*said_as, *at, first)?;
    lines[1..]
        .iter()
        .all(|(said_as, at, line)| shape_of(*said_as, *at, line).is_some())
        .then_some(named)
}

/// The first line of `content` that makes a statement: it holds a phrase or
/// a marker as a word and has no shape. It is not the headline that is
/// shown today, which is the first line of some length whatever it says.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn statement_line(kind: Capture, content: &str) -> Option<String> {
    let clean = strip_attachment_refs(content);
    let element = element_at_end(&clean);
    statement_lines(kind, &clean)
        .into_iter()
        .filter(|(_, at, _)| !element.as_ref().is_some_and(|inside| inside.contains(at)))
        .find(|(said_as, _, line)| shape(*said_as, line).is_none())
        .map(|(_, _, line)| line.to_string())
}

/// The lines of `clean` that are the inside of an element, when its last
/// line that is not blank is a closing tag alone. The inside runs from the
/// line that is the opening tag paired with it, with elements of the same
/// name nested inside counted, or from the first line when there is none:
/// the excerpt of a decision cuts the opening tag away. That is how the
/// output of a command is written into a transcript, and none of it is
/// said by the owner.
fn element_at_end(clean: &str) -> Option<std::ops::Range<usize>> {
    let lines: Vec<&str> = clean.lines().map(str::trim).collect();
    let end = lines.iter().rposition(|line| !line.is_empty())?;
    let name = lines[end].strip_prefix("</")?.strip_suffix('>')?;
    if !name.starts_with(char::is_alphabetic) || !name.chars().all(is_of_a_tag_name) {
        return None;
    }
    // The tags of the same name a line holds, opening and closing, wherever
    // they stand on it: an element may open on a line of text.
    let mut depth: isize = 1;
    let mut start = 0;
    for (at, line) in lines[..end].iter().enumerate().rev() {
        let (opens, closes) = tags_of(line, name);
        depth += closes as isize - opens as isize;
        if depth <= 0 {
            start = at;
            break;
        }
    }
    Some(start..end + 1)
}

/// How many tags of `name` open and close on `line`. A tag opens with the
/// name and a closing bracket or a space of any kind; what stands inside
/// the quotes of its attributes is not read as a tag.
fn tags_of(line: &str, name: &str) -> (usize, usize) {
    let mut opens = 0;
    let mut closes = 0;
    let mut rest = line;
    while let Some(at) = rest.find('<') {
        let tag = &rest[at + 1..];
        if let Some(after) = tag.strip_prefix('/').and_then(|t| t.strip_prefix(name))
            && after.starts_with('>')
        {
            closes += 1;
            rest = &after[1..];
            continue;
        }
        if let Some(after) = tag.strip_prefix(name)
            && after
                .chars()
                .next()
                .is_some_and(|c| c == '>' || c.is_whitespace())
        {
            opens += 1;
            // To the bracket that closes the tag, outside quotes.
            let mut quote = None;
            let mut end = after.len();
            for (offset, c) in after.char_indices() {
                match quote {
                    Some(open) if c == open => quote = None,
                    Some(_) => {}
                    None if is_quote(c) => quote = Some(c),
                    None if c == '>' => {
                        end = offset + 1;
                        break;
                    }
                    None => {}
                }
            }
            rest = &after[end..];
            continue;
        }
        rest = tag;
    }
    (opens, closes)
}

fn is_of_a_tag_name(c: char) -> bool {
    c.is_alphanumeric() || "-_:.".contains(c)
}

/// The lines of `clean` that hold a phrase or a marker as a word, each
/// with what it is judged as and its number.
///
/// A line with a phrase of a decision is judged as a decision. When the
/// memory is a correction and the strict predicate accepts it, a line with
/// a marker is judged as a correction as well: the marker that made the
/// predicate accept may sit in a pasted row while the correction, or a
/// decision, stands on the line above it. A reply that capture took for a
/// correction by a marker inside a longer word may still state a decision:
/// then its lines are those of a decision alone.
fn statement_lines(kind: Capture, clean: &str) -> Vec<(Capture, usize, &str)> {
    let correction = kind == Capture::Correction && is_correction(clean);
    let mut lines = Vec::new();
    for (at, line) in clean.lines().enumerate() {
        let plain = plain(line);
        if correction && holds_marker(&plain) {
            lines.push((Capture::Correction, at, line));
        }
        if holds_phrase(&plain) {
            lines.push((Capture::Decision, at, line));
        }
    }
    lines
}

fn holds_phrase(plain: &str) -> bool {
    holds_any(plain, data::DECISION)
}

fn holds_negatable(plain: &str) -> bool {
    holds_any(plain, data::CORRECTION_NEGATABLE)
}

fn holds_weak(plain: &str) -> bool {
    holds_any(plain, data::CORRECTION_WEAK)
}

/// Whether `plain` holds a marker of a correction, of any strength.
fn holds_marker(plain: &str) -> bool {
    holds_strong(plain) || holds_negatable(plain) || holds_weak(plain)
}

/// Whether a piece of a line says something of the kind `said_as`.
fn says(said_as: Capture, piece: &str) -> bool {
    let piece = plain(piece);
    match said_as {
        Capture::Decision => holds_phrase(&piece),
        Capture::Correction => holds_marker(&piece),
    }
}

/// The shape of a line, if it has one. What stands in front of a statement
/// (a mark of a list, a label in brackets, a link, one opening tag, the
/// number of a labelled item) is taken off and the rest is read again, so a
/// statement behind a link is a statement.
fn shape(said_as: Capture, line: &str) -> Option<Noise> {
    let mut line = bare(line);
    // Every round takes something off the front of the line, so the rounds
    // end; the bound is for a line made to have very many fronts.
    for _ in 0..data::SHAPE_ROUNDS {
        let mut chars = line.chars();
        let first = chars.next()?;
        let second = chars.next();

        if is_bar(first) || bars(line) >= 2 {
            return Some(Noise::TableRow);
        }
        if opens_tag(line) {
            match after_one_opening_tag(line) {
                Some(rest) if says(said_as, rest) => {
                    line = bare(rest);
                    continue;
                }
                _ => return Some(Noise::Markup),
            }
        }
        if first == '!' && second == Some('[') {
            match after_link(&line[1..]) {
                Some(rest) if says(said_as, rest) => {
                    line = bare(rest);
                    continue;
                }
                _ => return Some(Noise::ImageLink),
            }
        }
        if first == '[' {
            match after_link(line) {
                Some(rest) if says(said_as, rest) => {
                    line = bare(rest);
                    continue;
                }
                Some(_) => return Some(Noise::Link),
                None => {}
            }
            if line[1..]
                .trim_start()
                .starts_with(|c| c == '{' || is_quote(c))
            {
                return Some(Noise::Data);
            }
            // A label in brackets, or the box of a task, with a statement
            // behind it.
            if let Some(rest) = after_brackets(line).filter(|rest| says(said_as, rest)) {
                line = bare(rest);
                continue;
            }
        }
        if first == '{' {
            return Some(Noise::Data);
        }
        if let Some(value) = after_quoted_key_or_element(line) {
            // A quoted word and a colon open a correction as well: the key
            // the agent got wrong, and what is right behind it.
            match value {
                Some(rest) if says(said_as, rest) => {
                    line = bare(rest);
                    continue;
                }
                _ => return Some(Noise::Data),
            }
        }
        if let Some(rest) = after_list_number(line) {
            if opens_with_label(rest) {
                line = bare(rest);
                continue;
            }
            return Some(Noise::ListStep);
        }
        if said_as == Capture::Decision && asks(line) {
            return Some(Noise::Question);
        }
        return None;
    }
    None
}

/// A line as the rules of shape read it: without the marks a list, a quote
/// or a heading puts in front, and without redaction markers at its start.
/// A marker stands for text that was there; what follows it is what the
/// line says.
fn bare(line: &str) -> &str {
    let mut rest = line.trim();
    loop {
        let before = rest;
        rest =
            rest.trim_start_matches(|c: char| data::LINE_MARKS.contains(&c) || c.is_whitespace());
        if let Some(after) = rest.strip_prefix(data::REDACTION_MARKER_OPEN)
            && let Some(end) = after.find(']')
        {
            rest = after[end + 1..].trim_start();
        }
        if rest == before {
            return rest;
        }
    }
}

fn is_bar(c: char) -> bool {
    data::BARS.contains(&c)
}

/// The bars of a line that could be the walls of cells: a bar right after
/// a bar is an operator, and a bar inside a span of code is code.
fn bars(line: &str) -> usize {
    let mut count = 0;
    // A span of code opens with a run of backticks and closes with a run
    // of the same length.
    let mut in_code: Option<usize> = None;
    let mut run = 0;
    let mut after_bar = false;
    for c in line.chars().chain(std::iter::once(' ')) {
        if c == '`' {
            run += 1;
            after_bar = false;
            continue;
        }
        if run > 0 {
            in_code = match in_code {
                None => Some(run),
                Some(open) if open == run => None,
                open => open,
            };
            run = 0;
        }
        if is_bar(c) && in_code.is_none() {
            if after_bar {
                count -= 1;
            } else {
                count += 1;
            }
            after_bar = !after_bar;
        } else {
            after_bar = false;
        }
    }
    count
}

fn is_quote(c: char) -> bool {
    matches!(c, '"' | '\'')
}

/// Whether `text` opens with a tag: an angle bracket followed by a letter,
/// a slash, an exclamation mark or a question mark. A bracket followed by a
/// space or a digit is a comparison.
fn opens_tag(text: &str) -> bool {
    text.strip_prefix('<')
        .and_then(|rest| rest.chars().next())
        .is_some_and(|c| c.is_alphabetic() || "/!?".contains(c))
}

/// What follows the tag a line opens with, when that tag opens something,
/// is closed by an angle bracket, and no other tag follows on the line.
fn after_one_opening_tag(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('<')?;
    if !inner.starts_with(char::is_alphabetic) {
        return None;
    }
    // The bracket that closes the tag is the first one outside quotes.
    let mut quote = None;
    let end = inner.char_indices().find_map(|(at, c)| match quote {
        Some(open) if c == open => {
            quote = None;
            None
        }
        Some(_) => None,
        None if is_quote(c) => {
            quote = Some(c);
            None
        }
        None => (c == '>').then_some(at),
    })?;
    let rest = &inner[end + 1..];
    let another = rest
        .match_indices('<')
        .any(|(at, _)| opens_tag(&rest[at..]));
    (!inner[..end].ends_with('/') && !another).then_some(rest)
}

/// Where the `open` that `line` opens with is closed by its `close`, with
/// the pairs inside counted, as the offset behind the closing one.
fn closing_of(line: &str, open: char, close: char) -> Option<usize> {
    let mut depth = 0usize;
    for (at, c) in line.char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            depth = depth.checked_sub(1)?;
            if depth == 0 {
                return Some(at + close.len_utf8());
            }
        }
    }
    None
}

/// What follows the link a line opens with: `[text](target)`,
/// `[text][reference]` or `[[text]]`. The bracket that opens the line has
/// to be the one the target follows.
fn after_link(line: &str) -> Option<&str> {
    if !line.starts_with('[') {
        return None;
    }
    let rest = &line[closing_of(line, '[', ']')?..];
    if line.starts_with("[[") {
        return Some(rest);
    }
    match rest.chars().next()? {
        '(' => closing_of(rest, '(', ')').map(|end| &rest[end..]),
        '[' => closing_of(rest, '[', ']').map(|end| &rest[end..]),
        _ => None,
    }
}

/// What follows the brackets a line opens with, when text follows them.
fn after_brackets(line: &str) -> Option<&str> {
    let rest = line[closing_of(line, '[', ']')?..].trim_start();
    (!rest.is_empty()).then_some(rest)
}

/// `"key": value` as a line of data opens, with the key in quotes of
/// either kind, giving the value; or a text in quotes alone, followed by
/// a closing bracket, or followed by a comma and another value or
/// nothing, as an element of a list, giving nothing. A text in quotes
/// followed by a comma and prose is prose.
fn after_quoted_key_or_element(line: &str) -> Option<Option<&str>> {
    let mut chars = line.char_indices();
    let (_, quote) = chars.next().filter(|(_, c)| is_quote(*c))?;
    let mut escaped = false;
    for (at, c) in chars {
        if escaped {
            escaped = false;
        } else if c == '\\' {
            escaped = true;
        } else if c == quote {
            let rest = line[at + c.len_utf8()..].trim();
            return match rest.strip_prefix(':') {
                Some(value) => Some(Some(value)),
                None => {
                    let element = rest.is_empty()
                        || rest.starts_with(['}', ']'])
                        || rest.strip_prefix(',').is_some_and(is_a_tail_of_data);
                    element.then_some(None)
                }
            };
        }
    }
    None
}

/// Whether what follows a comma is more data and not prose: nothing, or
/// values with letters only inside quotes, apart from the words of data
/// that are written bare.
fn is_a_tail_of_data(tail: &str) -> bool {
    let mut bare = String::with_capacity(tail.len());
    let mut quote = None;
    let mut escaped = false;
    for c in tail.chars() {
        match quote {
            Some(open) => {
                if escaped {
                    escaped = false;
                } else if c == '\\' {
                    escaped = true;
                } else if c == open {
                    quote = None;
                }
            }
            None if is_quote(c) => quote = Some(c),
            None => bare.push(c),
        }
    }
    quote.is_none()
        && bare
            .split(|c: char| !c.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .all(|word| {
                word.chars().all(|c| c.is_ascii_digit()) || data::WORDS_OF_DATA.contains(&word)
            })
}

/// What follows the number a step of a list opens with, when a space
/// follows the number: `1.`, `2.3.`, `12)`, `(4)`, `a)`, also in bold.
fn after_list_number(line: &str) -> Option<&str> {
    let rest = behind_number(line)?.trim_start_matches(['*', '_']);
    rest.starts_with(char::is_whitespace)
        .then(|| rest.trim_start())
}

/// The rest of a line behind the number it opens with. A number and a dot
/// take up to three digits a level, so a year that opens a sentence is not
/// a step; a version, which has no dot at its end, is not one either.
fn behind_number(line: &str) -> Option<&str> {
    let digits = |text: &str| text.bytes().take_while(u8::is_ascii_digit).count();
    if let Some(inner) = line.strip_prefix('(') {
        let count = digits(inner);
        return (count > 0)
            .then(|| inner[count..].strip_prefix(')'))
            .flatten();
    }
    let count = digits(line);
    if count == 0 {
        let letter = line.chars().next().filter(|c| c.is_alphabetic())?;
        return line[letter.len_utf8()..].strip_prefix(')');
    }
    let mut rest = &line[count..];
    let mut short = count <= 3;
    let mut levels = 0;
    // Further levels: a dot and digits.
    while let Some(level) = rest.strip_prefix('.') {
        let more = digits(level);
        if more == 0 {
            break;
        }
        short &= more <= 3;
        levels += 1;
        rest = &level[more..];
    }
    match rest.strip_prefix(')') {
        Some(behind) if short || levels == 0 => Some(behind),
        Some(_) => None,
        None if short => rest.strip_prefix('.'),
        None => None,
    }
}

/// Whether what follows the number of a step opens with a label of a
/// decision, or with a marker of a correction: a numbered statement under
/// a heading capture cut off, or a numbered answer that leads with its
/// refusal, not a step of an instruction.
fn opens_with_label(rest: &str) -> bool {
    let rest = plain(bare(rest));
    data::DECISION
        .iter()
        .filter(|phrase| phrase.ends_with(':'))
        .any(|label| rest.starts_with(label))
        || data::CORRECTION_STRONG
            .iter()
            .chain(data::CORRECTION_NEGATABLE)
            .chain(data::CORRECTION_WEAK)
            .any(|marker| occurrences(&rest, marker).next() == Some(0))
}

/// Whether every phrase of a decision on the line stands in a clause that
/// ends with a question mark. A statement followed by a question, in the
/// next sentence or behind a comma, is a statement.
fn asks(line: &str) -> bool {
    let line = plain(line);
    let mut any = false;
    for (clause, end) in clauses(&line) {
        if holds_phrase(clause) {
            any = true;
            if end != Some('?') {
                return false;
            }
        }
    }
    any
}

/// The clauses of a line, each with the mark that ends it: the first full
/// stop, exclamation mark, question mark, comma or semicolon that closing
/// marks and then a space or the end of the line follow. The last piece
/// may have none.
fn clauses(line: &str) -> Vec<(&str, Option<char>)> {
    let mut out = Vec::new();
    let mut start = 0;
    for (at, c) in line.char_indices() {
        if !data::CLAUSE_ENDS.contains(&c) {
            continue;
        }
        let behind = line[at + 1..].trim_start_matches(data::CLOSING_MARKS);
        if behind.is_empty() || behind.starts_with(char::is_whitespace) {
            out.push((&line[start..at], Some(c)));
            start = at + 1;
        }
    }
    if start < line.len() {
        out.push((&line[start..], None));
    }
    out
}

/// Whether `line` holds a decision phrase as a word.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn holds_decision(line: &str) -> bool {
    holds_phrase(&plain(&strip_attachment_refs(line)))
}

/// The predicate of corrections that capture uses, step by step and in its
/// order, with these differences: a marker that accepts has to stand as a
/// word, a strong marker inside a phrase of everyday speech does not count,
/// and an apostrophe of any shape is an apostrophe. A negation refuses as
/// capture has it, wherever its letters stand: narrowing what refuses would
/// widen what is accepted.
pub(crate) fn is_correction(text: &str) -> bool {
    let lowered = strip_attachment_refs(text).to_lowercase();
    // Corrections are brief. The length is the one capture measures, with
    // the apostrophes as they were written. An empty reply holds no
    // marker and is refused by that.
    if lowered.trim().len() > data::CORRECTION_MAX_BYTES {
        return false;
    }
    let plain = plain(&lowered);
    let trimmed = plain.trim();
    // A reply that ends in a question mark asks; it does not steer.
    if trimmed.ends_with('?') {
        return false;
    }
    if holds_strong(trimmed) {
        return true;
    }
    if data::CORRECTION_NEGATED
        .iter()
        .any(|negation| trimmed.contains(negation))
    {
        return false;
    }
    if holds_negatable(trimmed) {
        return true;
    }
    if trimmed.matches('?').count() >= 2 {
        return false;
    }
    data::CORRECTION_WEAK
        .iter()
        .any(|marker| holds_within(trimmed, marker, data::CORRECTION_HEAD_CHARS))
}

/// A text as the lists are matched against it: in lower case, with the
/// apostrophe of the keyboard for the ones of a typesetter.
fn plain(text: &str) -> String {
    text.to_lowercase().replace(data::APOSTROPHES, "'")
}

fn holds_any(plain: &str, words: &[&str]) -> bool {
    words.iter().any(|word| holds(plain, word))
}

/// Whether `plain` holds a strong marker as a word that is not part of a
/// phrase of everyday speech. A phrase counts as a word too.
fn holds_strong(plain: &str) -> bool {
    let mut idiom = vec![false; plain.len()];
    for phrase in data::CORRECTION_IDIOMS {
        for at in occurrences(plain, phrase) {
            idiom[at..at + phrase.len()].fill(true);
        }
    }
    data::CORRECTION_STRONG.iter().any(|marker| {
        occurrences(plain, marker).any(|at| !idiom[at..at + marker.len()].iter().all(|c| *c))
    })
}

/// Whether `needle` stands in `haystack` as a word: nothing that belongs to
/// a word touches it. A needle that ends in a colon or a comma ends by
/// itself and needs the boundary on its left only.
pub(crate) fn holds(haystack: &str, needle: &str) -> bool {
    occurrences(haystack, needle).next().is_some()
}

/// [`holds`], for an occurrence that lies whole within the first `chars`
/// characters. The boundary on its right is judged on the whole text, so a
/// word cut by the window is not taken for a whole one.
fn holds_within(haystack: &str, needle: &str, chars: usize) -> bool {
    let window = haystack
        .char_indices()
        .nth(chars)
        .map_or(haystack.len(), |(at, _)| at);
    occurrences(haystack, needle).any(|at| at + needle.len() <= window)
}

/// Where `needle` stands as a word in `haystack`, as byte offsets.
fn occurrences<'a>(haystack: &'a str, needle: &'a str) -> impl Iterator<Item = usize> + 'a {
    let open_right = needle.ends_with([':', ',']);
    let step = needle.chars().next().map_or(1, char::len_utf8);
    let mut from = 0;
    std::iter::from_fn(move || {
        if needle.is_empty() {
            return None;
        }
        while let Some(found) = haystack.get(from..).and_then(|rest| rest.find(needle)) {
            let at = from + found;
            let end = at + needle.len();
            from = at + step;
            let left = !joins_before(&haystack[..at]);
            let right = open_right || !joins_behind(&haystack[end..]);
            if left && right {
                return Some(at);
            }
        }
        None
    })
}

/// Whether a character makes what it touches part of a longer word: a
/// letter or a digit of any script; a mark that is written onto a letter
/// or into a word without showing (an accent of its own, a soft hyphen, a
/// joiner); a slash, which makes a path.
fn is_of_a_word(c: char) -> bool {
    c.is_alphanumeric() || c == '/' || data::UNSEEN_IN_WORDS.contains(&c) || is_accent(c)
}

/// Whether what stands before a needle makes it part of a longer word. A
/// hyphen, an underscore or a dot right before it does: a flag, the name
/// of a file that begins with a dot, the second half of a compound.
fn joins_before(before: &str) -> bool {
    before
        .chars()
        .next_back()
        .is_some_and(|c| is_of_a_word(c) || data::JOINERS.contains(&c))
}

/// Whether what stands behind a needle makes it part of a longer word. A
/// hyphen, an underscore or a dot does when a word goes on behind it, as
/// in a compound or the name of a file, but not when that word is one of
/// the particles of the data file; with a space behind it, a hyphen is a
/// dash and a dot ends the sentence.
fn joins_behind(behind: &str) -> bool {
    let mut chars = behind.chars();
    let Some(next) = chars.next() else {
        return false;
    };
    if is_of_a_word(next) {
        return true;
    }
    if !data::JOINERS.contains(&next) {
        return false;
    }
    let rest = &behind[next.len_utf8()..];
    // A particle is glued by a hyphen alone; behind an underscore or a
    // dot it is part of a name.
    let particle = next == '-'
        && data::PARTICLES.iter().any(|particle| {
            rest.strip_prefix(particle)
                .is_some_and(|after| !joins_behind(after))
        });
    !particle && rest.starts_with(char::is_alphanumeric)
}

/// An accent written as a character of its own after its letter: the
/// blocks of combining marks.
fn is_accent(c: char) -> bool {
    data::ACCENTS.iter().any(|range| range.contains(&c))
}

/// Text without references to attachments (`@"path"`): the name of a file
/// holds words that are not the owner's. A reference that is not closed
/// takes the rest of the text with it. The same as capture does.
pub(crate) fn strip_attachment_refs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("@\"") {
        out.push_str(&rest[..start]);
        match rest[start + 2..].find('"') {
            Some(end) => rest = &rest[start + 2 + end + 1..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
#[path = "quality_unit_tests.rs"]
mod tests;
