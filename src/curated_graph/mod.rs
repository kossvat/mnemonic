//! Pure curated projection. Storage supplies evidence and durable slots;
//! this module never reads a file, allocates a slot, or changes a store.
mod layout;
pub(crate) use layout::render;

use serde::{Serialize, Serializer};
use std::collections::BTreeMap;

use crate::redaction::state::{check_identities, prepare_capped};
use crate::storage::curated_graph::{
    EntityKind, Evidence, GraphSlot, LinkKind, SlotRequest, unit_key, valid_key,
};
use crate::storage::projects::ProjectState;

pub(crate) const OVERVIEW_NODES: usize = 60;
pub(crate) const EXPANDED_NODES: usize = 150;
pub(crate) const EXPANDED_EDGES: usize = 375;
pub(crate) const OVERVIEW_BYTES: usize = 12_000;
pub(crate) const EXPANDED_BYTES: usize = 35_000;
const DAY: i64 = 86_400;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Kind {
    Project,
    Part,
    Unit,
    Technology,
    Person,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    Active,
    Quiet,
    Stale,
    Archived,
}

/// One unit from Reader.project_plans' all-open/last-closed selection, with
/// real memory evidence when available. `closed_at` supplies a display flag,
/// never memory activity or another filtering pass over the selected set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PlanUnit {
    pub(crate) project_id: i64,
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) closed_at: Option<i64>,
    pub(crate) last_memory: Option<i64>,
    pub(crate) memory_count: u32,
}

pub(crate) struct Input<'a> {
    pub(crate) evidence: &'a Evidence,
    pub(crate) units: &'a [PlanUnit],
    pub(crate) slots: &'a [GraphSlot],
    /// Separate owner and agent measures, as on the project's hours panel.
    /// Missing measurements stay unknown; other node kinds get no estimate.
    pub(crate) hours_30d: &'a BTreeMap<i64, Hours>,
    pub(crate) now: i64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Hours {
    pub(crate) owner_seconds: Option<u64>,
    pub(crate) agent_seconds: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Node {
    pub key: String,
    pub label: String,
    pub kind: Kind,
    pub x: i32,
    pub y: i32,
    pub hours_30d_tenths: [Option<u32>; 2],
    pub last_activity: Option<i64>,
    pub days_stale: Option<u32>,
    pub state: State,
    pub size_bucket: u8,
    pub closed: bool,
    pub project_slug: Option<String>,
}

/// Wire schema v1 node tuple: key, label, kind, x, y, [owner, agent] hours
/// in tenths, last activity epoch seconds, inactive days, state, agent-hour
/// size bucket, closed, project slug (only registry project/part nodes).
impl Serialize for Node {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        (
            &self.key,
            &self.label,
            self.kind as u8,
            self.x,
            self.y,
            self.hours_30d_tenths,
            self.last_activity,
            self.days_stale,
            self.state as u8,
            self.size_bucket,
            self.closed,
            &self.project_slug,
        )
            .serialize(serializer)
    }
}

/// Source and target are indices in `Payload.n`; kinds are contains=0,
/// uses=1, involves=2, relates=3. A count is distinct supporting memories.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Edge(pub u16, pub u16, pub u8, pub u32);

/// key, label, reason (stale registry=0, outside registry=1), memory count,
/// last memory/activity, inactive days, optional project slug. Archived items are omitted here;
/// the explicit archive history route supplies undo candidates.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Shelf(
    pub String,
    pub String,
    pub u8,
    pub u32,
    pub Option<i64>,
    pub Option<u32>,
    pub Option<String>,
);

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Payload {
    pub v: u8,
    pub n: Vec<Node>,
    pub e: Vec<Edge>,
    pub s: Vec<Shelf>,
    /// Eligible nodes, links, stale registry shelf rows, and outside-registry
    /// shelf rows omitted by scope or limits, in that order.
    pub o: [u32; 4],
}

pub struct Snapshot {
    pub payload: Payload,
    pub bytes: Vec<u8>,
    pub version: String,
    pub etag: String,
}

struct Candidate {
    node: Node,
    parent: Option<String>,
    slot: i64,
    memory_count: u32,
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Link {
    source: String,
    target: String,
    kind: u8,
    count: u32,
}

fn label(text: &str) -> String {
    let clean = prepare_capped(text, 64).value;
    let mut end = clean.len().min(64);
    while !clean.is_char_boundary(end) {
        end -= 1;
    }
    // A byte cut can expose a credential shape just like a character cut.
    prepare_capped(&clean[..end], 64).value
}

fn freshness(last: Option<i64>, now: i64, archived: bool) -> (State, Option<u32>) {
    let age = last.map(|at| now.saturating_sub(at).max(0));
    let state = if archived {
        State::Archived
    } else {
        match age {
            Some(age) if age <= 7 * DAY => State::Active,
            Some(age) if age < 30 * DAY => State::Quiet,
            _ => State::Stale,
        }
    };
    (
        state,
        age.map(|age| (age / DAY).min(i64::from(u32::MAX)) as u32),
    )
}

fn selected_units(units: &[PlanUnit]) -> Vec<&PlanUnit> {
    let mut selected = BTreeMap::new();
    for unit in units {
        selected.insert((unit.project_id, &unit.id), unit);
    }
    selected.into_values().collect()
}

/// Call from the existing background refresh writer, never from GET.
/// Storage owns collision handling and preserves all previous assignments.
pub(crate) fn slot_requests(evidence: &Evidence, units: &[PlanUnit]) -> Vec<SlotRequest> {
    let mut requests = BTreeMap::new();
    for project in &evidence.projects {
        if project.row.merged_into.is_some() {
            continue;
        }
        let parent_key = project.row.parent_id.map(|id| format!("project:{id}"));
        let key = format!("project:{}", project.row.id);
        requests.insert(
            key.clone(),
            SlotRequest {
                key,
                preferred_slot: parent_key.is_none().then_some(project.row.slot),
                parent_key,
            },
        );
    }
    for entity in &evidence.entities {
        let key = format!("entity:{}", entity.id);
        requests.insert(
            key.clone(),
            SlotRequest {
                key,
                preferred_slot: None,
                parent_key: Some(
                    match entity.kind {
                        EntityKind::Tech => "ring:tech",
                        EntityKind::Person => "ring:person",
                    }
                    .into(),
                ),
            },
        );
    }
    for unit in selected_units(units) {
        let Some(key) = unit_key(unit.project_id, &unit.id) else {
            continue;
        };
        requests.insert(
            key.clone(),
            SlotRequest {
                key,
                preferred_slot: None,
                parent_key: Some(format!("project:{}", unit.project_id)),
            },
        );
    }
    requests
        .into_values()
        .filter(|request| {
            valid_key(&request.key) && request.parent_key.as_deref().is_none_or(valid_key)
        })
        .collect()
}

pub(crate) fn contains(input: &Input<'_>, key: &str) -> bool {
    valid_key(key) && candidates(input).0.contains_key(key)
}

fn candidates(input: &Input<'_>) -> (BTreeMap<String, Candidate>, Vec<Link>, Vec<Shelf>) {
    let slots: BTreeMap<_, _> = input
        .slots
        .iter()
        .map(|slot| (slot.key.as_str(), slot))
        .collect();
    let mut nodes = BTreeMap::new();
    let mut shelf = BTreeMap::new();
    {
        let mut add = |key: String,
                       title: &str,
                       kind,
                       parent,
                       last,
                       archived,
                       hours: Hours,
                       closed,
                       count,
                       slug: Option<String>| {
            if !valid_key(&key) {
                return;
            }
            if slug
                .as_deref()
                .is_some_and(|slug| check_identities([slug]).is_err())
            {
                return;
            }
            let Some(slot) = slots.get(key.as_str()) else {
                return;
            };
            let Some((x, y)) = layout::position(&key, &slots) else {
                return;
            };
            let (state, days_stale) = freshness(last, input.now, archived);
            if state == State::Archived {
                return;
            }
            let node = Node {
                key: key.clone(),
                label: label(title),
                kind,
                x,
                y,
                hours_30d_tenths: [hours.owner_seconds, hours.agent_seconds].map(|seconds| {
                    seconds.map(|s| (s.saturating_add(180) / 360).min(u64::from(u32::MAX)) as u32)
                }),
                last_activity: last,
                days_stale,
                state,
                size_bucket: layout::size_bucket(hours.agent_seconds),
                closed,
                project_slug: slug,
            };
            if matches!(kind, Kind::Project | Kind::Part) && state == State::Stale {
                shelf.insert(
                    key.clone(),
                    Shelf(
                        key.clone(),
                        node.label.clone(),
                        0,
                        count,
                        last,
                        days_stale,
                        node.project_slug.clone(),
                    ),
                );
            }
            nodes.insert(
                key,
                Candidate {
                    node,
                    parent,
                    slot: slot.slot,
                    memory_count: count,
                },
            );
        };
        for project in &input.evidence.projects {
            let row = &project.row;
            if row.merged_into.is_some() || row.state == ProjectState::Ignored {
                continue;
            }
            add(
                format!("project:{}", row.id),
                &row.name,
                if row.parent_id.is_some() {
                    Kind::Part
                } else {
                    Kind::Project
                },
                row.parent_id.map(|id| format!("project:{id}")),
                project.last_memory.max(project.last_segment),
                row.state == ProjectState::Archived,
                input.hours_30d.get(&row.id).copied().unwrap_or_default(),
                false,
                project.memory_count,
                Some(row.slug.clone()),
            );
        }
        for entity in &input.evidence.entities {
            if entity.kind == EntityKind::Tech
                && (entity.memory_count < 3
                    || !input.evidence.links.iter().any(|link| {
                        link.target == format!("entity:{}", entity.id)
                            && link.kind == LinkKind::Uses
                            && link.memory_count > 0
                            && input.evidence.projects.iter().any(|project| {
                                project.row.state == ProjectState::Active
                                    && project.row.merged_into.is_none()
                                    && link.source == format!("project:{}", project.row.id)
                            })
                    }))
            {
                continue;
            }
            add(
                format!("entity:{}", entity.id),
                &entity.name,
                match entity.kind {
                    EntityKind::Tech => Kind::Technology,
                    EntityKind::Person => Kind::Person,
                },
                None,
                entity.last_memory,
                false,
                Hours::default(),
                false,
                entity.memory_count,
                None,
            );
        }
        for unit in selected_units(input.units) {
            let Some(key) = unit_key(unit.project_id, &unit.id) else {
                continue;
            };
            let project_key = format!("project:{}", unit.project_id);
            if !input.evidence.projects.iter().any(|project| {
                project.row.id == unit.project_id
                    && project.row.merged_into.is_none()
                    && project.row.state == ProjectState::Active
            }) {
                continue;
            }
            add(
                key,
                &unit.title,
                Kind::Unit,
                Some(project_key),
                unit.last_memory,
                false,
                Hours::default(),
                unit.closed_at.is_some(),
                unit.memory_count,
                None,
            );
        }
    }
    let mut links = BTreeMap::new();
    for link in &input.evidence.links {
        let kind = match link.kind {
            LinkKind::Contains => 0,
            LinkKind::Uses => 1,
            LinkKind::Involves => 2,
            LinkKind::Relates => 3,
        };
        let (mut source, mut target) = (link.source.clone(), link.target.clone());
        if kind == 3 && target < source {
            std::mem::swap(&mut source, &mut target);
        }
        if layout::valid_link(&nodes, &source, &target, kind, link.memory_count) {
            links
                .entry((source, target, kind))
                .and_modify(|count: &mut u32| *count = (*count).max(link.memory_count))
                .or_insert(link.memory_count);
        }
    }
    for (key, node) in &nodes {
        if matches!(node.node.kind, Kind::Unit | Kind::Part)
            && let Some(parent) = &node.parent
            && nodes.contains_key(parent)
        {
            links.entry((parent.clone(), key.clone(), 0)).or_insert(
                if node.node.kind == Kind::Unit {
                    node.memory_count
                } else {
                    0
                },
            );
        }
    }
    for item in &input.evidence.shelf {
        let key = format!("entity:{}", item.id);
        if item.archived_at.is_some() || !valid_key(&key) {
            continue;
        }
        shelf.insert(
            key.clone(),
            Shelf(
                key,
                label(&item.name),
                1,
                item.memory_count,
                item.last_memory,
                freshness(item.last_memory, input.now, false).1,
                None,
            ),
        );
    }
    (
        nodes,
        links
            .into_iter()
            .map(|((source, target, kind), count)| Link {
                source,
                target,
                kind,
                count,
            })
            .collect(),
        shelf.into_values().collect(),
    )
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod key_tests;
