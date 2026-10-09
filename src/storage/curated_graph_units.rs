//! A plan unit's evidence must name both the unit and its registry project.
use super::*;

type UnitMemories = BTreeMap<(i64, String), Memories>;
type UnitActivity = BTreeMap<(i64, String), (u32, Option<i64>)>;

fn requested<'a>(
    data: &data::Data,
    units: &'a [(i64, String)],
) -> BTreeMap<(i64, String), BTreeSet<&'a str>> {
    let mut requested: BTreeMap<(i64, String), BTreeSet<&str>> = BTreeMap::new();
    for (id, unit) in units {
        let Some(project) = data.canonical.get(id).copied() else {
            continue;
        };
        let name = canonical_name(unit);
        if !name.is_empty() {
            requested.entry((project, name)).or_default().insert(unit);
        }
    }
    requested
}

fn matched(data: &data::Data, units: &[(i64, String)]) -> UnitMemories {
    let mut projects: BTreeMap<i64, Memories> = BTreeMap::new();
    let mut names: BTreeMap<String, Memories> = BTreeMap::new();
    for entity in &data.entities {
        // An ambiguous registry identity is still held by the registry.
        // Its name cannot supply guessed plan-unit evidence either.
        if entity.archived.is_some() || data.owners.get(&entity.id).is_some_and(BTreeSet::is_empty)
        {
            continue;
        }
        let memories = data.memories.get(&entity.id).cloned().unwrap_or_default();
        for id in data.owners.get(&entity.id).into_iter().flatten() {
            projects.entry(*id).or_default().extend(memories.clone());
        }
        names
            .entry(canonical_name(&entity.name))
            .or_default()
            .extend(memories);
    }
    requested(data, units)
        .into_iter()
        .filter_map(|((project, name), raw_ids)| {
            // Entity canonicalization is lossy (prefixes, case and length).
            // Exact IDs may deduplicate through merges; distinct IDs sharing
            // a canonical name cannot borrow one another's evidence.
            if raw_ids.len() != 1 {
                return None;
            }
            let raw_id = raw_ids.into_iter().next()?;
            let (Some(owned), Some(named)) = (projects.get(&project), names.get(&name)) else {
                return None;
            };
            let memories: Memories = named
                .iter()
                .filter(|(id, _)| owned.contains_key(*id))
                .map(|(id, at)| (id.clone(), *at))
                .collect();
            (!memories.is_empty()).then_some(((project, raw_id.to_string()), memories))
        })
        .collect()
}

impl Storage {
    /// Evidence keyed by (canonical project, exact unit ID). Memory entities
    /// join by canonical name only when that name identifies one requested
    /// raw unit ID in the project. Missing/ambiguous evidence stays unknown.
    pub(crate) fn project_unit_memory_activity(
        &self,
        units: &[(i64, String)],
    ) -> Result<UnitActivity> {
        if units.is_empty() {
            return Ok(BTreeMap::new());
        }
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let tx = conn.transaction()?;
        Ok(matched(&data(&tx)?, units)
            .into_iter()
            .map(|(unit, memories)| (unit, (count(&memories), last(&memories))))
            .collect())
    }

    /// IDs for a unit contains-link, using the same evidence as its count.
    /// `units` is the complete prepared selection, not only the clicked unit:
    /// count and click must apply the same ambiguity check across unit IDs.
    /// The caller also validates the exact contains-link endpoints.
    pub(crate) fn curated_graph_unit_memories(
        &self,
        project: i64,
        unit: &str,
        units: &[(i64, String)],
        page: EdgePage,
    ) -> Result<Vec<String>> {
        if units.is_empty() {
            return Ok(Vec::new());
        }
        let mut conn = self
            .conn
            .lock()
            .map_err(|_| anyhow::anyhow!("store lock unavailable"))?;
        let tx = conn.transaction()?;
        let data = data::metadata(&tx)?;
        let Some(project) = data.canonical.get(&project).copied() else {
            return Ok(Vec::new());
        };
        let name = canonical_name(unit);
        let Some(raw_ids) = requested(&data, units).remove(&(project, name.clone())) else {
            return Ok(Vec::new());
        };
        if raw_ids.len() != 1 || !raw_ids.contains(unit) {
            return Ok(Vec::new());
        }
        let named: Vec<_> = data
            .entities
            .iter()
            .filter(|entity| {
                entity.archived.is_none()
                    && !data.owners.get(&entity.id).is_some_and(BTreeSet::is_empty)
                    && canonical_name(&entity.name) == name
            })
            .map(|entity| entity.id.as_str())
            .collect();
        let owned = edges::project_entities(&data, project);
        edges::page(&tx, &named, Some(&owned), page, 0)
    }
}
