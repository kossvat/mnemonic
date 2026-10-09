//! A click resolves metadata once and reads only its endpoint evidence.
use super::*;

// The explicit outer order starts from selected entity IDs and the existing
// entity index. The peer lookup uses the memory/entity primary key. Neither
// side scans unrelated memory links or loads titles/content into Rust.
const IDS: &str = "
SELECT DISTINCT m.id
FROM json_each(?1) selected
CROSS JOIN memory_entities me INDEXED BY idx_me_entity ON me.entity_id=selected.value
CROSS JOIN memories m ON m.id=me.memory_id
WHERE m.superseded_by IS NULL AND m.memory_type != 'session_summary'
AND (?2 IS NULL OR EXISTS (
    SELECT 1 FROM json_each(?2) other
    CROSS JOIN memory_entities peer
      ON peer.memory_id=m.id AND peer.entity_id=other.value
))
ORDER BY m.id";

/// Stream one sorted intersection. Identity admission precedes offset, and
/// at most 100 IDs are retained. A relates page also proves two safe IDs.
pub(super) fn page(
    conn: &Connection,
    selected: &[&str],
    other: Option<&[&str]>,
    page: EdgePage,
    minimum: usize,
) -> Result<Vec<String>> {
    let limit = page.limit.min(100);
    if limit == 0 || selected.is_empty() || other.is_some_and(<[_]>::is_empty) {
        return Ok(Vec::new());
    }
    let selected = serde_json::to_string(selected)?;
    let other = other.map(serde_json::to_string).transpose()?;
    let mut query = conn.prepare(IDS)?;
    let mut admitted = 0usize;
    let mut result = Vec::with_capacity(limit);
    #[cfg(test)]
    let mut fetched = 0;
    {
        let mut rows = query.query(params![selected, other])?;
        while let Some(row) = rows.next()? {
            #[cfg(test)]
            {
                fetched += 1;
            }
            let id: String = row.get(0)?;
            if crate::redaction::state::check_identities([id.as_str()]).is_err() {
                continue;
            }
            if admitted >= page.offset && result.len() < limit {
                result.push(id);
            }
            admitted = admitted.saturating_add(1);
            if admitted >= minimum && result.len() == limit {
                break;
            }
        }
    }
    #[cfg(test)]
    probe::record(&query, fetched);
    Ok(if admitted < minimum {
        Vec::new()
    } else {
        result
    })
}

pub(super) fn project_entities(data: &data::Data, project: i64) -> Vec<&str> {
    data.entities
        .iter()
        .filter(|entity| {
            entity.archived.is_none()
                && data
                    .owners
                    .get(&entity.id)
                    .is_some_and(|ids| ids.contains(&project))
        })
        .map(|entity| entity.id.as_str())
        .collect()
}

fn project(data: &data::Data, key: &str, listed: bool) -> Option<i64> {
    let id: i64 = key.strip_prefix("project:")?.parse().ok()?;
    if data.canonical.get(&id) != Some(&id) {
        return None;
    }
    let row = data.rows.iter().find(|row| row.id == id)?;
    // Contains/relates use the admitted registry rows. Uses/involves retain
    // the assembler's canonical project ownership rule.
    (!listed || crate::redaction::state::check_identities([row.slug.as_str()]).is_ok())
        .then_some(id)
}

pub(super) fn edge(
    conn: &Connection,
    source: &str,
    target: &str,
    kind: LinkKind,
    tech: &[String],
    people: &[String],
    requested: EdgePage,
) -> Result<Vec<String>> {
    if requested.limit == 0 || source == target {
        return Ok(Vec::new());
    }
    let data = data::metadata(conn)?;
    let registry = matches!(kind, LinkKind::Contains | LinkKind::Relates);
    let Some(source) = project(&data, source, registry) else {
        return Ok(Vec::new());
    };
    let owned = project_entities(&data, source);
    match kind {
        LinkKind::Contains | LinkKind::Relates => {
            let Some(target) = project(&data, target, true) else {
                return Ok(Vec::new());
            };
            if kind == LinkKind::Contains
                && !data.rows.iter().any(|row| {
                    data.canonical.get(&row.id) == Some(&target)
                        && row.parent_id.and_then(|id| data.canonical.get(&id)) == Some(&source)
                })
            {
                return Ok(Vec::new());
            }
            let other = project_entities(&data, target);
            page(
                conn,
                &owned,
                Some(&other),
                requested,
                if kind == LinkKind::Relates { 2 } else { 0 },
            )
        }
        LinkKind::Uses => {
            let Some(id) = target.strip_prefix("entity:") else {
                return Ok(Vec::new());
            };
            let Some(entity) = data.entities.iter().find(|entity| entity.id == id) else {
                return Ok(Vec::new());
            };
            if entity.archived.is_some()
                || entity.kind != "tech"
                || data.owners.contains_key(id)
                || crate::redaction::state::check_identities([id]).is_err()
                || !tech
                    .iter()
                    .any(|name| canonical_name(name) == canonical_name(&entity.name))
            {
                return Ok(Vec::new());
            }
            let selected = [id];
            if page(
                conn,
                &selected,
                None,
                EdgePage {
                    offset: 0,
                    limit: 3,
                },
                3,
            )?
            .len()
                != 3
            {
                return Ok(Vec::new());
            }
            page(conn, &selected, Some(&owned), requested, 1)
        }
        LinkKind::Involves => {
            for configured in people {
                let name = canonical_name(configured);
                if name.is_empty()
                    || crate::redaction::state::check_identities([
                        configured.as_str(),
                        name.as_str(),
                    ])
                    .is_err()
                {
                    continue;
                }
                let id = git2::Oid::hash_object(git2::ObjectType::Blob, name.as_bytes())?;
                if target != format!("entity:person:{id}") {
                    continue;
                }
                let selected: Vec<_> = data
                    .entities
                    .iter()
                    .filter(|entity| {
                        entity.archived.is_none()
                            && entity.kind == "person"
                            && canonical_name(&entity.name) == name
                    })
                    .map(|entity| entity.id.as_str())
                    .collect();
                return page(conn, &selected, Some(&owned), requested, 1);
            }
            Ok(Vec::new())
        }
    }
}

#[cfg(test)]
pub(super) mod probe {
    use rusqlite::{Statement, StatementStatus};
    use std::cell::Cell;

    #[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
    pub(crate) struct Work {
        pub(crate) queries: u32,
        pub(crate) memory_rows: u32,
        pub(crate) vm_steps: i64,
        pub(crate) fullscan_steps: i64,
    }
    thread_local! {
        static WORK: Cell<Work> = const { Cell::new(Work {
            queries: 0, memory_rows: 0, vm_steps: 0, fullscan_steps: 0,
        }) };
    }
    pub(super) fn record(query: &Statement<'_>, memory_rows: u32) {
        WORK.with(|work| {
            let mut total = work.get();
            total.queries += 1;
            total.memory_rows += memory_rows;
            total.vm_steps += i64::from(query.get_status(StatementStatus::VmStep));
            total.fullscan_steps += i64::from(query.get_status(StatementStatus::FullscanStep));
            work.set(total);
        });
    }
    pub(crate) fn measure<T>(action: impl FnOnce() -> T) -> (T, Work) {
        WORK.with(|work| work.set(Work::default()));
        let result = action();
        (result, WORK.with(Cell::take))
    }
}
