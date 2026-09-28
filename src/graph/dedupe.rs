//! Applying a dedupe plan: every variant of a group becomes the group's
//! canonical entity. The command line and the HTTP endpoint both build the
//! plan from stored names and apply it here.

use anyhow::Result;

use crate::redaction::state::is_refused;
use crate::storage::Storage;

/// What applying a plan did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Totals {
    pub merged: usize,
    pub renamed: usize,
    pub edges_redirected: usize,
    pub memory_links_redirected: usize,
    /// Entities left as they are: a name the redaction policy refuses,
    /// stored before it.
    pub refused: usize,
}

/// Apply `plan`, a list of (canonical name, stored variants). A refused
/// name stops its own entity only: the rest of the plan goes on.
pub fn apply<'a>(
    storage: &Storage,
    plan: impl IntoIterator<Item = (&'a str, &'a [String])>,
) -> Result<Totals> {
    let mut totals = Totals::default();
    for (canonical, variants) in plan {
        let canonical_exists = variants.iter().any(|v| v == canonical);
        if !canonical_exists && let Some(to_rename) = variants.first() {
            // Promote the first variant; the others then merge into it.
            match storage.rename_entity(to_rename, canonical) {
                Ok(true) => totals.renamed += 1,
                Ok(false) => {}
                // Without its canonical entity the group has nothing to
                // merge into.
                Err(e) if is_refused(&e) => {
                    totals.refused += 1;
                    continue;
                }
                Err(e) => return Err(e),
            }
        }
        for variant in variants {
            if variant == canonical {
                continue;
            }
            match storage.merge_entities(canonical, variant) {
                Ok(report) if report.alias_dropped => {
                    totals.merged += 1;
                    totals.edges_redirected += report.edges_redirected;
                    totals.memory_links_redirected += report.memory_links_redirected;
                }
                Ok(_) => {}
                Err(e) if is_refused(&e) => totals.refused += 1,
                Err(e) => return Err(e),
            }
        }
    }
    Ok(totals)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{Entity, EntityType};

    /// An entity stored before the policy under a name it refuses stays as
    /// it is, under that name; the rest of the plan is applied.
    #[test]
    fn redaction_state_dedupe_leaves_a_refused_entity_and_goes_on() {
        let value: String = "a1b2c3d4e5f6".chars().cycle().take(32).collect();
        let dir = crate::test_support::temp_dir("mnemonic-dedupe-");
        let storage = Storage::open(&dir.path().join("memory.db")).unwrap();
        for name in ["example-org", "example-org-co"] {
            storage
                .upsert_entity(&Entity {
                    name: name.into(),
                    entity_type: EntityType::Project,
                })
                .unwrap();
        }
        let legacy = format!("password={value}");
        storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO entities (id, name, entity_type, mention_count, first_seen, last_seen)
                 VALUES ('legacy', ?1, 'concept', 1, datetime('now'), datetime('now'))",
                [&legacy],
            )
            .unwrap();
        let canonical = format!("password-{value}");
        // A second one, whose canonical entity exists: refused at the merge.
        let (kept, other) = ("token-store".to_string(), format!("token={value}"));
        storage
            .upsert_entity(&Entity {
                name: kept.clone(),
                entity_type: EntityType::Concept,
            })
            .unwrap();
        storage
            .conn
            .lock()
            .unwrap()
            .execute(
                "INSERT INTO entities (id, name, entity_type, mention_count, first_seen, last_seen)
                 VALUES ('legacy2', ?1, 'concept', 1, datetime('now'), datetime('now'))",
                [&other],
            )
            .unwrap();
        assert!(crate::redaction::check_identity(&other).is_err());
        let plan = [
            (canonical.as_str(), vec![legacy.clone()]),
            (kept.as_str(), vec![kept.clone(), other.clone()]),
            (
                "example-org",
                vec!["example-org".to_string(), "example-org-co".to_string()],
            ),
        ];
        let totals = apply(&storage, plan.iter().map(|(c, v)| (*c, v.as_slice()))).unwrap();
        assert_eq!(
            totals,
            Totals {
                merged: 1,
                refused: 2,
                ..Totals::default()
            }
        );
        let names = storage.list_entity_names().unwrap();
        assert!(names.contains(&legacy) && !names.contains(&canonical));
        assert!(names.contains(&other) && names.contains(&kept));
        assert!(!names.contains(&"example-org-co".to_string()));
    }
}
