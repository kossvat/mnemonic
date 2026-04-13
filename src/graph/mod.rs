pub mod extractor;

use serde::{Deserialize, Serialize};

/// A named entity extracted from a memory event
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub struct Entity {
    pub name: String,
    pub entity_type: EntityType,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
pub enum EntityType {
    Project,
    Module,
    Tech,
    File,
    Concept,
    Person,
}

impl std::fmt::Display for EntityType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Project => write!(f, "project"),
            Self::Module => write!(f, "module"),
            Self::Tech => write!(f, "tech"),
            Self::File => write!(f, "file"),
            Self::Concept => write!(f, "concept"),
            Self::Person => write!(f, "person"),
        }
    }
}

impl EntityType {
    #[allow(dead_code)]
    pub fn from_str(s: &str) -> Self {
        match s {
            "project" => Self::Project,
            "module" => Self::Module,
            "tech" => Self::Tech,
            "file" => Self::File,
            "concept" => Self::Concept,
            "person" => Self::Person,
            _ => Self::Concept,
        }
    }
}

/// A directed relationship between two entities
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Edge {
    pub source: String,
    pub target: String,
    pub relation: String,
    pub memory_id: String,
}
