//! Which project a text names: the map of the workspace, in memory only.
//!
//! A path names a project by the folder that follows the folder of
//! projects in it, wherever the checkout lies: the main one, a worktree, a
//! review copy. So the map is keyed by the names of the folders, and a text
//! is searched for the marker (the name of the folder of projects and a
//! separator); what follows it is looked up, and a project inside a
//! project wins over its parent.
use std::collections::HashMap;
use std::path::Path;

pub type ProjectId = i64;

/// The projects a text names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Named {
    #[default]
    None,
    One(ProjectId),
    Many,
}

impl Named {
    fn with(self, id: ProjectId, merges: &Merges) -> Self {
        match self {
            Self::None => Self::One(id),
            Self::One(known) if merges.same(known, id) => self,
            _ => Self::Many,
        }
    }

    /// Both sets together; two folders the owner merged are one project.
    pub fn and(self, other: Self, merges: &Merges) -> Self {
        match other {
            Self::None => self,
            Self::One(id) => self.with(id, merges),
            Self::Many => Self::Many,
        }
    }
}

/// The projects the owner merged: an id, and the id it resolves to.
///
/// The rules compare projects through it, so a folder and the one it was
/// merged into are one project to them. What they keep is the id of the
/// folder a record named: a merge moves no row, and undoing it finds
/// every row under the id it was written with.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Merges(HashMap<ProjectId, ProjectId>);

impl Merges {
    pub fn insert(&mut self, id: ProjectId, into: ProjectId) {
        if id != into {
            self.0.insert(id, into);
        }
    }

    /// The project `id` resolves to.
    pub fn of(&self, id: ProjectId) -> ProjectId {
        self.0.get(&id).copied().unwrap_or(id)
    }

    pub fn same(&self, a: ProjectId, b: ProjectId) -> bool {
        self.of(a) == self.of(b)
    }

    /// Whether `known`, when there is one, is the project of `id`.
    pub fn is(&self, known: Option<ProjectId>, id: ProjectId) -> bool {
        known.is_some_and(|known| self.same(known, id))
    }
}

#[derive(Debug, Clone, Default)]
struct Entry {
    id: ProjectId,
    inner: HashMap<String, ProjectId>,
}

/// The folders of the workspace by name, and the names they had before.
#[derive(Debug, Clone, Default)]
pub struct ProjectMap {
    /// The folder of projects as a path spells it: its name and a separator.
    marker: String,
    top: HashMap<String, Entry>,
    merges: Merges,
}

/// What ends the name of a folder in a path.
const SEPARATORS: &[char] = &['/', '\\', '"', '\'', '\n', '\r', '\t', '`'];
/// What ends it in a command or a sentence, tried when the whole segment
/// is no project: a name may hold a space, a command line splits on one.
const BREAKS: &[char] = &[
    ' ', ')', '(', ',', ';', ':', '&', '|', '>', '<', '=', '*', '?', ']', '[', '}', '{',
];

fn in_a_name(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '-' | '_' | '.')
}

impl ProjectMap {
    /// An empty map for the projects under `root`.
    pub fn new(root: &Path) -> Self {
        let folder = root
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        Self {
            marker: format!("{folder}/"),
            top: HashMap::new(),
            merges: Merges::default(),
        }
    }

    /// Project `id` was merged into `into`.
    pub fn merge(&mut self, id: ProjectId, into: ProjectId) {
        self.merges.insert(id, into);
    }

    /// What the owner merged.
    pub fn merges(&self) -> &Merges {
        &self.merges
    }

    /// The folder `name` under the folder of projects is project `id`.
    pub fn insert(&mut self, name: &str, id: ProjectId) {
        self.top.entry(name.to_string()).or_default().id = id;
    }

    /// The folder `name` inside the project folder `parent` is project `id`.
    pub fn insert_inside(&mut self, parent: &str, name: &str, id: ProjectId) {
        if let Some(entry) = self.top.get_mut(parent) {
            entry.inner.insert(name.to_string(), id);
        }
    }

    /// How many folders the map holds.
    pub fn folders(&self) -> usize {
        self.top.values().map(|entry| 1 + entry.inner.len()).sum()
    }

    /// The name at the head of `rest`, with what follows it.
    fn segment<'t>(&self, rest: &'t str, known: impl Fn(&str) -> bool) -> Option<&'t str> {
        let whole = rest.split(SEPARATORS).next().unwrap_or(rest);
        if known(whole) {
            return Some(whole);
        }
        let head = whole.split(BREAKS).next().unwrap_or(whole);
        (head.len() < whole.len() && known(head)).then_some(head)
    }

    /// The project named right after a marker, the deepest that matches.
    fn after_marker(&self, rest: &str) -> Option<ProjectId> {
        let name = self.segment(rest, |name| self.top.contains_key(name))?;
        let entry = &self.top[name];
        let inside = rest[name.len()..].strip_prefix('/').and_then(|rest| {
            self.segment(rest, |name| entry.inner.contains_key(name))
                .map(|name| entry.inner[name])
        });
        Some(inside.unwrap_or(entry.id))
    }

    /// Every place in `text` where the marker stands as a folder of its own
    /// (not as the tail of a longer name), with the project named after it.
    fn hits<'t>(&'t self, text: &'t str) -> impl Iterator<Item = ProjectId> + 't {
        let mut from = 0;
        std::iter::from_fn(move || {
            // A folder of projects without a name marks nothing.
            while self.marker.len() > 1 {
                let at = from + text[from..].find(&self.marker)?;
                from = at + self.marker.len();
                let alone = text[..at].chars().next_back().is_none_or(|c| !in_a_name(c));
                if alone && let Some(id) = self.after_marker(&text[from..]) {
                    return Some(id);
                }
            }
            None
        })
    }

    /// The project a path lies in: the first folder of projects in it that
    /// names one.
    pub fn project_of(&self, path: &str) -> Option<ProjectId> {
        self.hits(path).next()
    }

    /// The projects a text names, in any of its paths.
    pub fn named_in(&self, text: &str) -> Named {
        let mut named = Named::None;
        for id in self.hits(text) {
            named = named.with(id, &self.merges);
            if named == Named::Many {
                break;
            }
        }
        named
    }
}
