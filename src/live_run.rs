//! The live state as a daemon runs it (live-projects, design 11.1 and
//! 11.3): what changes it is told here, each change posts or owes a
//! signal, and one timer wakes at the next moment time alone changes the
//! snapshot.
//!
//! The timer sleeps on the monotonic clock and is armed only while a
//! session is in the list, a day's hours are open or a signal is owed.
//! With nothing to wait for it waits for the next change, and wakes no
//! one.
use anyhow::Result;
use chrono::{DateTime, Duration, Utc};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use crate::live::{Hours, LiveState, Named, PlanLine, ProjectId, QUIET_SECS, Session, Settled};
use crate::live_signal::{Post, Throttle};
use crate::owner_time_day::Calendar;
use crate::storage::Storage;

#[path = "live_graph.rs"]
mod graph;
#[path = "live_plans.rs"]
mod plans;

pub struct Live {
    pub state: LiveState,
    /// The name the signal is posted under.
    name: String,
    post: Box<dyn Post>,
    throttle: Mutex<Throttle>,
    /// The next moment the passing of time changes the snapshot.
    next: Mutex<Option<DateTime<Utc>>>,
    wake: tokio::sync::Notify,
    plans: Mutex<plans::Plans>,
    graph: Mutex<graph::Graph>,
    /// The stamp of the store at the last count of its memories.
    counted: Mutex<Option<(i64, u64)>>,
}

impl Live {
    /// The live state of a run named `boot`, signalled under `name`.
    pub fn new(boot: impl Into<String>, name: String, post: Box<dyn Post>) -> Arc<Self> {
        Arc::new(Self {
            state: LiveState::new(boot),
            name,
            post,
            throttle: Mutex::new(Throttle::default()),
            next: Mutex::new(None),
            wake: tokio::sync::Notify::new(),
            plans: Mutex::new(plans::Plans::default()),
            graph: Mutex::new(graph::Graph::default()),
            counted: Mutex::new(None),
        })
    }

    /// The live state of a daemon whose profile lives in `home`: a run of
    /// its own, signalled under the identity of the profile through the
    /// service of the system.
    pub fn of_profile(home: &std::path::Path) -> Result<Arc<Self>> {
        let identity = crate::live_signal::identity(home)?;
        let boot = uuid::Uuid::new_v4().simple().to_string();
        Ok(Self::new(
            &boot[..16],
            crate::live_signal::name_of(&identity),
            Box::new(crate::live_signal::System),
        ))
    }

    fn signal(&self, changed: bool, now: Instant) {
        let mut throttle = self
            .throttle
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        if (changed && throttle.changed(now)) || throttle.tick(now) {
            drop(throttle);
            self.post.post(&self.name);
        }
    }

    /// Take what a change of the state left: signal it, and let the timer
    /// see the next moment.
    fn settled(&self, settled: Settled) {
        *self.next.lock().unwrap_or_else(|held| held.into_inner()) = settled.next;
        self.signal(settled.changed, Instant::now());
        self.wake.notify_one();
    }

    /// A daemon started: clients are told once, whatever the state.
    pub fn started(&self) {
        self.signal(true, Instant::now());
    }

    /// A record of a transcript was observed.
    pub fn observed(&self, stream: &str, session: Session) {
        self.observed_with_location(stream, session, None);
    }

    /// A transcript was replaced, or has no timed record.
    pub fn forget(&self, stream: &str) {
        self.forget_with_location(stream);
    }

    pub fn set_hours(&self, hours: Hours) {
        self.settled(self.state.set_hours(hours, Utc::now()));
    }

    /// Count the memories of the store now: a client shows the count
    /// beside the projects. A count that fails leaves the last one.
    pub fn count_memories(&self, storage: &Storage) {
        // Asked every few seconds, at rest too: the count is taken again
        // only when the store was written since the last one.
        let stamp = storage.change_stamp().ok();
        let mut counted = self.counted.lock().unwrap_or_else(|held| held.into_inner());
        if stamp.is_some() && *counted == stamp {
            return;
        }
        match storage.count() {
            Ok(count) => {
                *counted = stamp;
                drop(counted);
                self.settled(self.state.set_memories(count as u64, Utc::now()));
            }
            Err(e) => tracing::debug!("Live: the memories were not counted: {e}"),
        }
    }

    /// The reader of plans tells the line of a project's plan (MN-40-12).
    pub fn set_plan(&self, project: ProjectId, plan: Option<PlanLine>) {
        self.settled(self.state.set_plan(project, plan, Utc::now()));
    }

    /// Take the registry as the store has it now.
    pub fn load_projects(&self, storage: &Storage) -> Result<()> {
        let mut names = HashMap::new();
        let mut merged = HashMap::new();
        for row in storage.projects()? {
            let into = storage.project_canonical(row.id)?;
            if into != row.id {
                merged.insert(row.id, into);
            }
            names.insert(
                row.id,
                Named {
                    key: row.slug,
                    name: row.name,
                },
            );
        }
        self.settled(self.state.set_projects(names, merged, Utc::now()));
        Ok(())
    }

    /// Rebuild the sessions after a start: from the checkpoints of the
    /// transcripts that had a record in the time a session stays live.
    pub fn rebuild(&self, storage: &Storage, now: DateTime<Utc>) -> Result<usize> {
        self.load_projects(storage)?;
        let sessions = storage.live_sessions(now - Duration::seconds(QUIET_SECS))?;
        let found = sessions.len();
        for (stream, session) in sessions {
            self.settled(self.state.observed(&stream, session, now));
        }
        Ok(found)
    }

    /// Run the timer of the state until the daemon ends.
    pub fn spawn(
        self: Arc<Self>,
        calendar: Arc<dyn Calendar + Send + Sync>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                let wait = self.wait(&*calendar, Utc::now(), Instant::now());
                match wait {
                    // Nothing changes by time alone: wait for a change.
                    None => self.wake.notified().await,
                    Some(wait) => {
                        tokio::select! {
                            _ = tokio::time::sleep(wait) => {}
                            _ = self.wake.notified() => {}
                        }
                    }
                }
                self.pass(&*calendar, Utc::now(), Instant::now());
            }
        })
    }

    /// How long the timer sleeps: until a session changes state or
    /// leaves, until local midnight while the state is armed, until an
    /// owed signal is due. `None` when none of them is ahead.
    fn wait(
        &self,
        calendar: &dyn Calendar,
        now: DateTime<Utc>,
        clock: Instant,
    ) -> Option<std::time::Duration> {
        let by_state = *self.next.lock().unwrap_or_else(|held| held.into_inner());
        let midnight = self
            .state
            .is_armed()
            .then(|| calendar.bounds(calendar.date_of(now)))
            .flatten()
            .map(|(_, end)| end);
        let by_wall = [by_state, midnight]
            .into_iter()
            .flatten()
            .min()
            .map(|at| (at - now).to_std().unwrap_or_default());
        let owed = self
            .throttle
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .due()
            .map(|at| at.saturating_duration_since(clock));
        [by_wall, owed].into_iter().flatten().min()
    }

    /// One pass of the timer: time moved the state, and an owed signal
    /// that is due is posted.
    fn pass(&self, calendar: &dyn Calendar, now: DateTime<Utc>, clock: Instant) {
        let settled = self.state.tick(now, calendar.date_of(now));
        *self.next.lock().unwrap_or_else(|held| held.into_inner()) = settled.next;
        self.signal(settled.changed, clock);
    }
}

#[cfg(test)]
#[path = "live_run_tests.rs"]
mod tests;
