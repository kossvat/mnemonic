//! Values in credential context: `name = value`, `name: value`,
//! `"name": "value"`, `name => value`, `--name=value`, `NAME ?= value` and
//! `Authorization: <scheme> value`, as written in code, shells, env files,
//! Markdown and escaped JSON.

use super::Class;
use super::detect::{Span, push};

/// Names whose value is a credential. Bare `key` and lowercase `*_key` are
/// not: mnemonic's own records use them for identifiers.
const CREDENTIAL_NAMES: [&str; 10] = [
    "api_key",
    "apikey",
    "x_api_key",
    "access_token",
    "auth_token",
    "client_secret",
    "secret",
    "token",
    "password",
    "passwd",
];
/// Environment-style names (UPPER_SNAKE) ending in one of these, like
/// `OPENAI_API_KEY` or `GITHUB_TOKEN`.
const ENV_SUFFIXES: [&str; 6] = ["_KEY", "_TOKEN", "_SECRET", "_PASSWORD", "_PASSWD", "_PWD"];
/// The same words written in Russian.
const RU_NAMES: [&str; 3] = ["пароль", "токен", "секрет"];
/// HTTP authorization schemes kept readable in front of their credential.
const AUTH_SCHEMES: [&str; 9] = [
    "Bearer",
    "Basic",
    "Key",
    "Token",
    "Digest",
    "Negotiate",
    "NTLM",
    "ApiKey",
    "DPoP",
];

/// What a name makes of the value after it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Name {
    /// The value is the credential.
    Credential,
    /// `pwd`: a password in connection strings, the working directory in a
    /// shell environment; a path is left alone.
    Pwd,
    /// `Authorization`: an optional scheme word, then the credential.
    Authorization,
}

pub(super) fn credential_name(key: &str) -> Option<Name> {
    // `--token` is the flag for `token`.
    let key = key.trim_start_matches('-');
    let normalized = key.to_ascii_lowercase().replace('-', "_");
    match normalized.as_str() {
        "pwd" => return Some(Name::Pwd),
        "authorization" | "proxy_authorization" => return Some(Name::Authorization),
        n if CREDENTIAL_NAMES.contains(&n) => return Some(Name::Credential),
        _ => {}
    }
    if RU_NAMES.contains(&key.to_lowercase().as_str()) {
        return Some(Name::Credential);
    }
    let env_style = key
        .bytes()
        .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || b == b'_');
    let credential_suffix = ENV_SUFFIXES
        .iter()
        .any(|suffix| key.len() > suffix.len() && key.ends_with(suffix));
    (env_style && credential_suffix).then_some(Name::Credential)
}

/// Characters of an unquoted or quoted value: printable ASCII except
/// quotes, separators and brackets, so code and prose end it early.
pub(super) fn is_value(b: u8) -> bool {
    (0x21..=0x7e).contains(&b) && !b"\"'`,;()[]{}<>&".contains(&b)
}

fn is_key(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// A run of value characters, with what any suffix of it holds, so every
/// separator inside one run is judged in O(1): a run like
/// `token=token=...` would otherwise be rescanned from each separator.
struct Run {
    start: usize,
    /// First byte past the run.
    end: usize,
    /// The value's end: the run's, less the backslash of an escaped quote.
    value_end: usize,
    last_alpha: Option<usize>,
    last_digit: Option<usize>,
    /// Last byte that cannot be part of a dotted identifier path.
    last_undotted: Option<usize>,
}

impl Run {
    fn scan(s: &[u8], start: usize) -> Run {
        let mut end = start;
        while end < s.len() && is_value(s[end]) {
            end += 1;
        }
        let escaped_quote = end > start
            && s[end - 1] == b'\\'
            && s.get(end)
                .is_some_and(|&b| matches!(b, b'"' | b'\'' | b'`'));
        let value_end = if escaped_quote { end - 1 } else { end };
        let last = |pred: fn(&u8) -> bool| (start..value_end).rev().find(|&k| pred(&s[k]));
        Run {
            start,
            end,
            value_end,
            last_alpha: last(u8::is_ascii_alphabetic),
            last_digit: last(u8::is_ascii_digit),
            last_undotted: last(|b| !(b.is_ascii_alphanumeric() || *b == b'_' || *b == b'.')),
        }
    }

    /// Whether the value from `from` to the run's end is masked under
    /// `name`. A value in credential context is masked when token-shaped:
    /// at least 20 characters mixing ASCII letters and digits, not a call
    /// (`base64.b64encode(...)`) and not a dotted path of identifiers
    /// (`process.env.AUTH0_CLIENT_SECRET`). Passphrases without digits
    /// escape.
    fn masked(&self, s: &[u8], from: usize, name: Name) -> bool {
        let end = self.value_end;
        if from >= end {
            return false;
        }
        let holds = |last: Option<usize>| last.is_some_and(|k| k >= from);
        // Only a suffix free of other characters can be a dotted path, so
        // the full check runs at most once per run.
        let dotted = || !holds(self.last_undotted) && dotted_path(&s[from..end]);
        let token = || {
            end - from >= 20
                && holds(self.last_alpha)
                && holds(self.last_digit)
                && s.get(self.end) != Some(&b'(')
                && !dotted()
        };
        match name {
            Name::Credential => token(),
            Name::Pwd => !matches!(s[from], b'/' | b'~') && token(),
            // Any scheme's value is the credential; `$VAR` is a reference.
            Name::Authorization => end - from >= 12 && s[from] != b'$',
        }
    }
}

fn dotted_path(value: &[u8]) -> bool {
    let identifier = |seg: &[u8]| {
        seg.first()
            .is_some_and(|b| b.is_ascii_alphabetic() || *b == b'_')
            && seg.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'_')
    };
    value.contains(&b'.') && value.split(|&b| b == b'.').all(identifier)
}

pub(super) fn assignments(text: &str, out: &mut Vec<Span>) {
    let s = text.as_bytes();
    let mut values = Values::default();
    let mut i = 0;
    while i < s.len() {
        let b = s[i];
        if b != b'=' && b != b':' {
            i += 1;
            continue;
        }
        let sep_len = match (b, s.get(i + 1)) {
            // Comparisons and paths (`==`, `::`) are not assignments.
            (b'=', Some(b'=')) | (b':', Some(b':')) => {
                i += 2;
                continue;
            }
            (b'=', Some(b'>')) | (b':', Some(b'=')) => 2,
            _ => 1,
        };
        let comparison = i > 0 && matches!(s[i - 1], b'!' | b'<' | b'>' | b'=');
        // Every separator is judged, also inside a value that did not
        // qualify: an inner name can set other rules (`PWD=/tmp?api_key=`).
        if !comparison
            && let Some(name) = name_before(text, i)
            && let Some((start, end)) = values.after(s, i + sep_len, name)
        {
            push(out, start, end, Class::CredentialAssignment);
        }
        i += sep_len;
    }
}

/// The credential name, if any, written right before the separator at `sep`.
fn name_before(text: &str, sep: usize) -> Option<Name> {
    let s = text.as_bytes();
    let mut k = sep;
    // Makefile-style `?=`, `+=`, `|=`.
    if k > 0 && matches!(s[k - 1], b'?' | b'+' | b'|') {
        k -= 1;
    }
    // Pretty-printed JSON may break the line before the separator.
    while k > 0 && matches!(s[k - 1], b' ' | b'\t' | b'\r' | b'\n') {
        k -= 1;
    }
    // A subscript `["NAME"]`, a closing quote (maybe `\"` in escaped JSON)
    // or backtick, or a closing guillemet.
    if k > 0 && s[k - 1] == b']' {
        k -= 1;
    }
    if k > 0 && matches!(s[k - 1], b'"' | b'\'' | b'`') {
        k -= 1;
        if k > 0 && s[k - 1] == b'\\' {
            k -= 1;
        }
    } else if text[..k].ends_with('»') {
        k -= '»'.len_utf8();
    }
    let end = k;
    while k > 0 && is_key(s[k - 1]) {
        k -= 1;
    }
    // `\n` or `\t` written out in text: the escape letter is not the name's.
    if k > 0
        && s[k - 1] == b'\\'
        && k < end
        && matches!(s[k], b'n' | b't' | b'r' | b'f' | b'b' | b'v' | b'0')
    {
        k += 1;
    }
    if k < end {
        return credential_name(&text[k..end]);
    }
    let word: Vec<char> = text[..end]
        .chars()
        .rev()
        .take_while(|c| c.is_alphabetic())
        .collect();
    if word.is_empty() {
        return None;
    }
    let word: String = word.into_iter().rev().collect();
    credential_name(&word)
}

/// Values after separators, scanned left to right, reusing the last run.
#[derive(Default)]
struct Values {
    run: Option<Run>,
}

impl Values {
    /// The span of the value after a separator ending at `from`, if masked.
    fn after(&mut self, s: &[u8], from: usize, name: Name) -> Option<(usize, usize)> {
        let start = value_start(s, from, name);
        let reuse = self
            .run
            .as_ref()
            .is_some_and(|r| r.start <= start && start <= r.end);
        if !reuse {
            self.run = Some(Run::scan(s, start));
        }
        let run = self.run.as_ref()?;
        run.masked(s, start, name).then_some((start, run.value_end))
    }
}

/// The span of the value after a separator ending at `from`, if masked.
pub(super) fn value_after(s: &[u8], from: usize, name: Name) -> Option<(usize, usize)> {
    Values::default().after(s, from, name)
}

/// Where the value after a separator ending at `from` begins: past spaces
/// and line breaks, an opening quote, and a known authorization scheme.
fn value_start(s: &[u8], from: usize, name: Name) -> usize {
    let mut j = from;
    // The value may sit on the next line, as in pretty-printed JSON or YAML.
    while j < s.len() && matches!(s[j], b' ' | b'\t' | b'\r' | b'\n') {
        j += 1;
    }
    let quote = |b: u8| matches!(b, b'"' | b'\'' | b'`');
    if j + 1 < s.len() && s[j] == b'\\' && quote(s[j + 1]) {
        j += 2;
    } else if j < s.len() && quote(s[j]) {
        j += 1;
    } else if s[j..].starts_with("«".as_bytes()) {
        j += "«".len();
    }
    if name == Name::Authorization {
        // A known scheme stays readable; the line may fold after it. Any
        // other word is the credential itself.
        let word = s[j..]
            .iter()
            .take_while(|b| b.is_ascii_alphabetic())
            .count();
        let scheme = std::str::from_utf8(&s[j..j + word])
            .is_ok_and(|w| AUTH_SCHEMES.iter().any(|k| k.eq_ignore_ascii_case(w)));
        if scheme && s.get(j + word).is_some_and(u8::is_ascii_whitespace) {
            j += word;
            while j < s.len() && s[j].is_ascii_whitespace() {
                j += 1;
            }
        }
    }
    j
}
