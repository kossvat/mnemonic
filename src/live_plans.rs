//! Connect plan sweeps to the existing observation pass and requests.
use super::Live;
use crate::config::ProjectsConfig;
use crate::live::Session;
use crate::plans::{Checkout, Input, Plan, Reader, location::Location};
use crate::storage::{Storage, projects::ProjectRow};
use chrono::{DateTime, Utc};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Default)]
pub(super) struct Plans {
    source: Option<(Arc<Storage>, ProjectsConfig)>,
    reader: Reader,
    values: BTreeMap<i64, Plan>,
    inputs: BTreeMap<i64, Input>,
    locations: HashMap<String, Location>,
    last_sweep: Option<Instant>,
    #[cfg(test)]
    before_session: Option<Box<dyn FnOnce() + Send>>,
}

fn project_path(root: &Path, row: &ProjectRow, rows: &[ProjectRow]) -> Option<PathBuf> {
    let mut names = vec![row.name.as_str()];
    let mut parent = row.parent_id;
    let mut seen = HashSet::from([row.id]);
    while let Some(id) = parent {
        if !seen.insert(id) {
            return None;
        }
        let row = rows.iter().find(|row| row.id == id)?;
        names.push(&row.name);
        parent = row.parent_id;
    }
    let mut path = root.to_path_buf();
    for name in names.into_iter().rev() {
        let mut components = Path::new(name).components();
        if !matches!(components.next(), Some(std::path::Component::Normal(_)))
            || components.next().is_some()
        {
            return None;
        }
        path.push(name);
    }
    Some(path)
}

/// Resolve through the nearest existing directory inside the configured
/// workspace. A sparse main checkout may omit the project's whole suffix.
fn configured_project(main: &Path, workspace: &Path) -> Option<(PathBuf, PathBuf)> {
    let relative = main.strip_prefix(workspace).ok()?;
    if relative
        .components()
        .any(|part| !matches!(part, std::path::Component::Normal(_)))
    {
        return None;
    }
    let workspace = workspace.canonicalize().ok()?;
    let configured = workspace.join(relative);
    let mut ancestor = configured.as_path();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                // Broken links and links escaping the configured workspace
                // fail here; neither is treated as an absent sparse folder.
                let existing = ancestor.canonicalize().ok()?;
                if !existing.is_dir() || !existing.starts_with(&workspace) {
                    return None;
                }
                let resolved = existing.join(configured.strip_prefix(ancestor).ok()?);
                return Some((resolved, existing));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor
                    .parent()
                    .filter(|path| path.starts_with(&workspace))?;
            }
            Err(_) => return None,
        }
    }
}

/// A session may run at the repository root or inside its project, also
/// in a linked worktree outside the configured workspace. The project's
/// relative location is the same in those checkouts. The shared Git
/// directory, not a matching folder name, establishes repository identity.
fn checkout_of(main: &Path, workspace: &Path, cwd: &Path) -> Option<PathBuf> {
    if !cwd.is_absolute() {
        return None;
    }
    let cwd = cwd.canonicalize().ok()?;
    let (main, existing) = configured_project(main, workspace)?;
    match (
        git2::Repository::discover(&existing),
        git2::Repository::discover(&cwd),
    ) {
        (Ok(main_repo), Ok(repo)) => {
            let common = main_repo.commondir().canonicalize().ok()?;
            if common != repo.commondir().canonicalize().ok()? {
                return None;
            }
            let main_work = main_repo.workdir()?.canonicalize().ok()?;
            let work = repo.workdir()?.canonicalize().ok()?;
            let relative = main.strip_prefix(main_work).ok()?;
            let candidate = work.join(relative).canonicalize().ok()?;
            if !candidate.is_dir() || !candidate.starts_with(work) {
                return None;
            }
            let candidate_repo = git2::Repository::discover(&candidate).ok()?;
            (candidate_repo.commondir().canonicalize().ok()? == common).then_some(candidate)
        }
        (Err(_), Err(_)) => {
            // Without Git identity, only the configured project can be
            // selected. A similarly named folder elsewhere is no checkout.
            let workspace = workspace.canonicalize().ok()?;
            (main.is_dir()
                && main.starts_with(&workspace)
                && cwd.starts_with(&workspace)
                && (main.starts_with(&cwd) || cwd.starts_with(&main)))
            .then_some(main)
        }
        _ => None,
    }
}

impl Live {
    pub fn configure_plans(&self, storage: Arc<Storage>, config: ProjectsConfig) {
        self.plans
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .source = Some((storage, config));
        self.refresh_plans_at(Utc::now(), Instant::now(), true);
    }

    pub fn location(&self, stream: &str) -> Option<Location> {
        self.plans
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .locations
            .get(stream)
            .cloned()
    }

    /// A sweep must see the location together with the session that keeps
    /// it live. Publish both while holding the same lock as plan sweeps.
    pub fn observed_with_location(
        &self,
        stream: &str,
        session: Session,
        location: Option<Location>,
    ) {
        let mut plans = self.plans.lock().unwrap_or_else(|held| held.into_inner());
        if let Some(location) = location {
            plans.locations.insert(stream.into(), location);
        }
        #[cfg(test)]
        if let Some(before_session) = plans.before_session.take() {
            before_session();
        }
        self.settled(self.state.observed(stream, session, Utc::now()));
    }

    /// Prefix recovery adds metadata only to the matching still-live
    /// session. It cannot revive a forgotten session or replace metadata
    /// published by a newer observation while the prefix was read.
    pub fn recover_location(&self, stream: &str, expected: &Session, location: Location) -> bool {
        let mut plans = self.plans.lock().unwrap_or_else(|held| held.into_inner());
        if plans.locations.contains_key(stream)
            || !self
                .state
                .sessions(Utc::now())
                .iter()
                .any(|(known, session)| {
                    known == stream
                        && session.agent == expected.agent
                        && session.program == expected.program
                        && session.project == expected.project
                        && session.last_kind == expected.last_kind
                        && session.last_at.timestamp() == expected.last_at.timestamp()
                        && session.started_at.timestamp() == expected.started_at.timestamp()
                })
        {
            return false;
        }
        plans.locations.insert(stream.into(), location);
        true
    }

    pub(super) fn forget_with_location(&self, stream: &str) {
        let mut plans = self.plans.lock().unwrap_or_else(|held| held.into_inner());
        plans.locations.remove(stream);
        self.settled(self.state.forget(stream, Utc::now()));
    }

    pub fn refresh_plans(&self) {
        self.refresh_plans_at(Utc::now(), Instant::now(), false);
    }

    /// Injectable clocks hold the five-second cache without a test sleep.
    pub fn refresh_plans_at(&self, now: DateTime<Utc>, monotonic: Instant, force: bool) {
        let mut plans = self.plans.lock().unwrap_or_else(|held| held.into_inner());
        if !force
            && plans.last_sweep.is_some_and(|last| {
                monotonic.saturating_duration_since(last) < Duration::from_secs(5)
            })
        {
            return;
        }
        let Some((storage, config)) = plans.source.clone() else {
            return;
        };
        let Some(root) = config.root() else {
            return;
        };
        let Ok(rows) = storage.projects() else {
            return;
        };
        let sessions = self.state.sessions(now);
        let canonical: HashMap<i64, i64> = rows
            .iter()
            .filter_map(|row| {
                storage
                    .project_canonical(row.id)
                    .ok()
                    .map(|id| (row.id, id))
            })
            .collect();
        let streams: HashSet<&str> = sessions.iter().map(|(stream, _)| stream.as_str()).collect();
        plans
            .locations
            .retain(|stream, _| streams.contains(stream.as_str()));
        let mut values = BTreeMap::new();
        let mut inputs = BTreeMap::new();
        for row in &rows {
            if canonical.get(&row.id) != Some(&row.id) {
                continue;
            }
            let Some(main) = project_path(&root, row, &rows) else {
                continue;
            };
            let mut checkouts = Vec::new();
            for (stream, session) in &sessions {
                if session.project.and_then(|id| canonical.get(&id)).copied() != Some(row.id) {
                    continue;
                }
                let location = plans.locations.get(stream);
                let cwd = location.and_then(|location| location.cwd.as_deref());
                let checkout = cwd.and_then(|cwd| {
                    checkout_of(&main, config.workspace.as_deref().unwrap_or(&root), cwd)
                });
                if let Some(checkout) = checkout {
                    checkouts.push(Checkout {
                        root: checkout,
                        branch: location.and_then(|location| location.branch.clone()),
                        last_at: session.last_at,
                    });
                }
            }
            let input = Input {
                main,
                config: config.plans.get(&row.slug).cloned().unwrap_or_default(),
                checkouts,
            };
            values.insert(row.id, plans.reader.read(&input, now));
            inputs.insert(row.id, input);
        }
        plans.inputs = inputs;
        plans.last_sweep = Some(monotonic);
        let changed = plans.values != values;
        if !changed {
            return;
        }
        let removed: Vec<i64> = plans
            .values
            .keys()
            .filter(|id| !values.contains_key(id))
            .copied()
            .collect();
        plans.values = values.clone();
        // Keep publication ordered with sweeps: otherwise an older sweep
        // could publish its short lines after a newer one had stored them.
        for id in removed {
            self.set_plan(id, None);
        }
        for (id, plan) in values {
            let unit = plan
                .units
                .iter()
                .find(|unit| unit.current_task.is_some())
                .or_else(|| plan.units.first());
            self.set_plan(id, unit.map(|unit| unit.line()));
        }
        // A detail can change while the short live line stays the same.
        self.signal(true, monotonic);
    }

    pub fn project_plans(&self) -> BTreeMap<i64, Plan> {
        self.plans
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .values
            .clone()
    }

    pub fn project_plan_details(
        &self,
        project: i64,
    ) -> BTreeMap<String, crate::plans::gate::Detail> {
        let mut plans = self.plans.lock().unwrap_or_else(|held| held.into_inner());
        let Some((input, plan)) = plans
            .inputs
            .get(&project)
            .cloned()
            .zip(plans.values.get(&project).cloned())
        else {
            return BTreeMap::new();
        };
        plans.reader.details(&input, &plan)
    }
}

#[cfg(test)]
#[path = "live_plans_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "live_plans_checkout_tests.rs"]
mod checkout_tests;

#[cfg(test)]
#[path = "live_plans_publish_tests.rs"]
mod publish_tests;
