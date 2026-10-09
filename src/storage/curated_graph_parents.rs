//! Canonical containment can cycle after otherwise valid registry merges.
use super::*;

/// Layout ancestry needs a forest. Break one parent edge per cycle at the
/// lowest surviving registry ID, independent of query order. This changes
/// neither registry rows nor the separately supplied containment links.
/// Existing graph slots remain immutable; only new slot requests use this
/// ancestry, so a merge cannot move an already positioned project.
pub(super) fn normalize(projects: &mut [ProjectEvidence]) {
    let ids: BTreeSet<_> = projects.iter().map(|project| project.row.id).collect();
    let mut parents: BTreeMap<_, _> = projects
        .iter()
        .map(|project| {
            (
                project.row.id,
                project.row.parent_id.filter(|parent| ids.contains(parent)),
            )
        })
        .collect();
    let mut done = BTreeSet::new();
    for start in &ids {
        let mut chain = Vec::new();
        let mut offsets = BTreeMap::new();
        let mut at = Some(*start);
        while let Some(id) = at {
            if done.contains(&id) {
                break;
            }
            if let Some(offset) = offsets.get(&id).copied() {
                if let Some(anchor) = chain[offset..].iter().min() {
                    parents.insert(*anchor, None);
                }
                break;
            }
            offsets.insert(id, chain.len());
            chain.push(id);
            at = parents.get(&id).copied().flatten();
        }
        done.extend(chain);
    }
    for project in projects {
        project.row.parent_id = parents[&project.row.id];
    }
}
