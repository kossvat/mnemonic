//! The projects in work now (live-projects, design 11): the state kept in
//! memory and the snapshot it is served as.
//!
//! The state is the sessions the observation pass saw a record of lately,
//! the hours of the day the attribution worker pushes in, and one line of
//! each project's plan. It is a function of what it was given and of the
//! time it is asked at: nothing here reads a clock or a store.
//!
//! The snapshot carries the start of a session, never a duration, so it
//! changes when something happens and not as time passes. Its version
//! moves when and only when its content differs.
use chrono::{DateTime, Duration, NaiveDate, Utc};
use serde::Serialize;
use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex};

use crate::projects::facts::Kind;

pub type ProjectId = i64;

/// A session whose last record does not close a turn is running while that
/// record is younger than this, and quiet after it.
pub const RUNNING_SECS: i64 = 120;
/// A quiet session leaves the list when its last record is this old.
pub const QUIET_SECS: i64 = 1800;
/// A session that waits for the owner leaves the list after this long.
pub const WAITING_SECS: i64 = 600;
/// The snapshot holds at most this many projects and sessions, and counts
/// what it leaves out.
pub const MAX_PROJECTS: usize = 8;
pub const MAX_SESSIONS: usize = 16;
/// A name and a unit are at most this long, a task this long.
pub const NAME_CHARS: usize = 32;
pub const TASK_CHARS: usize = 40;

/// What a live session is doing, from the kind and the age of its last
/// record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SessionState {
    /// An agent is at work.
    Running,
    /// The agent closed its turn: the owner has the move.
    Waiting,
    /// No record for a while, and no turn closed: a long build.
    Quiet,
}

/// One session as the observation pass last saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Session {
    /// The kind of the client: `claude` or `codex`.
    pub agent: String,
    /// Whether a program ran it.
    pub program: bool,
    pub project: Option<ProjectId>,
    pub last_kind: Kind,
    pub last_at: DateTime<Utc>,
    /// When the stretch of work the session is in began.
    pub started_at: DateTime<Utc>,
}

impl Session {
    /// The state at `now`, or `None` when the session is live no more.
    pub fn state_at(&self, now: DateTime<Utc>) -> Option<SessionState> {
        let age = (now - self.last_at).num_seconds();
        match self.last_kind {
            Kind::TurnEnd if age < WAITING_SECS => Some(SessionState::Waiting),
            Kind::TurnEnd => None,
            _ if age < RUNNING_SECS => Some(SessionState::Running),
            _ if age < QUIET_SECS => Some(SessionState::Quiet),
            _ => None,
        }
    }

    /// The next moment after `now` at which the state changes.
    fn changes_at(&self, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
        let ages: &[i64] = match self.last_kind {
            Kind::TurnEnd => &[WAITING_SECS],
            _ => &[RUNNING_SECS, QUIET_SECS],
        };
        ages.iter()
            .map(|age| self.last_at + Duration::seconds(*age))
            .find(|at| *at > now)
    }
}

/// A project as the snapshot names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Named {
    pub key: String,
    pub name: String,
}

/// One line of a project's plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PlanLine {
    pub unit: String,
    pub done: u32,
    pub total: u32,
    pub stage: String,
    pub task: Option<String>,
    pub gate: Option<String>,
}

/// The time of a project today.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Today {
    pub owner_s: i64,
    pub agent_s: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub confidence: Option<&'static str>,
}

/// The hours of one local day, as the attribution worker made them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Hours {
    pub day: Option<NaiveDate>,
    pub projects: HashMap<ProjectId, Today>,
    pub owner_s: i64,
    pub agent_s: i64,
}

#[derive(Serialize)]
struct SessionOut<'a> {
    agent: &'a str,
    driver: &'static str,
    state: SessionState,
    /// Seconds since the epoch: a client counts the elapsed time itself.
    started_at: i64,
}

#[derive(Serialize)]
struct ProjectOut<'a> {
    key: String,
    name: String,
    state: SessionState,
    sessions: Vec<SessionOut<'a>>,
    more_sessions: usize,
    today: Today,
    plan: Option<PlanLine>,
}

#[derive(Serialize)]
struct UnknownOut<'a> {
    sessions: Vec<SessionOut<'a>>,
    more_sessions: usize,
}

#[derive(Serialize)]
struct TotalOut {
    owner_s: i64,
    agent_s: i64,
}

/// What the snapshot holds besides its version.
#[derive(Serialize)]
struct Content<'a> {
    projects: Vec<ProjectOut<'a>>,
    more_projects: usize,
    unknown: UnknownOut<'a>,
    today: TotalOut,
    /// The memories in the store as last counted; left out until then.
    #[serde(skip_serializing_if = "Option::is_none")]
    memories: Option<u64>,
}

/// A text of the snapshot: prepared by the policy and cut to its length.
fn shown(text: &str, max_chars: usize) -> String {
    crate::redaction::state::prepare_capped(text, max_chars).value
}

#[derive(Default)]
struct Inner {
    version: u64,
    sessions: HashMap<String, Session>,
    /// Every project by id, and the id each was merged into.
    names: HashMap<ProjectId, Named>,
    merged: HashMap<ProjectId, ProjectId>,
    plans: HashMap<ProjectId, PlanLine>,
    hours: Hours,
    memories: Option<u64>,
    /// The content of the last snapshot, and the body served for it.
    content: Vec<u8>,
    body: Arc<Vec<u8>>,
}

/// The live state of a daemon. `boot` tells one run of the daemon from
/// another: with the version it is the tag of a snapshot.
pub struct LiveState {
    boot: String,
    inner: Mutex<Inner>,
}

/// What a change of the state left: whether the snapshot differs, and the
/// next moment it will differ by the passing of time alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settled {
    pub changed: bool,
    pub next: Option<DateTime<Utc>>,
}

impl LiveState {
    /// A state with nothing in it, for a run named `boot`.
    pub fn new(boot: impl Into<String>) -> Self {
        let state = Self {
            boot: boot.into(),
            inner: Mutex::new(Inner::default()),
        };
        // The empty snapshot is a snapshot: a client can ask at once.
        let mut inner = state.lock();
        let content = serde_json::to_vec(&Content {
            projects: Vec::new(),
            more_projects: 0,
            unknown: UnknownOut {
                sessions: Vec::new(),
                more_sessions: 0,
            },
            today: TotalOut {
                owner_s: 0,
                agent_s: 0,
            },
            memories: None,
        })
        .unwrap_or_default();
        inner.body = Arc::new(state.body_of(0, &content));
        inner.content = content;
        drop(inner);
        state
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // A panic while the state was held leaves it as it was written.
        self.inner.lock().unwrap_or_else(|held| held.into_inner())
    }

    /// The body of a snapshot: its version and run, then its content.
    fn body_of(&self, version: u64, content: &[u8]) -> Vec<u8> {
        let head = serde_json::json!({ "v": version, "boot": self.boot }).to_string();
        let mut body = Vec::with_capacity(head.len() + content.len());
        // `{"v":..,"boot":".."` then the fields of the content.
        body.extend_from_slice(&head.as_bytes()[..head.len() - 1]);
        if content.len() > 2 {
            body.push(b',');
        }
        body.extend_from_slice(&content[1..]);
        body
    }

    /// The tag of the snapshot served now, and its body.
    pub fn snapshot(&self) -> (String, Arc<Vec<u8>>) {
        let inner = self.lock();
        (
            format!("\"{}-{}\"", self.boot, inner.version),
            inner.body.clone(),
        )
    }

    /// Live inputs for the reader of plans and the project's page.
    pub fn sessions(&self, now: DateTime<Utc>) -> Vec<(String, Session)> {
        let inner = self.lock();
        let mut sessions: Vec<_> = inner
            .sessions
            .iter()
            .filter(|(_, session)| session.state_at(now).is_some())
            .map(|(stream, session)| (stream.clone(), session.clone()))
            .collect();
        sessions.sort_by(|a, b| a.0.cmp(&b.0));
        sessions
    }

    /// The version of the snapshot served now.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn version(&self) -> u64 {
        self.lock().version
    }

    /// A record of `stream` was observed: the session as it is now.
    pub fn observed(&self, stream: &str, session: Session, now: DateTime<Utc>) -> Settled {
        let mut inner = self.lock();
        inner.sessions.insert(stream.to_string(), session);
        self.settle(&mut inner, now)
    }

    /// A transcript was replaced or is known no more.
    pub fn forget(&self, stream: &str, now: DateTime<Utc>) -> Settled {
        let mut inner = self.lock();
        inner.sessions.remove(stream);
        self.settle(&mut inner, now)
    }

    /// The registry as it is now: every project by id, and what was
    /// merged into what.
    pub fn set_projects(
        &self,
        names: HashMap<ProjectId, Named>,
        merged: HashMap<ProjectId, ProjectId>,
        now: DateTime<Utc>,
    ) -> Settled {
        let mut inner = self.lock();
        inner.names = names;
        inner.merged = merged;
        self.settle(&mut inner, now)
    }

    /// The hours of a local day, as the attribution worker made them.
    pub fn set_hours(&self, hours: Hours, now: DateTime<Utc>) -> Settled {
        let mut inner = self.lock();
        inner.hours = hours;
        self.settle(&mut inner, now)
    }

    /// The count of memories in the store, as it was just counted.
    pub fn set_memories(&self, count: u64, now: DateTime<Utc>) -> Settled {
        let mut inner = self.lock();
        inner.memories = Some(count);
        self.settle(&mut inner, now)
    }

    /// The line of a project's plan, or none. The reader of plans tells
    /// it (MN-40-12).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_plan(
        &self,
        project: ProjectId,
        plan: Option<PlanLine>,
        now: DateTime<Utc>,
    ) -> Settled {
        let mut inner = self.lock();
        match plan {
            Some(plan) => inner.plans.insert(project, plan),
            None => inner.plans.remove(&project),
        };
        self.settle(&mut inner, now)
    }

    /// Time passed: sessions change state and leave, and at the midnight
    /// of `today` the hours of the day start again.
    pub fn tick(&self, now: DateTime<Utc>, today: NaiveDate) -> Settled {
        let mut inner = self.lock();
        if inner.hours.day.is_some_and(|day| day != today) {
            inner.hours = Hours {
                day: Some(today),
                ..Hours::default()
            };
        }
        self.settle(&mut inner, now)
    }

    /// Whether any session is in the list, or a day's hours are open: the
    /// timer of the state runs only then.
    pub fn is_armed(&self) -> bool {
        let inner = self.lock();
        !inner.sessions.is_empty() || inner.hours.day.is_some()
    }

    /// Make the snapshot of `now`; move the version when it differs.
    fn settle(&self, inner: &mut Inner, now: DateTime<Utc>) -> Settled {
        // What left the list is forgotten: it is in the store.
        inner
            .sessions
            .retain(|_, session| session.state_at(now).is_some());
        let content = content_of(inner, now);
        let changed = content != inner.content;
        if changed {
            inner.version += 1;
            inner.body = Arc::new(self.body_of(inner.version, &content));
            inner.content = content;
        }
        let next = inner
            .sessions
            .values()
            .filter_map(|session| session.changes_at(now))
            .min();
        Settled { changed, next }
    }
}

fn session_out<'a>(session: &'a Session, state: SessionState) -> SessionOut<'a> {
    SessionOut {
        agent: &session.agent,
        driver: if session.program { "program" } else { "owner" },
        state,
        started_at: session.started_at.timestamp(),
    }
}

/// The content of the snapshot at `now`, serialised.
fn content_of(inner: &Inner, now: DateTime<Utc>) -> Vec<u8> {
    // The live sessions by the project merges resolve them to, each list
    // in the order of what the session does and when it began.
    let mut by_project: BTreeMap<Option<ProjectId>, Vec<(SessionState, &String, &Session)>> =
        BTreeMap::new();
    for (stream, session) in &inner.sessions {
        let Some(state) = session.state_at(now) else {
            continue;
        };
        let project = session
            .project
            .map(|id| inner.merged.get(&id).copied().unwrap_or(id))
            // A project the registry does not know is no project here.
            .filter(|id| inner.names.contains_key(id));
        by_project
            .entry(project)
            .or_default()
            .push((state, stream, session));
    }
    for sessions in by_project.values_mut() {
        sessions.sort_by_key(|(state, stream, session)| (*state, session.started_at, *stream));
    }
    let unknown = by_project.remove(&None).unwrap_or_default();
    let mut projects: Vec<(SessionState, &Named, ProjectId, Vec<_>)> = by_project
        .into_iter()
        .filter_map(|(id, sessions)| {
            let id = id?;
            let state = sessions.iter().map(|(state, _, _)| *state).min()?;
            Some((state, inner.names.get(&id)?, id, sessions))
        })
        .collect();
    projects.sort_by(|a, b| (a.0, &a.1.key).cmp(&(b.0, &b.1.key)));

    // The caps: so many projects, and so many sessions in all.
    let more_projects = projects.len().saturating_sub(MAX_PROJECTS);
    projects.truncate(MAX_PROJECTS);
    let mut room = MAX_SESSIONS;
    let mut take = |sessions: &[(SessionState, &String, &'_ Session)]| {
        let shown = sessions.len().min(room);
        room -= shown;
        shown
    };
    let projects: Vec<ProjectOut<'_>> = projects
        .iter()
        .map(|(state, named, id, sessions)| {
            let shown_sessions = take(sessions);
            ProjectOut {
                // The key is what the routes of a project take: served
                // whole. The name is what a surface shows: cut.
                key: shown(&named.key, usize::MAX),
                name: shown(&named.name, NAME_CHARS),
                state: *state,
                sessions: sessions[..shown_sessions]
                    .iter()
                    .map(|(state, _, session)| session_out(session, *state))
                    .collect(),
                more_sessions: sessions.len() - shown_sessions,
                today: inner.hours.projects.get(id).copied().unwrap_or_default(),
                plan: inner.plans.get(id).map(|plan| PlanLine {
                    unit: shown(&plan.unit, NAME_CHARS),
                    stage: shown(&plan.stage, NAME_CHARS),
                    task: plan.task.as_deref().map(|task| shown(task, TASK_CHARS)),
                    gate: plan.gate.as_deref().map(|gate| shown(gate, NAME_CHARS)),
                    ..plan.clone()
                }),
            }
        })
        .collect();
    let shown_unknown = take(&unknown);
    let content = Content {
        projects,
        more_projects,
        unknown: UnknownOut {
            sessions: unknown[..shown_unknown]
                .iter()
                .map(|(state, _, session)| session_out(session, *state))
                .collect(),
            more_sessions: unknown.len() - shown_unknown,
        },
        today: TotalOut {
            owner_s: inner.hours.owner_s,
            agent_s: inner.hours.agent_s,
        },
        memories: inner.memories,
    };
    serde_json::to_vec(&content).unwrap_or_default()
}

#[cfg(test)]
#[path = "live_tests.rs"]
mod tests;
