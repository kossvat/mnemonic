//! Credential recognition: one linear scan per shape, then overlapping
//! candidates merge. Every delimiter the shapes use is ASCII, so every span
//! boundary is a UTF-8 character boundary.

use super::Class;
use super::assign;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Span {
    pub start: usize,
    pub end: usize,
    pub class: Class,
}

/// Lower names a merged overlap: a whole key block, then a bearer token,
/// then a value in credential context, then a provider-shaped token, then
/// a JWT.
fn priority(class: Class) -> u8 {
    match class {
        Class::PrivateBlock | Class::PrivateKey => 0,
        Class::BearerToken => 1,
        Class::CredentialAssignment | Class::UrlPassword => 2,
        Class::ProviderToken => 3,
        Class::Jwt => 4,
    }
}

/// Non-overlapping credential spans of `text`, in text order.
pub(super) fn credential_spans(text: &str) -> Vec<Span> {
    let s = text.as_bytes();
    let mut found = Vec::new();
    private_keys(s, &mut found);
    assign::assignments(text, &mut found);
    url_passwords(s, &mut found);
    bearers(s, &mut found);
    let runs = token_runs(s);
    providers(s, &runs, &mut found);
    jwts(s, &runs, &mut found);
    resolve(found)
}

/// Overlapping candidates become one span covering all of them, counted
/// once under the highest-priority class. Dropping the loser of an overlap
/// instead would leave whatever part of it the winner did not cover.
pub(super) fn resolve(mut found: Vec<Span>) -> Vec<Span> {
    found.sort_by_key(|sp| sp.start);
    let mut merged: Vec<Span> = Vec::with_capacity(found.len());
    for span in found {
        match merged.last_mut() {
            Some(last) if span.start < last.end => {
                last.end = last.end.max(span.end);
                if priority(span.class) < priority(last.class) {
                    last.class = span.class;
                }
            }
            _ => merged.push(span),
        }
    }
    merged
}

pub(super) fn push(out: &mut Vec<Span>, start: usize, end: usize, class: Class) {
    if start < end {
        out.push(Span { start, end, class });
    }
}

fn find(hay: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if from >= hay.len() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

/// Characters of provider tokens and base64url: ASCII letters, digits, `_`, `-`.
fn is_tok(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Maximal runs of token characters. Anything else, including every
/// non-ASCII byte, is a boundary.
fn token_runs(s: &[u8]) -> Vec<(usize, usize)> {
    let mut runs = Vec::new();
    let mut i = 0;
    while i < s.len() {
        if is_tok(s[i]) {
            let start = i;
            while i < s.len() && is_tok(s[i]) {
                i += 1;
            }
            runs.push((start, i));
        } else {
            i += 1;
        }
    }
    runs
}

/// Where a credential may start inside a token run: at its start, right
/// after a `-` or `_` that glues it to a word (`tag-sk-...`), or right after
/// the letter of a written-out escape (`\nsk-...` in raw JSON).
fn starts(s: &[u8], (start, end): (usize, usize)) -> impl Iterator<Item = usize> + '_ {
    let escaped = start > 0
        && s[start - 1] == b'\\'
        && matches!(s[start], b'n' | b't' | b'r' | b'f' | b'b' | b'v' | b'0');
    (start..end).filter(move |&p| {
        p == start || matches!(s[p - 1], b'-' | b'_') || (escaped && p == start + 1)
    })
}

/// Length of the provider-shaped credential at `s[p..end]` (a token run's
/// tail), given the first `_` at or after `p` (`end` when none).
fn provider_len(
    s: &[u8],
    p: usize,
    end: usize,
    next_underscore: usize,
    shape: &RunShape,
) -> Option<usize> {
    let rest = &s[p..end];
    // A fixed-length shape ends at the run end or at a glue character.
    let fixed = |len: usize, ok: fn(&u8) -> bool| {
        (rest.len() >= len
            && (rest.len() == len || matches!(rest[len], b'-' | b'_'))
            && rest[..len].iter().all(ok))
        .then_some(len)
    };
    // Key families with hyphenated tails (OpenAI project, service-account
    // and admin keys, Anthropic, OpenRouter). A real key has a long segment
    // of letters and digits; a path component such as
    // `sk-ant-colony-sim` (Claude encodes a project's cwd into its
    // transcript folder name) has only words.
    for prefix in [
        &b"sk-proj-"[..],
        b"sk-ant-",
        b"sk-svcacct-",
        b"sk-admin-",
        b"sk-or-v1-",
    ] {
        if rest.starts_with(prefix) {
            let from = p + prefix.len();
            return (end - from >= 20 && shape.key_shaped_from(s, from, end)).then_some(rest.len());
        }
    }
    // Langfuse: sk-lf- and a UUID.
    if rest.starts_with(b"sk-lf-") {
        return fixed(42, |b| is_tok(*b)).filter(|_| uuid_shaped(&rest[6..42]));
    }
    // A bare sk- key is letters and digits; a hyphenated tail is a path
    // component such as `-work-sk-platform-api`.
    if rest.starts_with(b"sk-") {
        let tail = rest[3..]
            .iter()
            .take_while(|b| b.is_ascii_alphanumeric())
            .count();
        return (tail >= 20).then_some(3 + tail);
    }
    for prefix in [&b"github_pat_"[..], b"glpat-"] {
        if rest.starts_with(prefix) {
            return (rest.len() >= prefix.len() + 20).then_some(rest.len());
        }
    }
    // Classic GitHub tokens: 20+ letters and digits, so snake_case names
    // with a `ghs_` segment stay.
    for prefix in [&b"ghp_"[..], b"gho_", b"ghu_", b"ghs_", b"ghr_"] {
        if rest.starts_with(prefix) {
            let tail = rest[prefix.len()..]
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric())
                .count();
            return (tail >= 20).then_some(prefix.len() + tail);
        }
    }
    if rest.starts_with(b"npm_") {
        let tail = fixed(40, |b| b.is_ascii_alphanumeric() || *b == b'_')?;
        return (!rest[4..40].contains(&b'_')).then_some(tail);
    }
    let slack = rest.len() > 5
        && ((rest.starts_with(b"xox") && b"abceprs".contains(&rest[3]) && rest[4] == b'-')
            || rest.starts_with(b"xapp-"));
    if slack {
        // Letters, digits and hyphens only: the token ends at the first `_`.
        let len = next_underscore - p;
        return (len >= 25).then_some(len);
    }
    if rest.starts_with(b"AKIA") || rest.starts_with(b"ASIA") {
        let len = fixed(20, |b| b.is_ascii_uppercase() || b.is_ascii_digit())?;
        return Some(len);
    }
    if rest.starts_with(b"AIza") {
        return fixed(39, |b| is_tok(*b));
    }
    if rest.starts_with(b"dop_v1_") {
        return fixed(71, |b| is_tok(*b)).filter(|_| rest[7..71].iter().all(u8::is_ascii_hexdigit));
    }
    None
}

/// A segment of at least `min` characters mixing letters and digits: what
/// key material has and a word does not.
fn mixed(seg: &[u8], min: usize) -> bool {
    seg.len() >= min
        && seg.iter().any(u8::is_ascii_alphabetic)
        && seg.iter().any(u8::is_ascii_digit)
}

/// The `-`/`_`-delimited segments of one token run, judged once, so that
/// "does the tail from here read as key material" is answered in O(1) for
/// every candidate start (a run of repeated `sk-proj-` has thousands).
struct RunShape {
    /// Where each segment starts, ascending.
    starts: Vec<usize>,
    /// Per suffix of segments: any segment of 20 mixed characters.
    has20: Vec<bool>,
    /// Per suffix of segments: how many segments of 8 mixed characters.
    count8: Vec<u32>,
}

impl RunShape {
    fn of(s: &[u8], start: usize, end: usize) -> RunShape {
        let mut starts = Vec::new();
        let mut segs = Vec::new();
        let mut at = start;
        for k in start..=end {
            if k == end || matches!(s[k], b'-' | b'_') {
                starts.push(at);
                segs.push((mixed(&s[at..k], 20), mixed(&s[at..k], 8)));
                at = k + 1;
            }
        }
        let n = segs.len();
        let (mut has20, mut count8) = (vec![false; n + 1], vec![0u32; n + 1]);
        for i in (0..n).rev() {
            has20[i] = segs[i].0 || has20[i + 1];
            count8[i] = count8[i + 1] + u32::from(segs[i].1);
        }
        RunShape {
            starts,
            has20,
            count8,
        }
    }

    /// Whether the tail from `from` reads as key material: one segment of
    /// 20 mixed characters, or two of 8 (a base64url tail whose separators
    /// fall every few characters still has several; `sk-ant-colony-sim`
    /// has none).
    fn key_shaped_from(&self, s: &[u8], from: usize, end: usize) -> bool {
        match self.starts.binary_search(&from) {
            Ok(i) => self.has20[i] || self.count8[i] >= 2,
            // Inside a segment: judge what is left of it, then the rest.
            Err(i) => {
                let head_end = self.starts.get(i).map_or(end, |st| st - 1);
                let head = &s[from..head_end];
                mixed(head, 20) || self.has20[i] || u32::from(mixed(head, 8)) + self.count8[i] >= 2
            }
        }
    }
}

/// `xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx` in hexadecimal.
fn uuid_shaped(s: &[u8]) -> bool {
    s.len() == 36
        && s.iter().enumerate().all(|(i, &b)| match i {
            8 | 13 | 18 | 23 => b == b'-',
            _ => b.is_ascii_hexdigit(),
        })
}

fn providers(s: &[u8], runs: &[(usize, usize)], out: &mut Vec<Span>) {
    for (i, &(start, end)) in runs.iter().enumerate() {
        let shape = RunShape::of(s, start, end);
        // The first `_` at or after each candidate, advanced monotonically.
        let mut next_underscore: Option<usize> = None;
        // Past a match that ends inside the run, the rest may hold another
        // credential glued on with `_` or `-`.
        let mut resume = start;
        for p in starts(s, (start, end)) {
            if p < resume {
                continue;
            }
            let nu = match next_underscore {
                Some(nu) if nu >= p => nu,
                _ => (p..end).find(|&k| s[k] == b'_').unwrap_or(end),
            };
            next_underscore = Some(nu);
            if let Some(len) = provider_len(s, p, end, nu, &shape) {
                push(out, p, p + len, Class::ProviderToken);
                resume = p + len;
            }
        }
        // Telegram bot token: 8-12 digits, ':', then "AA" and 30+ more. The
        // digits may trail a word, as in an API URL's `bot123456789:AA...`.
        let digits = s[start..end]
            .iter()
            .rev()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if (8..=12).contains(&digits)
            && s.get(end) == Some(&b':')
            && let Some(&(next_start, next_end)) = runs.get(i + 1)
            && next_start == end + 1
            && next_end - next_start >= 32
            && s[next_start..].starts_with(b"AA")
        {
            push(out, end - digits, next_end, Class::ProviderToken);
        }
    }
}

/// Past this a run is not a JOSE header; also bounds the work per window.
const MAX_JWT_HEADER: usize = 4096;
/// Header starts tried per window: a real one is the first after its glue.
const MAX_JWT_TRIES: usize = 4;

fn jwts(s: &[u8], runs: &[(usize, usize)], out: &mut Vec<Span>) {
    for w in runs.windows(3) {
        let (a, b, c) = (w[0], w[1], w[2]);
        if next_part(s, a.1) != Some(b.0) || next_part(s, b.1) != Some(c.0) || c.1 - c.0 < 16 {
            continue;
        }
        if json_object(&s[b.0..b.1]).is_none() {
            continue;
        }
        // Any JSON object serialization, leading whitespace included; the
        // payload check above already makes this rare in ordinary text.
        let header = starts(s, a)
            .filter(|&p| a.1 - p <= MAX_JWT_HEADER)
            .take(MAX_JWT_TRIES)
            .find(|&p| json_object(&s[p..a.1]).is_some_and(|h| h.get("alg").is_some()));
        if let Some(p) = header {
            let padding = s[c.1..].iter().take(2).take_while(|&&b| b == b'=').count();
            push(out, p, c.1 + padding, Class::Jwt);
        }
    }
}

/// The start of the next JWT part after one ending at `end`: an optional
/// `=` or `==` of padding, then `.`.
fn next_part(s: &[u8], end: usize) -> Option<usize> {
    let padding = s[end..].iter().take(2).take_while(|&&b| b == b'=').count();
    (s.get(end + padding) == Some(&b'.')).then_some(end + padding + 1)
}

fn json_object(part: &[u8]) -> Option<serde_json::Value> {
    decode_b64url(part)
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .filter(serde_json::Value::is_object)
}

fn decode_b64url(input: &[u8]) -> Option<Vec<u8>> {
    let value = |b: u8| match b {
        b'A'..=b'Z' => Some(b - b'A'),
        b'a'..=b'z' => Some(b - b'a' + 26),
        b'0'..=b'9' => Some(b - b'0' + 52),
        b'-' => Some(62),
        b'_' => Some(63),
        _ => None,
    };
    if input.len() % 4 == 1 {
        return None;
    }
    let mut out = Vec::with_capacity(input.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in input {
        acc = (acc << 6) | u32::from(value(b)?);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

/// PEM private-key blocks, from BEGIN to the END line with the same label
/// or, when there is none, to the end of the text.
fn private_keys(s: &[u8], out: &mut Vec<Span>) {
    const BEGIN: &[u8] = b"-----BEGIN ";
    const TAIL: &[u8] = b"PRIVATE KEY-----";
    // Real labels ("RSA ", "EC ", "OPENSSH ", "ENCRYPTED ") are short; the
    // bound keeps a line of many BEGINs from being rescanned.
    const MAX_LABEL: usize = 32;
    let is_label = |b: &u8| b.is_ascii_uppercase() || b.is_ascii_digit() || *b == b' ';
    let mut i = 0;
    while let Some(begin) = find(s, BEGIN, i) {
        let label = begin + BEGIN.len();
        let window = (label + MAX_LABEL + TAIL.len()).min(s.len());
        let tail = find(&s[..window], TAIL, label).filter(|&t| s[label..t].iter().all(is_label));
        let Some(tail) = tail else {
            i = label;
            continue;
        };
        let footer = [&b"-----END "[..], &s[label..tail], TAIL].concat();
        let end = find(s, &footer, tail + TAIL.len()).map_or(s.len(), |f| f + footer.len());
        push(out, begin, end, Class::PrivateKey);
        i = end;
    }
}

/// The password in `scheme://user:password@host`.
fn url_passwords(s: &[u8], out: &mut Vec<Span>) {
    let mut i = 0;
    while let Some(p) = find(s, b"://", i) {
        i = p + 3;
        let mut j = p;
        while j > 0 && (s[j - 1].is_ascii_alphanumeric() || b"+.-".contains(&s[j - 1])) {
            j -= 1;
        }
        // The scheme starts at its first letter (`:-postgres://`, `1.redis://`).
        if !s[j..p].iter().any(u8::is_ascii_alphabetic) {
            continue;
        }
        // Userinfo may hold RFC 3986 sub-delimiters like `(`, `)` and `'`,
        // except the one that encloses the URL (`'http://...'`, `(...)`).
        let closer = match j.checked_sub(1).map(|k| s[k]) {
            Some(q @ (b'\'' | b'"' | b'`')) => Some(q),
            Some(b'(') => Some(b')'),
            _ => None,
        };
        let stop = |b: u8| b.is_ascii_whitespace() || b"/?#,\"`<>[]{}".contains(&b);
        // Parentheses balance: in `[db](postgres://u:p(1)x@h)` only the
        // outer `)` ends the URL.
        let mut depth = 0usize;
        let mut k = i;
        while k < s.len() && !stop(s[k]) {
            match (closer, s[k]) {
                (Some(b')'), b'(') => depth += 1,
                (Some(b')'), b')') if depth == 0 => break,
                (Some(b')'), b')') => depth -= 1,
                // An escaped quote belongs to the password (`'...pa\'ss@...'`).
                (Some(q), b) if q == b && q != b')' && s[k - 1] != b'\\' => break,
                _ => {}
            }
            k += 1;
        }
        let authority_end = k;
        let Some(at) = (i..authority_end).rev().find(|&k| s[k] == b'@') else {
            i = authority_end.max(i);
            continue;
        };
        if let Some(colon) = (i..at).find(|&k| s[k] == b':') {
            push(out, colon + 1, at, Class::UrlPassword);
        }
        i = authority_end.max(i);
    }
}

/// `Bearer <token>`, any case, the token alone: RFC 6750 b64token
/// characters plus `:`, after spaces, a newline or a `\` line continuation.
fn bearers(s: &[u8], out: &mut Vec<Span>) {
    const WORD: &[u8] = b"bearer";
    let is_bearer_char = |b: u8| b.is_ascii_alphanumeric() || b"-._~+/=:".contains(&b);
    let gap = |s: &[u8], j: usize| match s[j] {
        b' ' | b'\t' | b'\r' | b'\n' => 1,
        b'\\' if matches!(s.get(j + 1), Some(b'\n' | b'\r')) => 2,
        _ => 0,
    };
    let mut i = 0;
    while i + WORD.len() < s.len() {
        let hit = s[i..i + WORD.len()].eq_ignore_ascii_case(WORD)
            && (i == 0 || !s[i - 1].is_ascii_alphanumeric())
            && gap(s, i + WORD.len()) > 0;
        if !hit {
            i += 1;
            continue;
        }
        let mut j = i + WORD.len();
        while j < s.len() && gap(s, j) > 0 {
            j += gap(s, j);
        }
        let start = j;
        while j < s.len() && is_bearer_char(s[j]) {
            j += 1;
        }
        if j - start >= 20 {
            push(out, start, j, Class::BearerToken);
            i = j;
        } else {
            // A rejected short word may itself open the real one
            // (`Bearer Bearer <token>`).
            i = start.max(i + 1);
        }
    }
}
