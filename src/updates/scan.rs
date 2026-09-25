//! Deterministic value scanner: the business values a memory states (money,
//! commercial percentages, payment terms, dated deadlines, numbers that
//! follow a predicate such as "limit") and the words that say what they are
//! values OF.
//!
//! No regex and no model. Identifiers that merely look numeric (versions,
//! ports, hashes, clock times, build numbers, sizes) are masked first, so two
//! notes that differ only in those still count as the same note.

use std::collections::BTreeSet;

use super::lexicon as lx;

/// Only this much of a memory is scanned; values live near the top.
pub const MAX_INPUT_BYTES: usize = 16 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Class {
    Money,
    Percent,
    Terms,
    Date,
    Number,
}

impl Class {
    pub fn as_str(self) -> &'static str {
        match self {
            Class::Money => "money",
            Class::Percent => "percent",
            Class::Terms => "terms",
            Class::Date => "date",
            Class::Number => "number",
        }
    }
}

/// One value: `key` compares, `surface` is what a human wrote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Value {
    pub class: Class,
    pub key: String,
    pub surface: String,
    /// The predicate the value belongs to, when its sentence named one
    /// before it ("commission is 10%"). Not part of the key.
    pub pred: Option<&'static str>,
    /// Identifying words between the previous value (or the sentence start)
    /// and this one: what this value is about ("Gadget" in "Widget $5,
    /// Gadget $6"). Not part of the key.
    pub context: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub struct Scan {
    /// Values the text asserts.
    pub values: Vec<Value>,
    /// Values it names as what held before ("was $5", "from $5 to $6").
    pub prior: Vec<Value>,
    /// Stems of the words that identify what the values belong to.
    pub words: BTreeSet<String>,
    /// Canonical predicates named (price, commission, ...).
    pub preds: BTreeSet<&'static str>,
    /// Some value is asked about, proposed or bounded rather than stated
    /// ("should it be $6?", "maybe $6", "<$6"): the text asserts no value
    /// that could replace another.
    pub hypothetical: bool,
}

impl Scan {
    pub fn has_values(&self) -> bool {
        !self.values.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Word(String),
    Num(String),
    /// `yyyy-mm-dd`
    Date(String),
    Cur(&'static str, char),
    Pct,
    Slash,
    /// A minus sign directly before an amount: `-5%`, `-$5`.
    Minus,
    /// A bound or approximation next to an amount: `<`, `>`, `~`, `$5+`.
    Op,
    /// A measurement or a numbered variant (`16gb`, `phase 1`, `#12`,
    /// `v2`): never a value, but part of what a value is about.
    Ident(String),
    /// The end of a question.
    Ask,
    Stop,
}

pub fn scan(text: &str) -> Scan {
    let tokens = tokenize(&prepare(text));
    Extractor::new(&tokens).run()
}

/// Lowercase, cap, fold odd spaces, and blank out code.
fn prepare(text: &str) -> String {
    let mut end = text.len().min(MAX_INPUT_BYTES);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    let lowered = text[..end].to_lowercase();
    let mut out = String::with_capacity(lowered.len());
    let mut fenced = false;
    for line in lowered.lines() {
        if line.trim_start().starts_with("```") {
            fenced = !fenced;
            out.push('\n');
            continue;
        }
        if fenced {
            out.push('\n');
            continue;
        }
        let mut inline = false;
        for c in line.chars() {
            match c {
                '`' => {
                    inline = !inline;
                    out.push(' ');
                }
                _ if inline => out.push(' '),
                '\u{a0}' | '\u{2009}' | '\u{202f}' | '\t' => out.push(' '),
                _ => out.push(c),
            }
        }
        out.push('\n');
    }
    out
}

const EDGE: &[char] = &[
    ',', ';', ':', '!', '?', '(', ')', '[', ']', '{', '}', '"', '\'', '«', '»', '.', '*', '_',
];

fn tokenize(text: &str) -> Vec<Tok> {
    let mut tokens = Vec::new();
    for line in text.lines() {
        let chunks: Vec<&str> = line.split(' ').filter(|c| !c.is_empty()).collect();
        let mut previous_word = String::new();
        for (i, raw) in chunks.iter().enumerate() {
            let core = raw.trim_matches(EDGE);
            let next = chunks.get(i + 1).map(|c| c.trim_matches(EDGE));
            // `:8080` loses its colon to the edge trim; it is still a port.
            let bare_port =
                raw.starts_with(':') && core.len() >= 2 && core.chars().all(|c| c.is_ascii_digit());
            let mask = if bare_port {
                Mask::Drop
            } else {
                masked(core, &previous_word, next)
            };
            if mask != Mask::Keep {
                if mask == Mask::Ident {
                    tokens.push(Tok::Ident(core.to_owned()));
                }
                if raw.ends_with('?') {
                    tokens.push(Tok::Ask);
                } else if raw.ends_with(['.', '!', ';']) {
                    tokens.push(Tok::Stop);
                }
                previous_word.clear();
                continue;
            }
            let before = tokens.len();
            split_chunk(raw, &mut tokens);
            if let Some(Tok::Word(word)) = tokens[before..]
                .iter()
                .rev()
                .find(|t| matches!(t, Tok::Word(_) | Tok::Num(_)))
            {
                previous_word = word.clone();
            } else {
                previous_word.clear();
            }
        }
        tokens.push(Tok::Stop);
    }
    tokens
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mask {
    Keep,
    /// Noise: links, paths, hashes, times, ports.
    Drop,
    /// Never a value, but it tells variants apart: `RAM 16GB` and `RAM
    /// 32GB` are two products (review point).
    Ident,
}

/// Identifiers and measurements that are never business values.
fn masked(core: &str, previous_word: &str, next: Option<&str>) -> Mask {
    if core.is_empty() {
        return Mask::Keep;
    }
    let is_number = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_digit() || c == '.' || c == ',')
            && s.starts_with(|c: char| c.is_ascii_digit())
    };
    let money_nearby = core.contains(['$', '€', '£', '₽'])
        || next.is_some_and(|n| lx::currency(n).is_some() || lx::multiplier(n).is_some());
    let noise = core.contains("://")
        || core.starts_with("www.")
        || email(core)
        || path(core)
        || uuid(core)
        || hex(core)
        || host_port(core)
        || clock(core)
        || iso_datetime(core, next);
    let variant = (version(core) && !money_nearby)
        || dimensions(core)
        || attached_unit(core)
        || (core.starts_with('#')
            && core[1..].chars().all(|c| c.is_ascii_digit())
            && core.len() > 1)
        || (is_number(core) && lx::counter_keyword(previous_word))
        // `per 1000 tokens` is a price's basis, not a measurement.
        || (is_number(core)
            && next.is_some_and(lx::measure_unit)
            && !matches!(previous_word, "per" | "за" | "на"));
    if noise {
        Mask::Drop
    } else if variant {
        Mask::Ident
    } else {
        Mask::Keep
    }
}

fn email(s: &str) -> bool {
    s.split_once('@')
        .is_some_and(|(user, host)| !user.is_empty() && host.contains('.'))
}

fn path(s: &str) -> bool {
    if s.starts_with("~/") || s.starts_with("./") || s.starts_with("../") || s.contains('\\') {
        return true;
    }
    let slashes = s.matches('/').count();
    let file = s.rsplit('/').next().unwrap_or("");
    (s.starts_with('/') && s.len() > 1 && (slashes > 1 || s.contains('.')))
        || (slashes >= 1
            && file.rsplit_once('.').is_some_and(|(stem, ext)| {
                !stem.is_empty()
                    && ext.chars().all(char::is_alphanumeric)
                    && ext.chars().any(char::is_alphabetic)
            }))
}

fn uuid(s: &str) -> bool {
    let parts: Vec<&str> = s.split('-').collect();
    parts.len() == 5
        && parts.iter().map(|p| p.len()).eq([8, 4, 4, 4, 12])
        && parts
            .iter()
            .all(|p| p.chars().all(|c| c.is_ascii_hexdigit()))
}

fn hex(s: &str) -> bool {
    (7..=40).contains(&s.len())
        && s.chars().all(|c| c.is_ascii_hexdigit())
        && s.chars().any(|c| c.is_ascii_digit())
        && s.chars().any(|c| c.is_ascii_alphabetic())
}

/// `v2`, `v2.1`, `1.2.3`, `10.0.0.1`, `v1.2.3-beta`.
fn version(s: &str) -> bool {
    let (prefixed, body) = match s.strip_prefix('v') {
        Some(rest) => (true, rest),
        None => (false, s),
    };
    let body = body.split(['-', '+']).next().unwrap_or("");
    let groups: Vec<&str> = body.split('.').collect();
    let numeric = groups
        .iter()
        .all(|g| !g.is_empty() && g.chars().all(|c| c.is_ascii_digit()));
    numeric && (groups.len() >= 3 || (prefixed && !body.is_empty()))
}

fn host_port(s: &str) -> bool {
    s.rsplit_once(':').is_some_and(|(host, port)| {
        (2..=5).contains(&port.len())
            && port.chars().all(|c| c.is_ascii_digit())
            && (host.is_empty() || host.ends_with(|c: char| c.is_alphanumeric()))
            && !clock(s)
    })
}

fn clock(s: &str) -> bool {
    let s = s.trim_end_matches("am").trim_end_matches("pm");
    let parts: Vec<&str> = s.split(':').collect();
    (2..=3).contains(&parts.len())
        && (1..=2).contains(&parts[0].len())
        && parts[1..].iter().all(|p| p.len() == 2)
        && parts.iter().all(|p| p.chars().all(|c| c.is_ascii_digit()))
}

fn iso_date(s: &str) -> bool {
    let b = s.as_bytes();
    s.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && s.chars()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit())
}

/// A date that carries a time is a timestamp, not a deadline.
fn iso_datetime(s: &str, next: Option<&str>) -> bool {
    (s.len() > 10
        && s.is_char_boundary(10)
        && iso_date(&s[..10])
        && s[10..].starts_with(['t', ' ']))
        || (iso_date(s) && next.is_some_and(clock))
}

/// `1920x1080`, `1080p`.
fn dimensions(s: &str) -> bool {
    if let Some((w, h)) = s.split_once('x') {
        return !w.is_empty()
            && !h.is_empty()
            && w.chars().all(|c| c.is_ascii_digit())
            && h.chars().all(|c| c.is_ascii_digit());
    }
    s.strip_suffix('p')
        .is_some_and(|n| n.len() >= 3 && n.chars().all(|c| c.is_ascii_digit()))
}

/// `200ms`, `30fps`, `16gb`.
fn attached_unit(s: &str) -> bool {
    let digits = s
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .count();
    digits > 0 && digits < s.chars().count() && {
        let unit: String = s.chars().skip(digits).collect();
        lx::measure_unit(&unit)
    }
}

/// One whitespace-separated chunk into tokens.
fn split_chunk(raw: &str, tokens: &mut Vec<Tok>) {
    let chars: Vec<char> = raw.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        // A sign, not a dash: at the start of the chunk (or after an opening
        // bracket) and right before an amount.
        let signs_amount = c == '-'
            && (i == 0 || matches!(chars[i - 1], '(' | '[' | '"' | '\''))
            && chars
                .get(i + 1)
                .is_some_and(|n| n.is_ascii_digit() || lx::currency(&n.to_string()).is_some());
        if signs_amount {
            tokens.push(Tok::Minus);
            i += 1;
        } else if let Some(code) = lx::currency(&c.to_string()) {
            tokens.push(Tok::Cur(code, c));
            i += 1;
        } else if c == '%' {
            tokens.push(Tok::Pct);
            i += 1;
        } else if c == '/' {
            tokens.push(Tok::Slash);
            i += 1;
        } else if c.is_ascii_digit() {
            let start = i;
            while i < chars.len()
                && (chars[i].is_ascii_digit()
                    || (matches!(chars[i], '.' | ',')
                        && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())))
            {
                i += 1;
            }
            let number: String = chars[start..i].iter().collect();
            // `2026-10-15`
            if number.len() == 4 && chars.get(i) == Some(&'-') && i + 6 <= chars.len() {
                let candidate: String = chars[start..i + 6].iter().collect();
                if iso_date(&candidate) {
                    tokens.push(Tok::Date(candidate));
                    i += 6;
                    continue;
                }
            }
            tokens.push(Tok::Num(number));
        } else if c.is_alphabetic() {
            let start = i;
            while i < chars.len()
                && (chars[i].is_alphabetic()
                    || (matches!(chars[i], '-' | '\'' | '’')
                        && chars.get(i + 1).is_some_and(|n| n.is_alphabetic())))
            {
                i += 1;
            }
            let word: String = chars[start..i].iter().collect();
            tokens.push(Tok::Word(word));
        } else {
            match c {
                '?' => tokens.push(Tok::Ask),
                '.' | '!' | ';' => tokens.push(Tok::Stop),
                '<' | '>' | '≤' | '≥' | '~' | '≈' => tokens.push(Tok::Op),
                // `$5+`: an open bound, not a sum (that has spaces around).
                '+' if i > 0 && chars[i - 1].is_ascii_digit() => tokens.push(Tok::Op),
                _ => {}
            }
            i += 1;
        }
    }
}

/// Canonical decimal text of an amount, exact (no float). With both `.`
/// and `,` in it, the last one is the decimal point and the others group
/// thousands; with one kind used more than once, all of them group; used
/// once, it groups only before exactly three digits after a non-zero
/// integer (1,200 but 0.005). `shift` multiplies by a power of ten (`5k`).
/// `None` when `raw` is not a number, or not a consistent one.
pub fn amount(raw: &str, shift: u32) -> Option<String> {
    let (negative, raw) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    let separators: Vec<(usize, char)> = raw
        .char_indices()
        .filter(|(_, c)| matches!(c, '.' | ','))
        .collect();
    let both =
        separators.iter().any(|(_, c)| *c == '.') && separators.iter().any(|(_, c)| *c == ',');
    let decimal_at = match separators.as_slice() {
        [] => None,
        [(at, _)] => {
            let (int, after) = (&raw[..*at], &raw[at + 1..]);
            let grouping = after.len() == 3 && int.chars().any(|c| c != '0');
            (!grouping).then_some(*at)
        }
        many if both => {
            let (at, mark) = *many.last()?;
            if many[..many.len() - 1].iter().any(|(_, c)| *c == mark) {
                return None;
            }
            Some(at)
        }
        _ => None,
    };
    let (int_part, frac) = match decimal_at {
        Some(at) => (&raw[..at], raw[at + 1..].to_owned()),
        None => (raw, String::new()),
    };
    // Every group after a grouping separator has exactly three digits.
    let groups: Vec<&str> = int_part.split(['.', ',']).collect();
    if groups
        .iter()
        .any(|g| g.is_empty() || !g.chars().all(|c| c.is_ascii_digit()))
        || groups[1..].iter().any(|g| g.len() != 3)
        || !frac.chars().all(|c| c.is_ascii_digit())
        || (decimal_at.is_some() && frac.is_empty())
    {
        return None;
    }
    let mut int: String = groups.concat();
    let mut frac = frac;
    for _ in 0..shift {
        if frac.is_empty() {
            int.push('0');
        } else {
            int.push(frac.remove(0));
        }
    }
    let int = int.trim_start_matches('0');
    let frac = frac.trim_end_matches('0');
    let mut text = String::from(if int.is_empty() { "0" } else { int });
    if !frac.is_empty() {
        text.push('.');
        text.push_str(frac);
    }
    if negative && text != "0" {
        text.insert(0, '-');
    }
    Some(text)
}

#[path = "extract.rs"]
mod extract;
use extract::Extractor;

#[cfg(test)]
#[path = "scan_tests.rs"]
mod tests;
