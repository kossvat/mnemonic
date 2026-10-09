//! Registry-led graph evidence and reversible state. Reads never allocate slots.
use super::{
    Storage,
    projects::{ProjectRow, ProjectState},
};
use anyhow::{Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use std::collections::{BTreeMap, BTreeSet};

pub(super) const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS entity_archive (
    entity_id TEXT PRIMARY KEY, at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS graph_slots (
    node_key TEXT PRIMARY KEY, parent_key TEXT, slot INTEGER NOT NULL CHECK(slot >= 0)
);
CREATE UNIQUE INDEX IF NOT EXISTS graph_slots_namespace
    ON graph_slots(COALESCE(parent_key, ''), slot);
";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum EntityKind {
    Tech,
    Person,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LinkKind {
    Contains,
    Uses,
    Involves,
    Relates,
}
impl LinkKind {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Contains => "contains",
            Self::Uses => "uses",
            Self::Involves => "involves",
            Self::Relates => "relates",
        }
    }
}
#[derive(Debug, Clone)]
pub(crate) struct ProjectEvidence {
    pub row: ProjectRow,
    pub memory_count: u32,
    pub last_memory: Option<i64>,
    pub last_segment: Option<i64>,
}
#[derive(Debug, Clone)]
pub(crate) struct EntityEvidence {
    pub id: String,
    pub name: String,
    pub kind: EntityKind,
    pub memory_count: u32,
    pub last_memory: Option<i64>,
}
#[derive(Debug, Clone)]
pub(crate) struct EvidenceLink {
    pub source: String,
    pub target: String,
    pub kind: LinkKind,
    pub memory_count: u32,
}
#[derive(Debug, Clone)]
pub struct ArchivedCandidate {
    pub id: String,
    pub name: String,
    pub memory_count: u32,
    pub last_memory: Option<i64>,
    pub archived_at: Option<i64>,
}
#[derive(Debug, Clone, Default)]
pub(crate) struct Evidence {
    pub projects: Vec<ProjectEvidence>,
    pub entities: Vec<EntityEvidence>,
    pub links: Vec<EvidenceLink>,
    pub shelf: Vec<ArchivedCandidate>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct GraphSlot {
    pub key: String,
    pub parent_key: Option<String>,
    pub slot: i64,
}
#[derive(Debug, Clone)]
pub(crate) struct SlotRequest {
    pub key: String,
    pub parent_key: Option<String>,
    pub preferred_slot: Option<i64>,
}
#[derive(Debug, Clone, Copy)]
pub struct EdgePage {
    pub offset: usize,
    pub limit: usize,
}
#[derive(Debug, Clone)]
pub struct Term {
    pub name: String,
    pub memory_count: u32,
}

type Memories = BTreeMap<String, Option<i64>>;
type Links = BTreeMap<(String, String, LinkKind), BTreeSet<String>>;
fn key(id: i64) -> String {
    format!("project:{id}")
}
fn canonical_name(name: &str) -> String {
    crate::graph::canonical::canonicalize_name(name)
}
fn count(memories: &Memories) -> u32 {
    u32::try_from(memories.len()).unwrap_or(u32::MAX)
}
fn last(memories: &Memories) -> Option<i64> {
    memories.values().filter_map(|at| *at).max()
}
/// The raw plan identity is admitted before hashing and never normalized.
/// Keep existing short ASCII keys; encode other names in a reserved namespace.
pub(crate) fn unit_key(project: i64, raw: &str) -> Option<String> {
    if project <= 0
        || matches!(raw, "" | "." | "..")
        || crate::redaction::state::check_identities([raw]).is_err()
    {
        return None;
    }
    let key = if raw.len() <= 160
        && raw
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    {
        format!("unit:{project}:{raw}")
    } else {
        let bytes = [
            b"mnemonic-curated-graph-unit-v1\0".as_slice(),
            raw.as_bytes(),
        ]
        .concat();
        let digest = git2::Oid::hash_object(git2::ObjectType::Blob, &bytes).ok()?;
        format!("unit:{project}:h:{digest}")
    };
    valid_key(&key).then_some(key)
}

/// One identity domain for persisted slots, projection and HTTP node keys.
pub(crate) fn valid_key(value: &str) -> bool {
    if value.is_empty()
        || value.len() > 192
        || !value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b":-_.".contains(&byte))
        || crate::redaction::state::check_identities([value]).is_err()
    {
        return false;
    }
    let atom = |text: &str| {
        !text.is_empty()
            && text != "."
            && text != ".."
            && text
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"-_.".contains(&byte))
    };
    let project = |text: &str| {
        text.parse::<i64>()
            .is_ok_and(|id| id > 0 && id.to_string() == text)
    };
    if let Some(id) = value.strip_prefix("project:") {
        project(id)
    } else if let Some(id) = value.strip_prefix("entity:") {
        id.split(':').all(atom)
    } else if let Some(unit) = value.strip_prefix("unit:") {
        unit.split_once(':').is_some_and(|(id, unit)| {
            project(id)
                && if let Some(digest) = unit.strip_prefix("h:") {
                    digest.len() == 40
                        && digest
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                } else {
                    unit.len() <= 160 && atom(unit)
                }
        })
    } else {
        matches!(value, "ring:tech" | "ring:person")
    }
}

#[path = "curated_graph_data.rs"]
mod data;
#[path = "curated_graph_edges.rs"]
mod edges;
#[path = "curated_graph_evidence.rs"]
mod evidence;
#[path = "curated_graph_state.rs"]
mod state;
#[path = "curated_graph_units.rs"]
mod units;
use data::{data, shelf};
use evidence::evidence;

impl Storage {
    pub(crate) fn curated_graph_evidence(
        &self,
        tech: &[String],
        people: &[String],
    ) -> Result<Evidence> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let tx = conn.transaction()?;
        Ok(evidence(&tx, tech, people)?.0)
    }
    /// IDs only, sorted and capped to 100 per click; no memory text is loaded.
    pub(crate) fn curated_graph_edge_memories(
        &self,
        source: &str,
        target: &str,
        kind: LinkKind,
        tech: &[String],
        people: &[String],
        page: EdgePage,
    ) -> Result<Vec<String>> {
        ensure!(valid_key(source) && valid_key(target), "invalid graph key");
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let tx = conn.transaction()?;
        edges::edge(&tx, source, target, kind, tech, people, page)
    }
    pub fn entity_archive_shelf(&self, include_archived: bool) -> Result<Vec<ArchivedCandidate>> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let tx = conn.transaction()?;
        Ok(shelf(&data(&tx)?, include_archived))
    }
    pub fn curated_graph_top_terms(&self, project: i64, limit: usize) -> Result<Vec<Term>> {
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let tx = conn.transaction()?;
        let data = data(&tx)?;
        let Some(project) = data.canonical.get(&project) else {
            bail!("no such project")
        };
        let mut project_memories = BTreeSet::new();
        for entity in &data.entities {
            if entity.archived.is_none()
                && data
                    .owners
                    .get(&entity.id)
                    .is_some_and(|ids| ids.contains(project))
            {
                project_memories.extend(
                    data.memories
                        .get(&entity.id)
                        .into_iter()
                        .flat_map(|memories| memories.keys())
                        .cloned(),
                );
            }
        }
        let mut terms: Vec<_> = data
            .entities
            .iter()
            .filter(|entity| entity.kind == "concept")
            .filter_map(|entity| {
                let count = data
                    .memories
                    .get(&entity.id)?
                    .keys()
                    .filter(|id| project_memories.contains(*id))
                    .count();
                (count > 0).then(|| Term {
                    name: crate::redaction::state::prepare_capped(&entity.name, 80).value,
                    memory_count: u32::try_from(count).unwrap_or(u32::MAX),
                })
            })
            .collect();
        terms.sort_by(|a, b| {
            b.memory_count
                .cmp(&a.memory_count)
                .then_with(|| a.name.cmp(&b.name))
        });
        terms.truncate(limit.min(20));
        Ok(terms)
    }
}

#[cfg(test)]
#[path = "curated_graph_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "curated_graph_edge_tests.rs"]
mod edge_tests;
