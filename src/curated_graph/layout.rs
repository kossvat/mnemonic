use super::{
    Candidate, EXPANDED_BYTES, EXPANDED_EDGES, EXPANDED_NODES, Edge, Input, Kind, Link,
    OVERVIEW_BYTES, OVERVIEW_NODES, Payload, Snapshot, State, candidates, valid_key,
};
use crate::storage::curated_graph::GraphSlot;
use anyhow::{Result, bail};
use std::collections::{BTreeMap, BTreeSet};

const GOLDEN_ANGLE: f64 = 2.399_963_229_728_653;

/// Integer canvas coordinates. Stored parent and slot are the only inputs;
/// invisible ancestors retain their slots and continue to anchor children.
pub(super) fn position(key: &str, slots: &BTreeMap<&str, &GraphSlot>) -> Option<(i32, i32)> {
    fn at(
        key: &str,
        slots: &BTreeMap<&str, &GraphSlot>,
        seen: &mut BTreeSet<String>,
    ) -> Option<(f64, f64)> {
        if seen.len() >= 128 || !seen.insert(key.into()) {
            return None;
        }
        let row = slots.get(key)?;
        let slot = u64::try_from(row.slot).ok()?;
        let angle = slot as f64 * GOLDEN_ANGLE;
        let (origin, radius, angle) = match row.parent_key.as_deref() {
            None => ((0.0, 0.0), 300.0 * (slot as f64).sqrt(), angle),
            Some("ring:tech") => ((0.0, 0.0), 5_000.0, angle),
            Some("ring:person") => ((0.0, 0.0), 6_500.0, angle),
            Some(parent) => (
                at(parent, slots, seen)?,
                120.0 + (slot / 12) as f64 * 48.0,
                -0.9 + (slot % 12) as f64 * 1.8 / 11.0,
            ),
        };
        Some((
            origin.0 + radius * angle.cos(),
            origin.1 + radius * angle.sin(),
        ))
    }
    let (x, y) = at(key, slots, &mut BTreeSet::new())?;
    let integer = |value: f64| {
        (value.is_finite() && value >= i32::MIN as f64 && value <= i32::MAX as f64)
            .then(|| value.round() as i32)
    };
    Some((integer(x)?, integer(y)?))
}

pub(super) fn size_bucket(seconds: Option<u64>) -> u8 {
    match seconds {
        None | Some(0..=3_599) => 0,
        Some(3_600..=28_799) => 1,
        Some(28_800..=143_999) => 2,
        Some(144_000..=431_999) => 3,
        Some(_) => 4,
    }
}

pub(super) fn valid_link(
    nodes: &BTreeMap<String, Candidate>,
    source: &str,
    target: &str,
    kind: u8,
    count: u32,
) -> bool {
    let (Some(source), Some(target)) = (nodes.get(source), nodes.get(target)) else {
        return false;
    };
    let registry = |kind| matches!(kind, Kind::Project | Kind::Part);
    if source.node.key == target.node.key || !registry(source.node.kind) {
        return false;
    }
    match kind {
        0 => {
            // Supplied registry links already follow canonical merges. A
            // former part can now name a root or a part of another parent;
            // preserving that link must not change either node's slot.
            registry(target.node.kind)
                || (target.node.kind == Kind::Unit
                    && target.parent.as_deref() == Some(&source.node.key))
        }
        1 => target.node.kind == Kind::Technology && count > 0,
        2 => target.node.kind == Kind::Person && count > 0,
        3 => registry(target.node.kind) && count >= 2,
        _ => false,
    }
}

pub(super) fn select(
    nodes: &BTreeMap<String, Candidate>,
    links: &[Link],
    expand: Option<&str>,
) -> Vec<String> {
    let mut rank = BTreeMap::new();
    if let Some(focus) = expand {
        rank.insert(focus.to_string(), 0u8);
        for link in links {
            let other = if link.source == focus {
                Some(&link.target)
            } else if link.target == focus {
                Some(&link.source)
            } else {
                None
            };
            if let Some(other) = other {
                rank.entry(other.clone()).or_insert(2);
            }
        }
        for (key, node) in nodes {
            if node.parent.as_deref() == Some(focus) {
                rank.entry(key.clone()).or_insert(2);
            }
        }
        for key in rank.keys().cloned().collect::<Vec<_>>() {
            let mut parent = nodes[&key].parent.as_deref();
            let mut seen = BTreeSet::new();
            while let Some(key) = parent {
                if !seen.insert(key) {
                    break;
                }
                let Some(node) = nodes.get(key) else {
                    break;
                };
                rank.entry(key.to_string())
                    .and_modify(|rank| *rank = (*rank).min(1))
                    .or_insert(1);
                parent = node.parent.as_deref();
            }
        }
    } else {
        for (key, node) in nodes {
            let mut at = Some(key.as_str());
            let mut seen = BTreeSet::new();
            let mut shown = true;
            while let Some(key) = at {
                if !seen.insert(key) {
                    shown = false;
                    break;
                }
                let Some(ancestor) = nodes.get(key) else {
                    shown = false;
                    break;
                };
                if matches!(ancestor.node.kind, Kind::Project | Kind::Part)
                    && ancestor.node.state == State::Stale
                {
                    shown = false;
                    break;
                }
                at = ancestor.parent.as_deref();
            }
            if shown {
                rank.insert(key.clone(), node.node.kind as u8);
            }
        }
        // A technology remains useful in overview only beside at least one
        // visible registry project; configured people may stand alone.
        let visible = rank.keys().cloned().collect::<BTreeSet<_>>();
        rank.retain(|key, _| {
            nodes[key].node.kind != Kind::Technology
                || links.iter().any(|link| {
                    link.kind == 1 && link.target == *key && visible.contains(&link.source)
                })
        });
    }
    let mut selected = rank.into_iter().collect::<Vec<_>>();
    selected.sort_by(|(a, ar), (b, br)| {
        (ar, nodes[a].node.kind, nodes[a].slot, a).cmp(&(br, nodes[b].node.kind, nodes[b].slot, b))
    });
    selected.truncate(if expand.is_some() {
        EXPANDED_NODES
    } else {
        OVERVIEW_NODES
    });
    selected.into_iter().map(|(key, _)| key).collect()
}

/// `expand` is one canonical node key. A focus adds its direct neighbors,
/// direct children and their ancestors. Unknown/archived focuses fail; the
/// caller can return 404. No wildcard or repeated expansion is accepted.
pub(crate) fn render(input: Input<'_>, expand: Option<&str>) -> Result<Snapshot> {
    let (nodes, links, mut shelf) = candidates(&input);
    if expand.is_some_and(|key| !valid_key(key) || !nodes.contains_key(key)) {
        bail!("unknown graph expansion");
    }
    let selected = select(&nodes, &links, expand);
    let total_nodes = nodes.len();
    let total_links = links.len();
    let total_shelf = [
        shelf.iter().filter(|row| row.2 == 0).count(),
        shelf.iter().filter(|row| row.2 == 1).count(),
    ];
    let index: BTreeMap<_, _> = selected
        .iter()
        .enumerate()
        .map(|(i, key)| (key.as_str(), i as u16))
        .collect();
    let mut edges: Vec<_> = links
        .iter()
        .filter_map(|link| {
            Some(Edge(
                *index.get(link.source.as_str())?,
                *index.get(link.target.as_str())?,
                link.kind,
                link.count,
            ))
        })
        .collect();
    edges.sort_by_key(|edge| (edge.2, edge.0, edge.1));
    edges.truncate(if expand.is_some() {
        EXPANDED_EDGES
    } else {
        150
    });
    shelf.sort_by(|a, b| (a.2, &a.0).cmp(&(b.2, &b.0)));
    let mut shelf_kept = [0; 2];
    shelf.retain(|row| {
        let count = &mut shelf_kept[usize::from(row.2)];
        *count += 1;
        *count <= 20
    });
    let mut payload = Payload {
        v: 1,
        n: selected.iter().map(|key| nodes[key].node.clone()).collect(),
        e: edges,
        s: shelf,
        o: [0; 4],
    };
    let budget = if expand.is_some() {
        EXPANDED_BYTES
    } else {
        OVERVIEW_BYTES
    };
    let bytes = loop {
        payload.o = [
            (total_nodes - payload.n.len()).min(u32::MAX as usize) as u32,
            (total_links - payload.e.len()).min(u32::MAX as usize) as u32,
            (total_shelf[0] - payload.s.iter().filter(|row| row.2 == 0).count())
                .min(u32::MAX as usize) as u32,
            (total_shelf[1] - payload.s.iter().filter(|row| row.2 == 1).count())
                .min(u32::MAX as usize) as u32,
        ];
        let bytes = serde_json::to_vec(&payload)?;
        if bytes.len() <= budget {
            break bytes;
        }
        if payload.e.pop().is_some() || payload.s.pop().is_some() {
            continue;
        }
        if payload.n.pop().is_none() {
            bail!("graph body budget unavailable");
        }
    };
    // Hashing a blob uses no repository or filesystem and writes no object.
    let version = git2::Oid::hash_object(git2::ObjectType::Blob, &bytes)?.to_string();
    let etag = format!("\"curated-v1-{version}\"");
    Ok(Snapshot {
        payload,
        bytes,
        version,
        etag,
    })
}
