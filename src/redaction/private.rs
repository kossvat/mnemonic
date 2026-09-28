//! Explicit `<private>` regions: removed before anything else looks at the
//! text, so no excerpt, classifier or embedding ever sees part of one.

use super::PRIVATE_MARKER;

const NAME: &[u8] = b"private";
/// What a closer without an opener becomes: visible, and unable to join
/// the text around it into a tag (a space could: `<private` + ` ` + `>`).
const ORPHAN: &str = "[/private]";
/// Past this an opener's attributes are not a tag; keeps the scan linear.
const MAX_TAG: usize = 512;

/// Replace each outermost `<private ...>...</private>` region with one
/// marker; nested regions go with their parent. An opener without a closer
/// takes the rest of the text. A closer without an opener becomes
/// `[/private]`, so the text around it cannot join into a new tag. Tags match in any
/// case. Returns the text and the number of regions.
pub(super) fn strip(text: &str) -> (String, u32) {
    let s = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let (mut depth, mut regions, mut copied) = (0usize, 0u32, 0usize);
    let mut i = 0;
    while i < s.len() {
        if s[i] != b'<' {
            i += 1;
            continue;
        }
        if let Some(end) = opener_end(s, i) {
            if depth == 0 {
                out.push_str(&text[copied..i]);
            }
            depth += 1;
            i = end;
        } else if let Some(end) = closer_end(s, i) {
            if depth == 0 {
                out.push_str(&text[copied..i]);
                out.push_str(ORPHAN);
            } else {
                depth -= 1;
                if depth == 0 {
                    out.push_str(PRIVATE_MARKER);
                    regions += 1;
                }
            }
            if depth == 0 {
                copied = end;
            }
            i = end;
        } else {
            i += 1;
        }
    }
    if depth > 0 {
        out.push_str(PRIVATE_MARKER);
        regions += 1;
    } else {
        out.push_str(&text[copied..]);
    }
    (out, regions)
}

fn named(s: &[u8], at: usize) -> bool {
    s.get(at..at + NAME.len())
        .is_some_and(|w| w.eq_ignore_ascii_case(NAME))
}

/// `<private>` or `<private name="value" ...>`: the offset just past its
/// `>`. Attributes must be `name=value` pairs (spaces around `=` allowed),
/// so placeholders such as `<private key>` stay text, and a `>` inside a
/// quoted value does not end the tag.
fn opener_end(s: &[u8], i: usize) -> Option<usize> {
    if !named(s, i + 1) {
        return None;
    }
    let mut j = i + 1 + NAME.len();
    match s.get(j) {
        Some(b'>') => return Some(j + 1),
        Some(b) if b.is_ascii_whitespace() => {}
        _ => return None,
    }
    let limit = (j + MAX_TAG).min(s.len());
    loop {
        while j < limit && s[j].is_ascii_whitespace() {
            j += 1;
        }
        if j >= limit {
            return None;
        }
        if s[j] == b'>' {
            return Some(j + 1);
        }
        let name = j;
        while j < limit && (s[j].is_ascii_alphanumeric() || s[j] == b'-' || s[j] == b'_') {
            j += 1;
        }
        while j < limit && s[j].is_ascii_whitespace() {
            j += 1;
        }
        if j == name || j >= limit || s[j] != b'=' {
            return None;
        }
        j += 1;
        while j < limit && s[j].is_ascii_whitespace() {
            j += 1;
        }
        match s.get(j) {
            Some(&quote) if quote == b'"' || quote == b'\'' => {
                j += 1;
                while j < limit && s[j] != quote {
                    j += 1;
                }
                if j >= limit {
                    return None;
                }
                j += 1;
            }
            _ => {
                let value = j;
                while j < limit && !s[j].is_ascii_whitespace() && s[j] != b'>' {
                    j += 1;
                }
                if j == value {
                    return None;
                }
            }
        }
    }
}

/// `</private>` with optional spaces before `>`: the offset just past it.
fn closer_end(s: &[u8], i: usize) -> Option<usize> {
    if s.get(i + 1) != Some(&b'/') || !named(s, i + 2) {
        return None;
    }
    let mut j = i + 2 + NAME.len();
    while j < s.len() && s[j].is_ascii_whitespace() {
        j += 1;
    }
    (s.get(j) == Some(&b'>')).then_some(j + 1)
}
