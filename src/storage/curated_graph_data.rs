//! Read one consistent metadata snapshot, without memory text.
use super::*;

fn projects(conn: &Connection) -> Result<Vec<ProjectRow>> {
    let mut query = conn.prepare(
        "SELECT id,slug,name,parent_id,entity_id,state,slot,merged_into,prev_entity_type
        FROM projects ORDER BY slot,id",
    )?;
    Ok(query
        .query_map([], |r| {
            Ok(ProjectRow {
                id: r.get(0)?,
                slug: r.get(1)?,
                name: r.get(2)?,
                parent_id: r.get(3)?,
                entity_id: r.get(4)?,
                state: match r.get::<_, String>(5)?.as_str() {
                    "archived" => ProjectState::Archived,
                    "ignored" => ProjectState::Ignored,
                    _ => ProjectState::Active,
                },
                slot: r.get(6)?,
                merged_into: r.get(7)?,
                prev_entity_type: r.get(8)?,
            })
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn redirects(rows: &[ProjectRow]) -> BTreeMap<i64, i64> {
    let next: BTreeMap<_, _> = rows.iter().map(|row| (row.id, row.merged_into)).collect();
    rows.iter()
        .map(|row| {
            let mut id = row.id;
            let mut seen = BTreeSet::new();
            while seen.insert(id) {
                match next.get(&id).copied().flatten() {
                    Some(target) if next.contains_key(&target) => id = target,
                    _ => return (row.id, id),
                }
            }
            (row.id, row.id)
        })
        .collect()
}

pub(super) struct EntityRow {
    pub(super) id: String,
    pub(super) name: String,
    pub(super) kind: String,
    pub(super) archived: Option<i64>,
}
pub(super) struct Data {
    pub(super) rows: Vec<ProjectRow>,
    pub(super) canonical: BTreeMap<i64, i64>,
    pub(super) entities: Vec<EntityRow>,
    pub(super) owners: BTreeMap<String, BTreeSet<i64>>,
    pub(super) memories: BTreeMap<String, Memories>,
}
/// Identity and registry metadata only. Click paths never load the graph's
/// complete memory evidence; the background assembler opts into `data`.
pub(super) fn metadata(conn: &Connection) -> Result<Data> {
    let rows = projects(conn)?;
    let canonical = redirects(&rows);
    let mut names: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut bindings: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    for row in &rows {
        let id = canonical[&row.id];
        for name in [&row.name, &row.slug] {
            names.entry(canonical_name(name)).or_default().insert(id);
        }
        if let Some(entity) = &row.entity_id {
            bindings.entry(entity.clone()).or_default().insert(id);
        }
    }
    let mut aliases =
        conn.prepare("SELECT alias,project_id FROM project_aliases WHERE kind='name'")?;
    for row in aliases.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))? {
        let (name, id) = row?;
        if let Some(id) = canonical.get(&id) {
            names.entry(canonical_name(&name)).or_default().insert(*id);
        }
    }
    let mut query = conn.prepare(
        "SELECT e.id,e.name,e.entity_type,a.at FROM entities e
        LEFT JOIN entity_archive a ON a.entity_id=e.id ORDER BY e.id",
    )?;
    let entities = query
        .query_map([], |r| {
            Ok(EntityRow {
                id: r.get(0)?,
                name: r.get(1)?,
                kind: r.get(2)?,
                archived: r.get(3)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut owners = BTreeMap::new();
    for entity in &entities {
        // An explicit unique binding outranks ambiguous display names.
        // Shared bindings or names cannot make one mention name two projects.
        // Keep an empty entry for ambiguity: the registry still holds it, so
        // it is neither an outside-registry archive candidate nor a tech node.
        if let Some(ids) = bindings
            .get(&entity.id)
            .or_else(|| names.get(&canonical_name(&entity.name)))
        {
            owners.insert(
                entity.id.clone(),
                if ids.len() == 1 {
                    ids.clone()
                } else {
                    BTreeSet::new()
                },
            );
        }
    }
    Ok(Data {
        rows,
        canonical,
        entities,
        owners,
        memories: BTreeMap::new(),
    })
}

pub(super) fn data(conn: &Connection) -> Result<Data> {
    let mut data = metadata(conn)?;
    let mut query = conn.prepare(
        "SELECT me.entity_id,m.id,unixepoch(m.timestamp) FROM memory_entities me
        JOIN memories m ON m.id=me.memory_id WHERE m.superseded_by IS NULL
        AND m.memory_type != 'session_summary' ORDER BY me.entity_id,m.id",
    )?;
    for row in query.query_map([], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, Option<i64>>(2)?,
        ))
    })? {
        let (entity, id, at) = row?;
        if crate::redaction::state::check_identities([id.as_str()]).is_ok() {
            data.memories.entry(entity).or_default().insert(id, at);
        }
    }
    Ok(data)
}

pub(super) fn shelf(data: &Data, include_archived: bool) -> Vec<ArchivedCandidate> {
    data.entities
        .iter()
        .filter(|entity| {
            (entity.kind == "project"
                && !data.owners.contains_key(&entity.id)
                && (include_archived || entity.archived.is_none()))
                || (include_archived && entity.archived.is_some())
        })
        .filter_map(|entity| {
            crate::redaction::state::check_identities([entity.id.as_str()]).ok()?;
            let memories = data.memories.get(&entity.id).cloned().unwrap_or_default();
            Some(ArchivedCandidate {
                id: entity.id.clone(),
                name: crate::redaction::state::prepare_capped(&entity.name, 80).value,
                memory_count: count(&memories),
                last_memory: last(&memories),
                archived_at: entity.archived,
            })
        })
        .collect()
}
