//! Which project a session is working on, record by record (live-projects,
//! design 7.3).
//!
//! A pure function from the previous state and the facts of a record to
//! the next state. The state is ids and enums only, so it can be kept in a
//! checkpoint and the rules resumed exactly where they stopped.
//!
//! In order:
//! 1. a pin of the transcript by the owner decides, whatever the record;
//! 2. a call of a write tool that names exactly one project switches to it
//!    at once;
//! 3. a call of a read tool that names exactly one project makes it the
//!    candidate, and the state switches when two calls in a row, among
//!    those that name a project, name the same one;
//! 4. a call of the shell is judged by its folder, as a read: the folder
//!    it runs in when that is a project's (the one the call names, else
//!    the one the session is in), else the one project its command names;
//! 5. a call that names two or more projects changes nothing and is
//!    counted as ambiguous;
//! 6. a change of the record's folder into a project's folder switches at
//!    once;
//! 7. otherwise the state stays. Before the first signal it is the
//!    project of the launch folder, or unknown.
pub use super::lookup::{Merges, Named, ProjectId};

/// What kind of tool a call is of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolClass {
    Write,
    Read,
    Shell,
}

/// Where the folder a record names lies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Folder {
    /// The record does not say where the session is.
    #[default]
    Unsaid,
    /// In no project of the workspace.
    Outside,
    Project(ProjectId),
}

/// What the rules read of a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Facts {
    pub folder: Folder,
    /// A call of a tool: its class and the projects its input names.
    pub call: Option<(ToolClass, Named)>,
    /// The folder a call of the shell says it runs in, when it says one:
    /// a call can run elsewhere than where the session is.
    pub shell_dir: Folder,
}

/// How the project in force was learned.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash, PartialOrd, Ord)]
pub enum Source {
    #[default]
    Unknown,
    /// The folder the session was launched in.
    Launch,
    /// A later change of the session's folder.
    Folder,
    /// A path in the input of a call.
    Path,
    /// A pin by the owner.
    Manual,
}

impl Source {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Launch => "launch",
            Self::Folder => "folder",
            Self::Path => "path",
            Self::Manual => "manual",
        }
    }

    pub fn parse(text: &str) -> Self {
        match text {
            "launch" => Self::Launch,
            "folder" => Self::Folder,
            "path" => Self::Path,
            "manual" => Self::Manual,
            _ => Self::Unknown,
        }
    }
}

/// The state of the rules after a record.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct State {
    pub project: Option<ProjectId>,
    pub source: Source,
    /// The project the last naming read named, when it was not the one in
    /// force: a second one in a row switches to it.
    pub candidate: Option<ProjectId>,
    /// The project of the folder the session is in, by the last record
    /// that said where it is.
    pub folder: Option<ProjectId>,
    /// Whether a record has said where the session is.
    pub placed: bool,
}

/// What a record did to the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Stayed,
    Switched,
    /// A read named another project for the first time.
    Candidate,
    /// A call named two or more projects.
    Ambiguous,
}

/// The state after a record, and what the record did, where no project
/// was merged.
#[cfg_attr(not(test), allow(dead_code))]
pub fn step(state: State, pin: Option<ProjectId>, facts: &Facts) -> (State, Outcome) {
    step_merged(state, pin, facts, &Merges::default())
}

/// The state after a record, and what the record did.
///
/// Two folders the owner merged are one project to every rule: a move
/// between them is no switch. The state still holds the id of the folder
/// the last write, or the last change of folder, named, so what is kept
/// says where the work was.
pub fn step_merged(
    state: State,
    pin: Option<ProjectId>,
    facts: &Facts,
    merges: &Merges,
) -> (State, Outcome) {
    let mut next = state;
    // Where the session is, whatever else the record says.
    let mut moved = None;
    let entered = match facts.folder {
        Folder::Unsaid => None,
        Folder::Outside => {
            next.folder = None;
            None
        }
        Folder::Project(id) => {
            next.folder = Some(id);
            moved = (state.folder != Some(id)).then_some(id);
            moved.filter(|id| !merges.is(state.folder, *id))
        }
    };
    let said = facts.folder != Folder::Unsaid;
    next.placed = state.placed || said;

    let switch = |mut next: State, id: ProjectId, source: Source| {
        let outcome = if merges.is(next.project, id) {
            Outcome::Stayed
        } else {
            Outcome::Switched
        };
        if outcome == Outcome::Switched || source == Source::Manual {
            next.source = source;
        }
        next.project = Some(id);
        next.candidate = None;
        (next, outcome)
    };

    // 1. A pin decides.
    if let Some(pin) = pin {
        return switch(next, pin, Source::Manual);
    }

    let mut outcome = Outcome::Stayed;
    if let Some((class, named)) = facts.call {
        // 4. The shell is judged by its folder, as a read: the folder the
        // call says it runs in, else the one the session is in.
        let ran_in = match facts.shell_dir {
            Folder::Project(id) => Some(id),
            Folder::Outside => None,
            Folder::Unsaid => next.folder,
        };
        let (class, named) = match (class, ran_in) {
            (ToolClass::Shell, Some(folder)) => (ToolClass::Read, Named::One(folder)),
            (ToolClass::Shell, None) => (ToolClass::Read, named),
            (class, _) => (class, named),
        };
        match (class, named) {
            // 2. A write that names one project switches at once.
            (ToolClass::Write, Named::One(id)) => return switch(next, id, Source::Path),
            // 3. A read needs two in a row.
            (_, Named::One(id)) if merges.is(next.project, id) => next.candidate = None,
            (_, Named::One(id)) if merges.is(next.candidate, id) => {
                return switch(next, id, Source::Path);
            }
            (_, Named::One(id)) => {
                next.candidate = Some(id);
                outcome = Outcome::Candidate;
            }
            // 5. Two or more change nothing.
            (_, Named::Many) => outcome = Outcome::Ambiguous,
            (_, Named::None) => {}
        }
    }

    // 6. The folder changed into a project's. The first record to say where
    // the session is names the folder it was launched in.
    if let Some(id) = entered
        && !merges.is(next.project, id)
    {
        let source = if state.placed {
            Source::Folder
        } else {
            Source::Launch
        };
        let candidate = next
            .candidate
            .filter(|candidate| !merges.same(*candidate, id));
        let (mut next, _) = switch(next, id, source);
        next.candidate = candidate;
        return (next, Outcome::Switched);
    }
    // The session moved to a folder merged with the project in work: the
    // same project, kept from here under the folder it is in now.
    if let Some(id) = moved
        && merges.is(next.project, id)
    {
        next.project = Some(id);
    }
    // 7. Otherwise the state stays.
    (next, outcome)
}

#[cfg(test)]
#[path = "resolver_tests.rs"]
mod tests;
