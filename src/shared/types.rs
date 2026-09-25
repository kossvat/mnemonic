use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

/// Only explicitly published project knowledge belongs in this store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedRecord {
    pub project_id: String,
    pub key: String,
    pub title: String,
    pub content: String,
    pub source: String,
    pub revision: i64,
    pub updated_at: String,
    /// Who approved this text. Attribution supplied by the curator, the same
    /// standing as `source`: recorded, not authenticated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published_by: Option<String>,
    /// Set when the text was promoted from a reviewed observation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_observation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub origin_writer_id: Option<String>,
}

/// What the caller believes the key holds right now. Two curators editing the
/// same key must not silently overwrite each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expected {
    /// Last writer wins. Only for a single curator working alone.
    Any,
    /// The key must not exist yet.
    Absent,
    /// The key must still be at this revision.
    Revision(i64),
}

/// The key changed since the caller last looked. Nothing was written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    pub key: String,
    pub current_revision: Option<i64>,
}

impl std::fmt::Display for Conflict {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.current_revision {
            Some(revision) => write!(
                f,
                "revision conflict: key {} is now at revision {revision}; read it again",
                self.key
            ),
            None => write!(f, "revision conflict: key {} does not exist", self.key),
        }
    }
}

impl std::error::Error for Conflict {}

/// The revision and records describe one consistent database snapshot.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedContext {
    pub project_id: String,
    pub revision: i64,
    pub records: Vec<SharedRecord>,
    pub truncated: bool,
}

/// Agent input is quarantined: review never publishes or changes private memory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Observation {
    pub id: String,
    pub project_id: String,
    pub writer_id: String,
    pub request_id: String,
    pub title: String,
    pub content: String,
    pub source: String,
    pub status: String,
    pub created_at: String,
    /// The person this writer acts for. Absent on rows from schema version 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub principal_id: Option<String>,
    /// The published key this observation became, once a curator promoted it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub promoted_key: Option<String>,
}

pub(crate) fn validate_slug(value: &str, field: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b"_-".contains(&b)),
        "{field} must be 1..64 lowercase ASCII letters, digits, underscores or hyphens"
    );
    Ok(())
}

pub(crate) fn validate_identifier(value: &str, field: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"._:-".contains(&b)),
        "{field} must be 1..128 ASCII letters, digits, dots, colons, underscores or hyphens"
    );
    Ok(())
}

pub(crate) fn validate_text(
    value: &str,
    field: &str,
    max_bytes: usize,
    multiline: bool,
) -> Result<()> {
    ensure!(
        !value.trim().is_empty() && value.len() <= max_bytes,
        "{field} must be nonblank and at most {max_bytes} bytes"
    );
    ensure!(
        !value.chars().any(|c| {
            (c.is_control() && !(multiline && matches!(c, '\n' | '\r' | '\t')))
                || matches!(c, '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        }),
        "{field} contains unsupported control characters"
    );
    if let Some((index, hidden)) = first_hidden_char(value) {
        anyhow::bail!(
            "{field} contains the invisible character U+{:04X} at character {index}",
            hidden as u32
        );
    }
    Ok(())
}

/// Characters that render as nothing. A human reviewing text before it becomes
/// shared truth cannot see them, but a model reads them: the tag block and
/// variation selectors can carry a whole hidden instruction.
fn is_hidden(c: char) -> bool {
    matches!(c,
        '\u{00ad}' | '\u{061c}' | '\u{115f}' | '\u{1160}' | '\u{180e}'
        | '\u{200b}'..='\u{200f}' | '\u{2028}' | '\u{2029}' | '\u{2060}'..='\u{2064}'
        | '\u{3164}' | '\u{fe00}'..='\u{fe0f}' | '\u{feff}' | '\u{ffa0}'
        | '\u{fff9}'..='\u{fffb}' | '\u{e0000}'..='\u{e007f}' | '\u{e0100}'..='\u{e01ef}')
}

/// Characters that carry the Unicode Emoji property, the only ones a
/// variation selector or a zero-width joiner may legitimately attach to.
/// Whole blocks are too coarse in both directions: `\u{00a9}` sits below them
/// and `\u{3000}` IDEOGRAPHIC SPACE sits inside them, which would let an
/// invisible sequence hide on a run of blank characters.
fn is_pictograph(c: char) -> bool {
    matches!(c,
        '\u{00a9}' | '\u{00ae}' | '\u{203c}' | '\u{2049}' | '\u{2122}' | '\u{2139}'
        | '\u{2194}'..='\u{21aa}' | '\u{231a}'..='\u{231b}' | '\u{2328}' | '\u{23cf}'
        | '\u{23e9}'..='\u{23f3}' | '\u{23f8}'..='\u{23fa}' | '\u{24c2}'
        | '\u{25aa}'..='\u{25ab}' | '\u{25b6}' | '\u{25c0}' | '\u{25fb}'..='\u{25fe}'
        | '\u{2600}'..='\u{27bf}' | '\u{2934}'..='\u{2935}' | '\u{2b05}'..='\u{2b07}'
        | '\u{2b1b}'..='\u{2b1c}' | '\u{2b50}' | '\u{2b55}' | '\u{3030}' | '\u{303d}'
        | '\u{3297}' | '\u{3299}' | '\u{1f000}'..='\u{1faff}')
}

/// A keycap emoji is an ASCII base, U+FE0F, then the enclosing keycap.
fn is_keycap_base(c: char) -> bool {
    c.is_ascii_digit() || matches!(c, '#' | '*')
}

/// First hidden character that is not part of an ordinary emoji sequence.
/// U+FE0F after a symbol and U+200D between two symbols stay legal, so
/// `\u{26a0}\u{fe0f}` and family emoji pass; anywhere else they could encode
/// bits by presence and are refused.
fn first_hidden_char(value: &str) -> Option<(usize, char)> {
    let chars: Vec<char> = value.chars().collect();
    chars.iter().enumerate().find_map(|(index, &c)| {
        if !is_hidden(c) {
            return None;
        }
        let previous = index.checked_sub(1).map(|i| chars[i]);
        let after_symbol = previous.is_some_and(is_pictograph);
        let next = chars.get(index + 1).copied();
        let legal = match c {
            // Also the keycap form: `1` + U+FE0F + U+20E3 renders as one emoji.
            '\u{fe0f}' => {
                after_symbol || (previous.is_some_and(is_keycap_base) && next == Some('\u{20e3}'))
            }
            '\u{200d}' => {
                (after_symbol || previous == Some('\u{fe0f}')) && next.is_some_and(is_pictograph)
            }
            _ => false,
        };
        (!legal).then_some((index, c))
    })
}

pub(crate) fn validate_limit(limit: usize) -> Result<()> {
    ensure!(
        (1..=100).contains(&limit),
        "limit must be between 1 and 100"
    );
    Ok(())
}

pub(crate) fn validate_payload(title: &str, content: &str, source: &str) -> Result<()> {
    validate_text(title, "title", 240, false)?;
    validate_text(content, "content", 32_768, true)?;
    validate_text(source, "source", 2_048, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hidden_characters_are_refused_with_their_position() {
        for hidden in [
            "\u{200b}",
            "\u{200e}",
            "\u{2060}",
            "\u{feff}",
            "\u{00ad}",
            "\u{2028}",
            "\u{e0041}",
            "\u{e0101}",
            "\u{fe00}",
            "\u{3164}",
        ] {
            let text = format!("ship it{hidden} on Friday");
            let error = validate_text(&text, "content", 1024, true)
                .unwrap_err()
                .to_string();
            assert!(error.contains("at character 7"), "{error}");
        }
    }

    #[test]
    fn emoji_sequences_pass_but_stray_joiners_do_not() {
        for ok in [
            "\u{26a0}\u{fe0f} breaking change",
            "team \u{1f468}\u{200d}\u{1f469}\u{200d}\u{1f467} ok",
            "heart \u{2764}\u{fe0f}\u{200d}\u{1f525} ok",
            "Step 1\u{fe0f}\u{20e3}: deploy",
            "keys #\u{fe0f}\u{20e3} and *\u{fe0f}\u{20e3}",
            "\u{00a9}\u{fe0f} 2026 and \u{00ae}\u{fe0f} too",
            "\u{2764}\u{fe0f} and \u{2b50}\u{fe0f}",
            "обычный русский текст и plain ASCII",
        ] {
            validate_text(ok, "content", 1024, true).unwrap();
        }
        for bad in [
            "price\u{fe0f} is 42",
            "1\u{fe0f} no keycap follows",
            "a\u{fe0f}\u{20e3} is not a keycap",
            "\u{fe0f}\u{20e3} has no base",
            "a\u{200d}b",
            "\u{1f468}\u{200d}x",
            "\u{fe0f}leading",
            // Ideographic spaces are not emoji: a joiner on them is a way to
            // hide an invisible sequence in what looks like blank text.
            "\u{3000}\u{fe0f}\u{3000}\u{200d}\u{3000}",
            "\u{3000}\u{fe0f} spaced",
        ] {
            assert!(
                validate_text(bad, "content", 1024, true).is_err(),
                "{bad:?}"
            );
        }
    }
}
