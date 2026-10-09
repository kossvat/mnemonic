//! Projects as the owner keeps them: the folders of one workspace
//! (live-projects, design 6).
//!
//! The configuration names a workspace and a folder inside it whose
//! children are projects. Every child folder becomes a row of the registry
//! under the name of its folder: that folder holds the owner's projects and
//! nothing else, and a project only just begun is a project too. A hidden
//! folder and a file are not projects. A folder inside a project that
//! carries the marker of a project (a repository, a plan folder, the
//! manifest of a build) is a project inside it and names its parent.
//!
//! Where a project lives is never stored: the map from the names of
//! folders to projects (`lookup`) is built here, in memory, from this
//! listing and from the names the configuration says a folder had before.
//! `resolver` holds the rules that say which project a session works on,
//! and `facts` reads what they need of a transcript record.
pub mod facts;
pub mod lookup;
pub mod resolver;
pub mod segments;

use anyhow::Result;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

pub use lookup::ProjectMap;

/// The map of the workspace as the last listing gave it, shared with the
/// passes that read it. `None` until the first listing, and for good when
/// no workspace is configured.
pub type SharedMap = std::sync::Arc<std::sync::RwLock<Option<std::sync::Arc<ProjectMap>>>>;

/// The map as it is now.
pub fn current(shared: &SharedMap) -> Option<std::sync::Arc<ProjectMap>> {
    shared.read().ok().and_then(|map| map.clone())
}

use crate::config::ProjectsConfig;
use crate::graph::canonical::canonicalize_name;
use crate::storage::Storage;
use crate::storage::projects::AliasKind;

/// What makes a folder inside a project a project of its own: a
/// repository, a plan folder, or the manifest of a build. A folder named
/// `specs` is not among them: the plan folder itself holds one, and so
/// does many a folder of documents, and neither is a project.
const MARKERS: &[&str] = &[
    ".git",
    "openspec",
    "Cargo.toml",
    "package.json",
    "Package.swift",
    "pyproject.toml",
    "go.mod",
];

/// What joins the key of a parent and of the project inside it.
const INSIDE: char = ':';

/// The key of a name: lower case, every run of other characters one dash.
pub fn slug_of(name: &str) -> String {
    let mut slug = String::with_capacity(name.len());
    for c in name.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            slug.push(c);
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    slug.trim_matches('-').to_string()
}

/// A project found in the workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub name: String,
    pub slug: String,
    pub path: PathBuf,
    /// The index in the listing of the project this one lies in.
    pub parent: Option<usize>,
}

/// What a listing of the workspace found, parents before what lies in
/// them, and how many folders it could not take: a name the identity
/// policy refuses, as it arrived or as its key, a name that is not text,
/// one with no key, and one whose key another folder already took.
#[derive(Debug, Default)]
pub struct Listing {
    pub found: Vec<Found>,
    pub refused: usize,
}

/// The visible child folders of `folder`, by name.
fn child_folders(folder: &Path) -> Vec<(Option<String>, PathBuf)> {
    let Ok(entries) = std::fs::read_dir(folder) else {
        return Vec::new();
    };
    let mut children: Vec<(Option<String>, PathBuf)> = entries
        .flatten()
        .map(|entry| (entry.file_name().into_string().ok(), entry.path()))
        .filter(|(name, path)| {
            !name.as_deref().is_some_and(|name| name.starts_with('.')) && path.is_dir()
        })
        .collect();
    children.sort();
    children
}

fn has_marker(folder: &Path) -> bool {
    MARKERS.iter().any(|marker| folder.join(marker).exists())
}

impl Listing {
    /// Take a folder as a project, or count it as refused.
    fn take(
        &mut self,
        taken: &mut HashSet<String>,
        name: Option<String>,
        path: PathBuf,
        parent: Option<usize>,
    ) -> Option<usize> {
        let slug = name.as_deref().map(slug_of).map(|slug| match parent {
            Some(parent) => format!("{}{INSIDE}{slug}", self.found[parent].slug),
            None => slug,
        });
        let (Some(name), Some(slug)) = (name, slug) else {
            self.refused += 1;
            return None;
        };
        let clean = crate::redaction::state::check_identities([name.as_str(), slug.as_str()]);
        if slug.is_empty()
            || slug.ends_with(INSIDE)
            || clean.is_err()
            || !taken.insert(slug.clone())
        {
            self.refused += 1;
            return None;
        }
        self.found.push(Found {
            name,
            slug,
            path,
            parent,
        });
        Some(self.found.len() - 1)
    }
}

/// List the projects under `root`.
pub fn list(root: &Path) -> Listing {
    let mut listing = Listing::default();
    let mut taken = HashSet::new();
    for (name, path) in child_folders(root) {
        let Some(parent) = listing.take(&mut taken, name, path.clone(), None) else {
            continue;
        };
        for (name, inner) in child_folders(&path) {
            if has_marker(&inner) {
                listing.take(&mut taken, name, inner, Some(parent));
            }
        }
    }
    listing
}

/// What one pass over the workspace did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct SyncReport {
    /// Rows of the registry the workspace holds now.
    pub projects: usize,
    /// Folders not taken (see [`Listing`]) and names the store refused.
    pub refused: usize,
    /// Aliases and former names of the configuration that name no project
    /// or could not be set.
    pub unset: usize,
}

/// Bring the registry up to the workspace: a row for every project found,
/// its entity made a project, the aliases of the configuration set, and
/// the map of folders built. `None` when no workspace is configured:
/// nothing is discovered and nothing written.
pub fn sync(
    storage: &Storage,
    config: &ProjectsConfig,
) -> Result<Option<(ProjectMap, SyncReport)>> {
    let Some(root) = config.root() else {
        return Ok(None);
    };
    let listing = list(&root);
    let mut report = SyncReport {
        refused: listing.refused,
        ..SyncReport::default()
    };
    let mut map = ProjectMap::new(&root);
    let mut ids: Vec<Option<i64>> = Vec::with_capacity(listing.found.len());
    for found in &listing.found {
        let parent = match found.parent {
            Some(parent) => match ids[parent] {
                Some(id) => Some(id),
                // Its parent was refused by the store: so is it.
                None => {
                    ids.push(None);
                    report.refused += 1;
                    continue;
                }
            },
            None => None,
        };
        match storage.project_upsert(&found.slug, &found.name, parent) {
            Ok(id) => {
                storage.project_link_entity(id, &canonicalize_name(&found.name))?;
                // The folder names its own row, and what is written of
                // it keeps that id: a merge moves no row, and undoing one
                // finds each under the id it was written with. To the
                // rules a row and the one it was merged into are one
                // project, through the merges the map carries.
                map.merge(id, storage.project_canonical(id)?);
                match found.parent {
                    Some(parent) => map.insert_inside(&listing.found[parent].name, &found.name, id),
                    None => map.insert(&found.name, id),
                }
                ids.push(Some(id));
                report.projects += 1;
            }
            Err(e) if crate::redaction::state::is_refused(&e) => {
                ids.push(None);
                report.refused += 1;
            }
            Err(e) => return Err(e),
        }
    }
    let named = config
        .aliases
        .iter()
        .map(|(alias, target)| (alias, target, AliasKind::Name));
    let former = config
        .former
        .iter()
        .map(|(alias, target)| (alias, target, AliasKind::Scope));
    let listed: HashSet<&str> = listing.found.iter().map(|f| f.slug.as_str()).collect();
    for (alias, target, kind) in named.chain(former) {
        match set_alias(storage, &listed, alias, target, kind)? {
            // A name the folder had before still names it in old records.
            Some(id) if kind == AliasKind::Scope => map.insert(alias, id),
            Some(_) => {}
            None => report.unset += 1,
        }
    }
    Ok(Some((map, report)))
}

/// The key a configuration names a project by: its folder, or the folder
/// of its parent and its own, each keyed as a folder is.
pub fn key_of(target: &str) -> String {
    target
        .split(INSIDE)
        .map(slug_of)
        .collect::<Vec<_>>()
        .join(&INSIDE.to_string())
}

/// Set one alias of the configuration. The id of the project it names
/// now, or `None` when it could not be set: a name the identity policy
/// refuses as it was written or as its key, a target that is no project,
/// a name another project carries.
///
/// A former name whose row is still in the registry, with its folder gone,
/// is a project that was renamed: the old row is merged into the target, a
/// redirect and nothing moved, and the name maps to the target.
fn set_alias(
    storage: &Storage,
    listed: &HashSet<&str>,
    alias: &str,
    target: &str,
    kind: AliasKind,
) -> Result<Option<i64>> {
    let key = slug_of(alias);
    // As they were written: making a key drops the syntax that makes a
    // text a credential and keeps the credential.
    let clean = crate::redaction::state::check_identities([alias, target, key.as_str()]);
    if key.is_empty() || clean.is_err() {
        return Ok(None);
    }
    let Some(project) = storage.project_by_key(&key_of(target))? else {
        return Ok(None);
    };
    let into = storage.project_canonical(project.id)?;
    if kind == AliasKind::Scope
        && let Some(old) = storage.project_by_key(&key)?
        && old.slug == key
        && old.id != project.id
    {
        if listed.contains(key.as_str()) {
            // Both folders are there: two projects, not one renamed.
            return Ok(None);
        }
        if storage.project_canonical(old.id)? != into {
            storage.project_merge(old.id, into)?;
        }
        return Ok(Some(into));
    }
    match storage.project_alias_set(&key, project.id, kind) {
        Ok(set) => Ok(set.then_some(into)),
        Err(e) if crate::redaction::state::is_refused(&e) => Ok(None),
        Err(e) => Err(e),
    }
}

/// The change times of the workspace's folder, of the projects in it and
/// of the folders in each project: while they stand still, no folder was
/// added, removed or renamed, and none gained or lost the marker of a
/// project.
fn stamp(root: &Path) -> Vec<Option<SystemTime>> {
    let modified = |path: &Path| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
    };
    let mut stamp = vec![modified(root)];
    for (_, project) in child_folders(root) {
        stamp.push(modified(&project));
        stamp.extend(
            child_folders(&project)
                .iter()
                .map(|(_, path)| modified(path)),
        );
    }
    stamp
}

/// Keep the registry up to the workspace: once at start, then every
/// `interval_secs`, and only when a change time of the workspace moved.
pub fn spawn_worker(
    storage: std::sync::Arc<Storage>,
    config: ProjectsConfig,
    interval_secs: u64,
    shared: SharedMap,
    live: Option<std::sync::Arc<crate::live_run::Live>>,
) -> Option<tokio::task::JoinHandle<()>> {
    let root = config.root()?;
    Some(tokio::spawn(async move {
        let mut seen = None;
        let mut ticker =
            tokio::time::interval(std::time::Duration::from_secs(interval_secs.max(60)));
        loop {
            ticker.tick().await;
            let (storage, config, root, before) =
                (storage.clone(), config.clone(), root.clone(), seen.clone());
            let told = (storage.clone(), live.clone());
            let pass = tokio::task::spawn_blocking(move || {
                // The folders, and the merges of the registry: a merge
                // changes which project a folder names.
                let now = (stamp(&root), storage.project_redirects_stamp()?);
                if before.as_ref() == Some(&now) {
                    // No folder moved. An entity that appeared since the
                    // last pass is tied to its project all the same.
                    storage.projects_link_missing(canonicalize_name)?;
                    return Ok((now, None));
                }
                sync(&storage, &config).map(|done| (now, done))
            })
            .await;
            match pass {
                Ok(Ok((now, done))) => {
                    seen = Some(now);
                    if let Some((map, report)) = done {
                        if let Ok(mut held) = shared.write() {
                            *held = Some(std::sync::Arc::new(map.clone()));
                        }
                        // The live state names projects as the registry
                        // has them now, merges included.
                        if let (storage, Some(live)) = &told
                            && let Err(e) = live.load_projects(storage)
                        {
                            tracing::warn!("Projects: the live state was not told: {e}");
                        }
                        tracing::info!(
                            "Projects: {} in the workspace ({} folders mapped, {} refused, {} aliases unset)",
                            report.projects,
                            map.folders(),
                            report.refused,
                            report.unset
                        );
                    }
                    if let Some(live) = &told.1 {
                        let live = live.clone();
                        match tokio::task::spawn_blocking(move || live.refresh_graph()).await {
                            Ok(Ok(_)) => {}
                            Ok(Err(e)) => tracing::warn!("Graph: refresh failed: {e}"),
                            Err(e) => tracing::warn!("Graph: refresh join error: {e}"),
                        }
                    }
                }
                Ok(Err(e)) => tracing::warn!("Projects: the workspace could not be read: {e}"),
                Err(e) => tracing::warn!("Projects: join error: {e}"),
            }
        }
    }))
}

#[cfg(test)]
#[path = "registry_tests.rs"]
mod tests;
