use super::{
    Checkout, Input, ParsedUnit, Plan, Provider, Unit, cache, closed, finish, gate, providers,
};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

#[path = "discovery_source.rs"]
mod source;

/// Parsed plans and receipts, held only in memory. The caller decides when
/// to sweep; every sweep stats known files and parses only changed files.
#[derive(Default)]
pub struct Reader {
    files: cache::Cache,
    gates: gate::Reader,
    selected: BTreeMap<(PathBuf, String), PathBuf>,
    #[cfg(test)]
    detail_scans: usize,
}

struct Source {
    provider: Provider,
    authority: Option<PathBuf>,
}

struct Candidate {
    unit: Unit,
    /// Archiving closes a unit without asserting that its gate passed.
    closed_at: Option<i64>,
}

fn missing(path: &Path) -> bool {
    path.symlink_metadata()
        .is_err_and(|error| error.kind() == std::io::ErrorKind::NotFound)
}

impl Reader {
    fn change_folders(&mut self, root: &Path) -> Vec<(PathBuf, String, Option<i64>)> {
        let Some(changes) = cache::within(root, Path::new("openspec/changes")) else {
            return Vec::new();
        };
        let mut folders = Vec::new();
        for folder in self.files.dirs(&changes) {
            let Some(name) = folder.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            if name == "archive" {
                for archived in self.files.dirs(&folder) {
                    let Some(name) = archived.file_name().and_then(|name| name.to_str()) else {
                        continue;
                    };
                    let date = name
                        .get(..10)
                        .and_then(|date| chrono::NaiveDate::parse_from_str(date, "%Y-%m-%d").ok());
                    let Some((date, id)) = date.zip(name.get(11..)) else {
                        continue;
                    };
                    if name.get(10..11) != Some("-") || id.is_empty() {
                        continue;
                    }
                    let closed_at = date.and_hms_opt(0, 0, 0).unwrap().and_utc().timestamp();
                    folders.push((archived.clone(), id.to_string(), Some(closed_at)));
                }
            } else {
                folders.push((folder.clone(), name.to_string(), None));
            }
        }
        let active: BTreeSet<_> = folders
            .iter()
            .filter(|(_, _, closed)| closed.is_none())
            .map(|(_, id, _)| id.clone())
            .collect();
        let (mut archived, mut open): (Vec<_>, Vec<_>) = folders
            .into_iter()
            .partition(|(_, _, closed)| closed.is_some());
        archived.sort_by(|a, b| (b.2, &b.1).cmp(&(a.2, &a.1)));
        // Only one archive can be shown. Find its existing source without
        // parsing every historical task list on every sweep.
        if let Some(latest) = archived.into_iter().find(|(folder, id, _)| {
            !active.contains(id)
                && ["tasks.md", "proposal.md"].iter().any(|name| {
                    folder
                        .strip_prefix(root)
                        .ok()
                        .and_then(|relative| cache::within(root, &relative.join(name)))
                        .is_some_and(|path| path.is_file())
                })
        }) {
            open.push(latest);
        }
        open
    }

    fn at(
        &mut self,
        root: &Path,
        source: &Source,
        input: &Input,
        now: DateTime<Utc>,
    ) -> Vec<Candidate> {
        if missing(root) {
            return Vec::new();
        }
        let mut parsed = Vec::new();
        let default = "plan";
        if let Some(authority) = source.effective_authority() {
            match cache::within(root, authority) {
                Some(path) => {
                    if let Some((units, modified)) = self.files.units(
                        &path,
                        source.provider,
                        default,
                        source.provider == Provider::Checkbox,
                        now,
                    ) {
                        parsed.extend(units.into_iter().map(|unit| (unit, modified, None)));
                    }
                }
                None => parsed.push((
                    ParsedUnit {
                        id: default.into(),
                        title: "Unreadable plan".into(),
                        error: Some("plan location refused".into()),
                        ..ParsedUnit::default()
                    },
                    0,
                    None,
                )),
            }
        } else {
            for (folder, id, closed_at) in self.change_folders(root) {
                if crate::redaction::state::check_identities([id.as_str()]).is_err() {
                    continue;
                }
                let Ok(relative) = folder.strip_prefix(root) else {
                    continue;
                };
                let relative = relative.join("tasks.md");
                let Some(tasks) = cache::within(root, &relative) else {
                    continue;
                };
                if let Some((units, modified)) =
                    self.files
                        .units(&tasks, Provider::Checkbox, &id, false, now)
                {
                    parsed.extend(units.into_iter().map(|unit| (unit, modified, closed_at)));
                } else if let Some(proposal) =
                    cache::within(root, &relative.with_file_name("proposal.md"))
                    && let Some((text, modified)) = self.files.text(&proposal)
                {
                    let title = text
                        .as_ref()
                        .ok()
                        .and_then(|text| text.lines().find_map(|line| line.strip_prefix("# ")))
                        .unwrap_or(&id);
                    parsed.push((
                        ParsedUnit {
                            id: id.clone(),
                            title: crate::redaction::state::prepare_capped(title, 160).value,
                            proposed: true,
                            error: text.err(),
                            ..ParsedUnit::default()
                        },
                        modified,
                        closed_at,
                    ));
                }
            }
        }
        let has_gate = input.config.gate_required.unwrap_or_else(|| {
            input.config.receipt.is_some()
                || ["private/verify_change.py", "bin/verify.sh", "target/checks"]
                    .iter()
                    .any(|name| {
                        cache::within(root, Path::new(name)).is_some_and(|path| path.exists())
                    })
        });
        let single = parsed
            .iter()
            .filter(|(_, _, closed)| closed.is_none())
            .count()
            == 1;
        let branch_units: Vec<Option<String>> = input
            .checkouts
            .iter()
            .map(|checkout| {
                checkout.branch.as_ref().and_then(|branch| {
                    parsed
                        .iter()
                        .filter(|(unit, _, closed)| {
                            closed.is_none() && super::task_in_branch(branch, &unit.id)
                        })
                        .max_by_key(|(unit, _, _)| unit.id.len())
                        .map(|(unit, _, _)| unit.id.clone())
                })
            })
            .collect();
        parsed
            .into_iter()
            .map(|(unit, modified, closed_at)| {
                let active: Vec<Checkout> = input
                    .checkouts
                    .iter()
                    .enumerate()
                    .filter(|(i, checkout)| {
                        closed_at.is_none()
                            && (single
                                || branch_units[*i].as_ref() == Some(&unit.id)
                                || checkout.branch.as_ref().is_some_and(|branch| {
                                    unit.tasks
                                        .iter()
                                        .any(|task| super::task_in_branch(branch, &task.id))
                                }))
                    })
                    .map(|(_, checkout)| checkout.clone())
                    .collect();
                let gate = self
                    .gates
                    .read(root, &unit.id, input.config.receipt.as_deref());
                Candidate {
                    unit: finish(
                        unit,
                        source.provider,
                        gate,
                        modified,
                        &active,
                        has_gate,
                        now,
                    ),
                    closed_at,
                }
            })
            .collect()
    }

    pub fn read(&mut self, input: &Input, now: DateTime<Utc>) -> Plan {
        self.selected.retain(|(main, _), _| main != &input.main);
        let source = match self.source(input) {
            Ok(source) => source,
            Err(error) => {
                return Plan {
                    units: vec![finish(
                        ParsedUnit {
                            id: "plan".into(),
                            title: "Unreadable plan".into(),
                            error: Some(error.into()),
                            ..ParsedUnit::default()
                        },
                        input.config.provider.unwrap_or(Provider::Checkbox),
                        gate::Gate::default(),
                        0,
                        &[],
                        false,
                        now,
                    )],
                    message: None,
                };
            }
        };
        let mut roots = BTreeSet::new();
        if source.provider == Provider::Registry || !input.checkouts.is_empty() {
            roots.insert(input.main.clone());
        } else {
            roots.insert(
                input
                    .config
                    .checkout
                    .as_ref()
                    .filter(|path| path.is_absolute())
                    .unwrap_or(&input.main)
                    .clone(),
            );
        }
        if source.provider != Provider::Registry {
            roots.extend(input.checkouts.iter().map(|checkout| checkout.root.clone()));
        }
        let roots: BTreeSet<PathBuf> = roots
            .into_iter()
            .map(|root| root.canonicalize().unwrap_or(root))
            .collect();
        let mut alternatives: BTreeMap<String, Vec<(PathBuf, Candidate)>> = BTreeMap::new();
        let mut per_root = Vec::new();
        for root in roots {
            // A reopened unit can coexist with older archives in one
            // checkout. It is still one checkout, and its active copy wins.
            let mut units: BTreeMap<String, Candidate> = BTreeMap::new();
            for candidate in self.at(&root, &source, input, now) {
                let priority = (candidate.closed_at.is_none(), candidate.closed_at);
                if units
                    .get(&candidate.unit.id)
                    .is_none_or(|known| priority > (known.closed_at.is_none(), known.closed_at))
                {
                    units.insert(candidate.unit.id.clone(), candidate);
                }
            }
            per_root.push(
                units
                    .values()
                    .map(|candidate| candidate.unit.id.clone())
                    .collect::<BTreeSet<_>>(),
            );
            for candidate in units.into_values() {
                alternatives
                    .entry(candidate.unit.id.clone())
                    .or_default()
                    .push((root.clone(), candidate));
            }
        }
        let mut candidates: Vec<Candidate> = alternatives
            .into_iter()
            .filter_map(|(id, mut versions)| {
                // Equal counts prefer the first sorted checkout, so bytes are
                // deterministic and opening the page does not reorder units.
                let selected = versions
                    .iter()
                    .enumerate()
                    .max_by_key(|(i, (_, candidate))| {
                        (
                            candidate.unit.done,
                            candidate.closed_at.is_none(),
                            candidate.closed_at,
                            std::cmp::Reverse(*i),
                        )
                    })?
                    .0;
                let (root, mut candidate) = versions.remove(selected);
                let unit = &mut candidate.unit;
                self.selected.insert((input.main.clone(), id.clone()), root);
                unit.checkouts = versions
                    .iter()
                    .filter(|(_, other)| {
                        let other = &other.unit;
                        (other.done, other.total, &other.counts)
                            != (unit.done, unit.total, &unit.counts)
                    })
                    .count()
                    + per_root.iter().filter(|ids| !ids.contains(&id)).count();
                Some(candidate)
            })
            .collect();
        // Open units plus the last completed one. Keep historical sections
        // out of the normal surface without treating them as missing data.
        let last_closed = candidates
            .iter()
            .filter(|candidate| closed(candidate.closed_at, candidate.unit.stage))
            .max_by_key(|candidate| {
                (
                    candidate.closed_at.unwrap_or(candidate.unit.as_of),
                    &candidate.unit.id,
                )
            })
            .map(|candidate| candidate.unit.id.clone());
        candidates.retain(|candidate| {
            !closed(candidate.closed_at, candidate.unit.stage)
                || Some(&candidate.unit.id) == last_closed.as_ref()
        });
        let units: Vec<Unit> = candidates
            .into_iter()
            .map(|mut candidate| {
                candidate.unit.closed = closed(candidate.closed_at, candidate.unit.stage);
                candidate.unit.closed_at = candidate.closed_at;
                candidate.unit
            })
            .collect();
        Plan {
            message: units.is_empty().then_some("no plan file"),
            units,
        }
    }

    /// More expensive checkout facts are asked only when the owner opens
    /// a project, never by a background sweep or the compact live route.
    pub fn details(&mut self, input: &Input, plan: &Plan) -> BTreeMap<String, gate::Detail> {
        let mut cache = gate::DetailCache::default();
        let details = plan
            .units
            .iter()
            .map(|unit| {
                let root = self
                    .selected
                    .get(&(input.main.clone(), unit.id.clone()))
                    .unwrap_or(&input.main);
                (
                    unit.id.clone(),
                    self.gates.detail_cached(
                        root,
                        &unit.id,
                        input.config.receipt.as_deref(),
                        &mut cache,
                    ),
                )
            })
            .collect();
        #[cfg(test)]
        {
            self.detail_scans = cache.scan_count();
        }
        details
    }

    /// Counts actual parser runs, for offline cost and cache assertions.
    #[cfg(test)]
    pub fn parsed_count(&self) -> usize {
        self.files.parsed
    }
}

#[cfg(test)]
#[path = "discovery_tests.rs"]
mod tests;
