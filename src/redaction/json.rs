//! Structured values (tags, metadata, payloads): walked field by field, so
//! a credential keeps its key's context and the result stays valid JSON.

use serde_json::{Map, Value};

use super::assign::{self, Name};
use super::{Counts, Redacted, RedactionError, check_identity, redact_text, redact_with_context};

/// Bounds on a walk. Past any of them the value is refused whole: a field
/// that was never looked at must not be stored as if it had been.
#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub max_depth: usize,
    pub max_nodes: usize,
    pub max_text_bytes: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            max_depth: 32,
            max_nodes: 100_000,
            max_text_bytes: 16 << 20,
        }
    }
}

impl Limits {
    /// The same limits less the room a written summary takes (one object
    /// of a few numbers), so a value preparation admits also passes the
    /// full limits at the write that follows.
    pub(super) fn for_preparation(self) -> Limits {
        Limits {
            max_depth: self.max_depth.saturating_sub(2),
            max_nodes: self.max_nodes.saturating_sub(16),
            max_text_bytes: self.max_text_bytes.saturating_sub(1024),
        }
    }
}

/// Prepare every string in `value`. Everything under a credential name
/// (as the policy names them), however deeply nested in arrays or objects,
/// is in credential context: a token-shaped value is masked whole. A value
/// under one of `structural_keys` identifies something and is refused, not
/// masked, when preparing it would change it; so is an object key.
pub fn redact_json(
    value: &Value,
    structural_keys: &[&str],
    limits: Limits,
) -> Result<Redacted<Value>, RedactionError> {
    let mut walk = Walk {
        structural_keys,
        limits,
        nodes: 0,
        text_bytes: 0,
        counts: Counts::default(),
        changed: false,
        report: false,
    };
    let value = walk.value(value, 0, None)?;
    Ok(Redacted {
        value,
        counts: walk.counts,
        changed: walk.changed,
    })
}

/// One JSON document in which no object has two members of the same name,
/// or nothing. The usual parser keeps the last of two and drops the first,
/// so what it hands over to be judged is not what the text holds: a text
/// that is judged as a document and kept as a text has to be read this way.
pub fn parse_strict(text: &str) -> Option<Value> {
    use serde::de::DeserializeSeed;
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let value = Strict.deserialize(&mut deserializer).ok()?;
    deserializer.end().ok()?;
    Some(value)
}

struct Strict;

impl<'de> serde::de::DeserializeSeed<'de> for Strict {
    type Value = Value;

    fn deserialize<D: serde::Deserializer<'de>>(self, deserializer: D) -> Result<Value, D::Error> {
        deserializer.deserialize_any(self)
    }
}

impl<'de> serde::de::Visitor<'de> for Strict {
    type Value = Value;

    fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("a JSON value")
    }

    fn visit_bool<E>(self, v: bool) -> Result<Value, E> {
        Ok(Value::Bool(v))
    }

    fn visit_i64<E>(self, v: i64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_u64<E>(self, v: u64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_f64<E>(self, v: f64) -> Result<Value, E> {
        Ok(Value::from(v))
    }

    fn visit_str<E>(self, v: &str) -> Result<Value, E> {
        Ok(Value::String(v.to_owned()))
    }

    fn visit_string<E>(self, v: String) -> Result<Value, E> {
        Ok(Value::String(v))
    }

    fn visit_unit<E>(self) -> Result<Value, E> {
        Ok(Value::Null)
    }

    fn visit_seq<A: serde::de::SeqAccess<'de>>(self, mut seq: A) -> Result<Value, A::Error> {
        let mut items = Vec::new();
        while let Some(item) = seq.next_element_seed(Strict)? {
            items.push(item);
        }
        Ok(Value::Array(items))
    }

    fn visit_map<A: serde::de::MapAccess<'de>>(self, mut map: A) -> Result<Value, A::Error> {
        let mut members = Map::new();
        while let Some(name) = map.next_key::<String>()? {
            let value = map.next_value_seed(Strict)?;
            if members.insert(name, value).is_some() {
                return Err(serde::de::Error::custom("an object names a member twice"));
            }
        }
        Ok(Value::Object(members))
    }
}

/// What preparing `value` would redact, counted for a report. The walk is
/// the one a write is prepared by, with one difference: an object key the
/// policy refuses is counted like a value and the walk goes on, where a
/// write stops. Past a limit there is no count: what was not looked at
/// cannot be reported as clean.
pub fn inspect_json(value: &Value, limits: Limits) -> Result<Counts, RedactionError> {
    let mut walk = Walk {
        structural_keys: &[],
        limits,
        nodes: 0,
        text_bytes: 0,
        counts: Counts::default(),
        changed: false,
        report: true,
    };
    walk.value(value, 0, None)?;
    Ok(walk.counts)
}

struct Walk<'a> {
    structural_keys: &'a [&'a str],
    limits: Limits,
    nodes: usize,
    text_bytes: usize,
    counts: Counts,
    changed: bool,
    /// Count what a write would refuse, and go on.
    report: bool,
}

impl Walk<'_> {
    /// Every node, of any kind and position, counts toward the limits.
    fn enter(&mut self, depth: usize) -> Result<(), RedactionError> {
        if depth > self.limits.max_depth {
            return Err(RedactionError::TooDeep);
        }
        self.nodes += 1;
        if self.nodes > self.limits.max_nodes {
            return Err(RedactionError::TooLarge);
        }
        Ok(())
    }

    fn charge(&mut self, bytes: usize) -> Result<(), RedactionError> {
        self.text_bytes = self.text_bytes.saturating_add(bytes);
        if self.text_bytes > self.limits.max_text_bytes {
            return Err(RedactionError::TooLarge);
        }
        Ok(())
    }

    /// `context`: the credential name of the nearest enclosing key.
    fn value(
        &mut self,
        value: &Value,
        depth: usize,
        context: Option<Name>,
    ) -> Result<Value, RedactionError> {
        self.enter(depth)?;
        Ok(match value {
            Value::String(text) => {
                self.charge(text.len())?;
                let out = match context {
                    Some(name) => self.credential(text, name),
                    None => self.text(text),
                };
                // A marker can be longer than what it replaces, and the
                // write's check walks the prepared text: charge the growth
                // so what preparation admits, the write admits.
                self.charge(out.len().saturating_sub(text.len()))?;
                Value::String(out)
            }
            Value::Array(items) => Value::Array(
                items
                    .iter()
                    .map(|item| self.value(item, depth + 1, context))
                    .collect::<Result<_, _>>()?,
            ),
            Value::Object(map) => {
                let mut out = Map::new();
                for (key, item) in map {
                    self.charge(key.len())?;
                    // Renaming a key could collide with another; refuse.
                    if self.report {
                        self.text(key);
                    } else {
                        check_identity(key)?;
                    }
                    let name = assign::credential_name(key).or(context);
                    let prepared = self.value(item, depth + 1, name)?;
                    // Structural fields identify something: refuse instead
                    // of storing a changed identity.
                    if self.structural_keys.contains(&key.as_str()) && prepared != *item {
                        return Err(RedactionError::SensitiveContent);
                    }
                    out.insert(key.clone(), prepared);
                }
                Value::Object(out)
            }
            other => other.clone(),
        })
    }

    fn text(&mut self, text: &str) -> String {
        let r = redact_text(text);
        self.counts.merge(&r.counts);
        self.changed |= r.changed;
        r.value
    }

    /// A string in credential context is itself the credential when
    /// token-shaped, even without a provider prefix or a `name=` in it;
    /// the same value rules as in text apply.
    fn credential(&mut self, text: &str, name: Name) -> String {
        let r = redact_with_context(text, |stripped| {
            // The whole string is the value: leading newlines do not hide it.
            let lead = stripped.len() - stripped.trim_start().len();
            assign::value_after(&stripped.as_bytes()[lead..], 0, name)
                .map(|(a, b)| (a + lead, b + lead))
        });
        self.counts.merge(&r.counts);
        self.changed |= r.changed;
        r.value
    }
}
