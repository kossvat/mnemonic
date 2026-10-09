//! Assemble evidence counts after resolving registry identities.
use super::*;

#[path = "curated_graph_parents.rs"]
mod parents;

pub(super) fn evidence(
    conn: &Connection,
    tech: &[String],
    people: &[String],
) -> Result<(Evidence, Links)> {
    let data = data(conn)?;
    let mut project_memories: BTreeMap<i64, Memories> = BTreeMap::new();
    for entity in &data.entities {
        if entity.archived.is_some() {
            continue;
        }
        if let (Some(ids), Some(memories)) =
            (data.owners.get(&entity.id), data.memories.get(&entity.id))
        {
            for id in ids {
                project_memories
                    .entry(*id)
                    .or_default()
                    .extend(memories.clone());
            }
        }
    }
    let mut segments: BTreeMap<i64, i64> = BTreeMap::new();
    let mut query = conn.prepare(
        "SELECT project_id,MAX(unixepoch(last_at)) FROM work_segments
        WHERE project_id IS NOT NULL GROUP BY project_id",
    )?;
    for row in query.query_map([], |r| {
        Ok((r.get::<_, i64>(0)?, r.get::<_, Option<i64>>(1)?))
    })? {
        let (id, at) = row?;
        if let (Some(id), Some(at)) = (data.canonical.get(&id), at) {
            segments
                .entry(*id)
                .and_modify(|last| *last = (*last).max(at))
                .or_insert(at);
        }
    }
    let mut out = Evidence {
        shelf: shelf(&data, false),
        ..Default::default()
    };
    for row in &data.rows {
        if data.canonical[&row.id] != row.id
            || crate::redaction::state::check_identities([row.slug.as_str()]).is_err()
        {
            continue;
        }
        let mut row = row.clone();
        row.name = crate::redaction::state::prepare_capped(&row.name, 80).value;
        row.parent_id = row
            .parent_id
            .and_then(|id| data.canonical.get(&id).copied())
            .filter(|id| *id != row.id);
        let memories = project_memories.get(&row.id).cloned().unwrap_or_default();
        out.projects.push(ProjectEvidence {
            last_segment: segments.get(&row.id).copied(),
            memory_count: count(&memories),
            last_memory: last(&memories),
            row,
        });
    }
    parents::normalize(&mut out.projects);
    let tech: BTreeSet<_> = tech.iter().map(|name| canonical_name(name)).collect();
    let people: BTreeMap<_, _> = people
        .iter()
        .filter_map(|name| {
            let canonical = canonical_name(name);
            (!canonical.is_empty()
                && crate::redaction::state::check_identities([name.as_str(), canonical.as_str()])
                    .is_ok())
            .then(|| {
                (
                    canonical,
                    crate::redaction::state::prepare_capped(name, 80).value,
                )
            })
        })
        .collect();
    let mut links = Links::new();
    for entity in &data.entities {
        if entity.archived.is_some()
            || data.owners.contains_key(&entity.id)
            || crate::redaction::state::check_identities([entity.id.as_str()]).is_err()
        {
            continue;
        }
        let name = canonical_name(&entity.name);
        let kind = match entity.kind.as_str() {
            "tech" if tech.contains(&name) => EntityKind::Tech,
            _ => continue,
        };
        let memories = data.memories.get(&entity.id).cloned().unwrap_or_default();
        if kind == EntityKind::Tech && memories.len() < 3 {
            continue;
        }
        let mut joined = false;
        for (project, known) in &project_memories {
            let shared: BTreeSet<_> = known
                .keys()
                .filter(|id| memories.contains_key(*id))
                .cloned()
                .collect();
            if shared.is_empty() {
                continue;
            }
            joined = true;
            links.insert(
                (
                    key(*project),
                    format!("entity:{}", entity.id),
                    LinkKind::Uses,
                ),
                shared,
            );
        }
        if kind == EntityKind::Tech && !joined {
            continue;
        }
        out.entities.push(EntityEvidence {
            id: entity.id.clone(),
            name: crate::redaction::state::prepare_capped(&entity.name, 80).value,
            kind,
            memory_count: count(&memories),
            last_memory: last(&memories),
        });
    }
    // The configuration is sufficient for a person node. Its key is the
    // hash of its canonical name, independent of a graph entity's lifetime.
    for (name, label) in people {
        let mut memories = Memories::new();
        for entity in &data.entities {
            if entity.kind == "person"
                && entity.archived.is_none()
                && canonical_name(&entity.name) == name
            {
                memories.extend(data.memories.get(&entity.id).cloned().unwrap_or_default());
            }
        }
        let id = format!(
            "person:{}",
            git2::Oid::hash_object(git2::ObjectType::Blob, name.as_bytes())?
        );
        for (project, known) in &project_memories {
            let shared: BTreeSet<_> = known
                .keys()
                .filter(|id| memories.contains_key(*id))
                .cloned()
                .collect();
            if !shared.is_empty() {
                links.insert(
                    (key(*project), format!("entity:{id}"), LinkKind::Involves),
                    shared,
                );
            }
        }
        out.entities.push(EntityEvidence {
            id,
            name: label,
            kind: EntityKind::Person,
            memory_count: count(&memories),
            last_memory: last(&memories),
        });
    }
    out.entities.sort_by(|a, b| a.id.cmp(&b.id));
    // Resolve every registry relationship, including rows redirected by a
    // merge. Their historical containment is never deleted by the projection.
    let visible: BTreeSet<_> = out.projects.iter().map(|project| project.row.id).collect();
    for row in &data.rows {
        let child = data.canonical[&row.id];
        if let Some(parent) = row
            .parent_id
            .and_then(|id| data.canonical.get(&id).copied())
            && parent != child
            && visible.contains(&parent)
            && visible.contains(&child)
        {
            let own = project_memories.get(&child).cloned().unwrap_or_default();
            let shared = project_memories
                .get(&parent)
                .map(|memories| {
                    own.keys()
                        .filter(|id| memories.contains_key(*id))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            links.insert((key(parent), key(child), LinkKind::Contains), shared);
        }
    }
    for (index, project) in out.projects.iter().enumerate() {
        let own = project_memories
            .get(&project.row.id)
            .cloned()
            .unwrap_or_default();
        for other in out.projects.iter().skip(index + 1) {
            let shared: BTreeSet<_> = project_memories
                .get(&other.row.id)
                .map(|memories| {
                    own.keys()
                        .filter(|id| memories.contains_key(*id))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default();
            if shared.len() >= 2 {
                let mut pair = [key(project.row.id), key(other.row.id)];
                pair.sort();
                links.insert(
                    (pair[0].clone(), pair[1].clone(), LinkKind::Relates),
                    shared,
                );
            }
        }
    }
    out.links = links
        .iter()
        .map(|((source, target, kind), memories)| EvidenceLink {
            source: source.clone(),
            target: target.clone(),
            kind: *kind,
            memory_count: u32::try_from(memories.len()).unwrap_or(u32::MAX),
        })
        .collect();
    Ok((out, links))
}
