//! Replay the transcripts through the resolver without writing anything
//! (live-projects, design 9.2): `mnemonic attribute replay --dry-run`.
//!
//! It reads history through the reader of the Records backfill, the same
//! files, scopes and rule for a session a program ran, and opens the store
//! through its read-only entry. It prints aggregates and no text of a
//! record: counts, hours, the names of projects, and a sample of segments
//! as times for the owner to label.
use anyhow::{Result, bail};
use chrono::{DateTime, Duration, Utc};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use super::codex::CodexWatcher;
use super::conversation::ConversationWatcher;
use super::history::{TranscriptDirs, complete_records};
use super::ingress::RecordScope;
use super::scope::{ClaudeScope, CodexScope, ProjectScope};
use crate::config::Config;
use crate::projects::facts::{self, Observed};
use crate::projects::lookup::Merges;
use crate::projects::resolver::Source;
use crate::projects::segments::{Counts, Learned, Segment, Walker};
use crate::projects::{ProjectMap, key_of, list};
use crate::storage::readonly::ReadOnlyStore;

/// What the dry run is asked for.
pub struct Options {
    /// Time in the last `days` days counts.
    pub days: u32,
    /// How many segments to show for labelling.
    pub sample: usize,
}

/// Who drove a transcript.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Driver {
    Owner,
    Program,
}

/// A segment as the report shows it: times, names and enums.
#[derive(Debug, Clone, PartialEq)]
pub struct Shown {
    pub started_at: DateTime<Utc>,
    pub seconds: i64,
    pub agent: &'static str,
    pub driver: Driver,
    /// The key of the project, or `None` for unknown.
    pub project: Option<String>,
    pub learned: &'static str,
}

/// The aggregates of a dry run.
#[derive(Debug, Default)]
pub struct Report {
    pub projects: usize,
    pub refused: usize,
    pub transcripts: usize,
    pub bytes: u64,
    pub elapsed_secs: f64,
    pub days: u32,
    pub counts: Counts,
    /// Seconds of transcripts by driver and project; `None` is unknown.
    pub seconds: BTreeMap<(Driver, Option<String>), i64>,
    /// Seconds of the owner's transcripts by how the project was learned.
    pub learned: BTreeMap<&'static str, i64>,
    /// Transcripts of the owner in the window, and those among them that
    /// worked on more than one project.
    pub owner_transcripts: usize,
    pub on_several: usize,
    pub sample: Vec<Shown>,
}

fn learned_name(learned: Learned) -> &'static str {
    match learned {
        Learned::Backfill => "backfill",
        Learned::By(Source::Unknown) => "unknown",
        Learned::By(Source::Launch) => "launch",
        Learned::By(Source::Folder) => "folder",
        Learned::By(Source::Path) => "path",
        Learned::By(Source::Manual) => "manual",
    }
}

/// One transcript read whole: its segments, what was counted, who drove it.
struct Read {
    segments: Vec<Segment>,
    counts: Counts,
    driver: Driver,
}

/// Read one transcript. `in_scope` says for each record whether the scope
/// admits it; `observe` reads its facts. Who drove the session is what the
/// first record that says so says, as the observation pass decides it: a
/// record the scope leaves out says it as well as any other.
fn read(
    path: &Path,
    idle_secs: u64,
    merges: &Merges,
    mut in_scope: impl FnMut(&str) -> bool,
    observe: impl Fn(&str) -> Option<Observed>,
) -> Result<Read> {
    let mut walker = Walker::new(idle_secs).merged(merges.clone());
    let mut program: Option<bool> = None;
    complete_records(path, |line| {
        let admitted = in_scope(line);
        if let Some(observed) = observe(line.trim_end_matches('\n')) {
            if program.is_none() {
                program = observed.program;
            }
            if admitted {
                walker.record(&observed, None);
            }
        }
    })?;
    let (segments, counts) = walker.finish();
    let driver = if program == Some(true) {
        Driver::Program
    } else {
        Driver::Owner
    };
    Ok(Read {
        segments,
        counts,
        driver,
    })
}

/// Replay every transcript the watchers read and give the aggregates. It
/// writes nothing, to the store or anywhere.
pub fn dry_run(config: &Config, options: &Options, now: DateTime<Utc>) -> Result<Report> {
    let Some(root) = config.projects.root() else {
        bail!("no workspace is configured: set [projects] workspace in the configuration");
    };
    let began = std::time::Instant::now();
    // The projects as the workspace holds them now, numbered by their place
    // in the listing: the registry of the store is not needed, and may not
    // be there yet. What the owner merged, when the store has a registry,
    // is given to the rules as the daemon gives it: a project and the one
    // it was merged into are one project to them. A project others were
    // merged into is a project though its folder is gone: it is numbered
    // after the listing.
    let listing = list(&root);
    let redirects = match ReadOnlyStore::open(&config.storage.db_path)? {
        Some(store) => store.project_redirects()?,
        None => Default::default(),
    };
    let mut keys: Vec<String> = listing
        .found
        .iter()
        .map(|found| found.slug.clone())
        .collect();
    let mut map = ProjectMap::new(&root);
    for (index, found) in listing.found.iter().enumerate() {
        match found.parent {
            Some(parent) => {
                map.insert_inside(&listing.found[parent].name, &found.name, index as i64)
            }
            None => map.insert(&found.name, index as i64),
        }
        if let Some(canonical) = redirects.get(&found.slug) {
            let into = keys.iter().position(|key| key == canonical);
            let into = into.unwrap_or_else(|| {
                keys.push(canonical.clone());
                keys.len() - 1
            });
            map.merge(index as i64, into as i64);
        }
    }
    let place = |slug: &str| listing.found.iter().position(|found| found.slug == slug);
    for (former, target) in &config.projects.former {
        // As the registry takes a former name: not one a folder carries now.
        let carried = listing
            .found
            .iter()
            .any(|found| found.parent.is_none() && found.name == *former);
        if !carried && let Some(index) = place(&key_of(target)) {
            map.insert(former, index as i64);
        }
    }
    let merges = map.merges().clone();
    let key = |id: i64| -> String { keys[merges.of(id) as usize].clone() };

    let mut report = Report {
        projects: listing.found.len(),
        refused: listing.refused,
        days: options.days,
        ..Report::default()
    };
    let since = now - Duration::days(i64::from(options.days));
    let idle = config.sessions.idle_timeout_secs;
    let scope = ProjectScope::new(&config.watchers.project_roots);
    let dirs = TranscriptDirs::of(config);
    let mut shown: Vec<Shown> = Vec::new();
    let mut take = |report: &mut Report, read: Read, agent: &'static str, bytes: u64| {
        report.transcripts += 1;
        report.bytes += bytes;
        report.counts.records += read.counts.records;
        report.counts.naming += read.counts.naming;
        report.counts.ambiguous += read.counts.ambiguous;
        report.counts.switches += read.counts.switches;
        let mut projects = std::collections::BTreeSet::new();
        let mut in_window = false;
        for segment in &read.segments {
            // Only the part of a segment inside the window counts.
            let (started, last) = (segment.started_at.max(since), segment.last_at.min(now));
            if last < started || segment.last_at < since {
                continue;
            }
            in_window = true;
            let project = segment.project.map(key);
            let seconds = (last - started).num_seconds();
            *report
                .seconds
                .entry((read.driver, project.clone()))
                .or_default() += seconds;
            if read.driver == Driver::Owner {
                *report
                    .learned
                    .entry(learned_name(segment.learned))
                    .or_default() += seconds;
                projects.extend(project.clone());
            }
            shown.push(Shown {
                started_at: started,
                seconds,
                agent,
                driver: read.driver,
                project,
                learned: learned_name(segment.learned),
            });
        }
        if in_window && read.driver == Driver::Owner {
            report.owner_transcripts += 1;
            report.on_several += usize::from(projects.len() > 1);
        }
    };
    let size = |path: &Path| std::fs::metadata(path).map(|meta| meta.len()).unwrap_or(0);

    if let Some(dir) = dirs.conversation.filter(|dir| dir.exists()) {
        let mut folder = ClaudeScope::new(scope.clone());
        for path in ConversationWatcher::new(dir).find_jsonl_files() {
            folder.reset();
            let read = read(
                &path,
                idle,
                &merges,
                |line| !scope.is_restricted() || folder.admit(line),
                |line| facts::claude(line, &map),
            )?;
            take(&mut report, read, "claude", size(&path));
        }
    }
    // The archive beside the sessions folder holds history when that
    // folder itself is gone: either is enough.
    let archived = dirs.codex_archive.filter(|dir| dir.exists());
    if let Some(dir) = dirs.codex.filter(|dir| dir.exists() || archived.is_some()) {
        let mut watcher = CodexWatcher::new(dir);
        if let Some(archive) = archived {
            watcher = watcher.with_archived_dir(archive);
        }
        let mut folder = CodexScope::new(scope.clone());
        for path in watcher.find_rollout_files() {
            folder.reset();
            let read = read(
                &path,
                idle,
                &merges,
                |line| !scope.is_restricted() || folder.admit(line),
                |line| facts::codex(line, &map),
            )?;
            take(&mut report, read, "codex", size(&path));
        }
    }
    report.sample = sample(shown, options.sample);
    report.elapsed_secs = began.elapsed().as_secs_f64();
    Ok(report)
}

/// The segments to label: half the longest, half spread over the rest, of
/// those that lasted a minute or more; in the order of their start.
fn sample(mut segments: Vec<Shown>, count: usize) -> Vec<Shown> {
    segments.retain(|s| s.seconds >= 60 && s.driver == Driver::Owner);
    segments.sort_by(|a, b| {
        b.seconds
            .cmp(&a.seconds)
            .then(a.started_at.cmp(&b.started_at))
    });
    let longest = count.div_ceil(2).min(segments.len());
    let mut chosen: Vec<Shown> = segments.drain(..longest).collect();
    let rest = count.saturating_sub(chosen.len()).min(segments.len());
    for step in 0..rest {
        chosen.push(segments[step * segments.len() / rest].clone());
    }
    chosen.sort_by_key(|shown| shown.started_at);
    chosen
}

fn hours(seconds: i64) -> f64 {
    seconds as f64 / 3600.0
}

impl std::fmt::Display for Report {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let megabytes = self.bytes as f64 / 1e6;
        writeln!(f, "Attribution replay, dry run: nothing was written.")?;
        writeln!(
            f,
            "Workspace: {} projects, {} folders not taken.",
            self.projects, self.refused
        )?;
        writeln!(
            f,
            "Read {} transcripts ({megabytes:.0} MB) in {:.1} s, {:.0} MB/s.",
            self.transcripts,
            self.elapsed_secs,
            megabytes / self.elapsed_secs.max(1e-9)
        )?;
        writeln!(f, "Window: the last {} days.", self.days)?;
        for (driver, title) in [
            (Driver::Owner, "The owner's sessions"),
            (Driver::Program, "Sessions a program ran"),
        ] {
            let mut rows: Vec<(&Option<String>, i64)> = self
                .seconds
                .iter()
                .filter(|((of, _), _)| *of == driver)
                .map(|((_, project), seconds)| (project, *seconds))
                .collect();
            let total: i64 = rows.iter().map(|(_, seconds)| seconds).sum();
            writeln!(
                f,
                "\n{title}, transcript-hours by project ({:.1} h):",
                hours(total)
            )?;
            rows.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(b.0)));
            for (project, seconds) in rows {
                let name = project.as_deref().unwrap_or("(unknown)");
                let share = 100.0 * seconds as f64 / total.max(1) as f64;
                writeln!(f, "  {name:<32} {:>7.1} h  {share:>5.1}%", hours(seconds))?;
            }
        }
        let total: i64 = self.learned.values().sum();
        let mut line = String::new();
        for (name, seconds) in &self.learned {
            let share = 100.0 * *seconds as f64 / total.max(1) as f64;
            write!(line, "  {name} {share:.1}%")?;
        }
        writeln!(
            f,
            "\nHow the project was learned (the owner's sessions):{line}"
        )?;
        let naming = self.counts.naming + self.counts.ambiguous;
        writeln!(
            f,
            "Records: {}. Calls that named a project: {naming}, of them ambiguous: {} ({:.1}%). Switches: {}.",
            self.counts.records,
            self.counts.ambiguous,
            100.0 * self.counts.ambiguous as f64 / naming.max(1) as f64,
            self.counts.switches
        )?;
        writeln!(
            f,
            "The owner's transcripts on more than one project: {} of {}.",
            self.on_several, self.owner_transcripts
        )?;
        if !self.sample.is_empty() {
            writeln!(
                f,
                "\n{} segments to label: is the project right?",
                self.sample.len()
            )?;
            writeln!(
                f,
                "   #  started (local)    min  agent   project                          learned"
            )?;
            for (index, shown) in self.sample.iter().enumerate() {
                let local = shown.started_at.with_timezone(&chrono::Local);
                writeln!(
                    f,
                    "  {:>2}  {}  {:>5}  {:<6}  {:<32} {}",
                    index + 1,
                    local.format("%m-%d %a %H:%M"),
                    shown.seconds / 60,
                    shown.agent,
                    shown.project.as_deref().unwrap_or("(unknown)"),
                    shown.learned
                )?;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "replay_tests.rs"]
mod tests;
