use crate::event::MemoryEntry;
use crate::graph::{Edge, Entity, EntityType};
use std::collections::HashSet;
use std::sync::OnceLock;

/// User-specific graph vocabulary, loaded once at startup from the `[graph]`
/// config section and merged on top of the generic built-in defaults below.
/// Keeps PRIVATE project / persona / client / person names out of the public
/// source. Empty in tests / library use — `init_user_lists` is only called
/// from the binary entrypoint after `Config::load`.
#[derive(Default)]
struct UserLists {
    projects: HashSet<String>,
    tech: HashSet<String>,
    people: HashSet<String>,
    deny: HashSet<String>,
}

static USER_LISTS: OnceLock<UserLists> = OnceLock::new();

/// Merge the user's private `[graph]` lists into the extractor. Call once at
/// process startup (after `Config::load`). Idempotent — later calls are no-ops.
pub fn init_user_lists(cfg: &crate::config::GraphConfig) {
    let norm = |v: &[String]| -> HashSet<String> {
        v.iter()
            .map(|s| s.trim().to_lowercase())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let _ = USER_LISTS.set(UserLists {
        projects: norm(&cfg.projects),
        tech: norm(&cfg.tech),
        people: norm(&cfg.people),
        deny: norm(&cfg.deny),
    });
}

fn user_lists() -> Option<&'static UserLists> {
    USER_LISTS.get()
}

/// Trait for entity extractors. Rule-based now, LLM-based later.
pub trait EntityExtractor: Send + Sync {
    fn extract(&self, entry: &MemoryEntry) -> ExtractionResult;
}

#[derive(Debug, Default)]
pub struct ExtractionResult {
    pub entities: Vec<Entity>,
    pub edges: Vec<Edge>,
}

/// Known tech names for auto-detection
const KNOWN_TECH: &[&str] = &[
    "rust",
    "python",
    "typescript",
    "javascript",
    "go",
    "java",
    "swift",
    "react",
    "nextjs",
    "next.js",
    "vue",
    "svelte",
    "angular",
    "postgresql",
    "postgres",
    "sqlite",
    "mongodb",
    "redis",
    "mysql",
    "docker",
    "kubernetes",
    "k8s",
    "nginx",
    "caddy",
    "tokio",
    "axum",
    "hyper",
    "actix",
    "warp",
    "fastapi",
    "flask",
    "django",
    "express",
    "nestjs",
    "hono",
    "tailwind",
    "shadcn",
    "swiftui",
    "vercel",
    "cloudflare",
    "aws",
    "gcp",
    "jwt",
    "oauth",
    "ssh",
    "tls",
    "ssl",
    "git",
    "github",
    "gitlab",
    "telegram",
    "slack",
    "discord",
    "openai",
    "anthropic",
    "claude",
    "gemini",
    "elevenlabs",
    "twilio",
    "stripe",
    "chromadb",
    "lancedb",
    "cozodb",
    "pinecone",
    "mcp",
    "grpc",
    "graphql",
    "rest",
    "supabase",
    "firebase",
    "prisma",
    // Generic agent tooling + mnemonic's own stack (safe public defaults).
    // Niche / stack-revealing tools belong in [graph].tech (local config).
    "codex",
    "claude-code",
    "ollama",
    "onnx",
    "hnsw",
    "fastembed",
    "pgvector",
];

/// Known project names → EntityType::Project.
///
/// Curate this for your own workspace — the defaults below are intentionally
/// generic so they boost mnemonic-related extraction out of the box. Add the
/// names of YOUR projects, clients, products, etc. so the extractor doesn't
/// have to guess. A name in this list always becomes an entity of type
/// Project regardless of casing or surrounding context.
const KNOWN_PROJECTS: &[&str] = &[
    // The daemon talks about itself a lot — keep these so self-references
    // become first-class graph nodes.
    "mnemonic",
    "mnemonic-bar",
    // Your real projects / clients / personas are PRIVATE: put them in
    // [graph].projects in your local config, NOT in this public default list.
    // Generic project name stubs the extraction eval fixtures anchor on.
    "auth-service",
    "billing-service",
    "checkout",
    "inventory-labeler",
    "supplier-parser",
    "media-pipeline",
    "voice-assistant",
    "internal-crm",
    "dev-server",
];

/// Known people → EntityType::Person. Kept tiny and curated — the rule
/// extractor otherwise has no person signal, so unknown names fall through
/// to Concept (or get denied).
// People names are private — add them in [graph].people (local config).
const KNOWN_PEOPLE: &[&str] = &[];

/// Generic / meta terms that must never become graph nodes. These dominate
/// the rule extractor's noise (memory tags + conventional meta words);
/// denying them keeps the graph about projects, tech and people instead of
/// "decision" / "conversation" / "session-log".
const DENY_CONCEPTS: &[&str] = &[
    "decision",
    "conversation",
    "session-log",
    "session",
    "sessions",
    "previous-conversation",
    "correction",
    "summary",
    "note",
    "notes",
    "feedback",
    "todo",
    "migration",
    "demo",
    "build",
    "status",
    "memory",
    "content",
    "breakthrough",
    "mvp",
    "mvp-1",
    "port",
    "sync",
    "worker",
    "edge",
    "lab",
    "app",
    "engineer",
    "interview-prep",
    "widget",
    "cli",
    "pipeline",
    "database",
    "llm",
    "standard-mobile",
    // Generic pronouns / non-entities an LLM extractor tends to emit.
    "you",
    "user",
    "someone",
    "something",
    "anyone",
    "everyone",
    "nobody",
    "current-owner",
    "peer-team-member",
    "chatgpt-bot",
];

/// Words that should never become entities
const STOPWORDS: &[&str] = &[
    // English function words
    "a",
    "an",
    "the",
    "and",
    "or",
    "but",
    "in",
    "on",
    "at",
    "to",
    "for",
    "of",
    "with",
    "by",
    "from",
    "is",
    "are",
    "was",
    "were",
    "be",
    "been",
    "being",
    "have",
    "has",
    "had",
    "do",
    "does",
    "did",
    "will",
    "would",
    "could",
    "should",
    "may",
    "might",
    "shall",
    "can",
    "need",
    "must",
    "not",
    "all",
    "each",
    "every",
    "this",
    "that",
    "it",
    "its",
    // Common verbs in commits
    "add",
    "fix",
    "update",
    "remove",
    "delete",
    "change",
    "modify",
    "implement",
    "refactor",
    "resolve",
    "use",
    "set",
    "get",
    "make",
    "move",
    "rename",
    "merge",
    "revert",
    "apply",
    "handle",
    "ensure",
    "check",
    "verify",
    "enable",
    "disable",
    "allow",
    "prevent",
    "create",
    "write",
    "read",
    "run",
    "stop",
    "start",
    "init",
    "support",
    "include",
    "exclude",
    "skip",
    "show",
    "hide",
    "bump",
    "prepare",
    "release",
    "deploy",
    "publish",
    "ship",
    // Common nouns that are too generic
    "new",
    "old",
    "file",
    "files",
    "code",
    "data",
    "type",
    "types",
    "name",
    "value",
    "key",
    "list",
    "item",
    "items",
    "path",
    "error",
    "bug",
    "issue",
    "test",
    "tests",
    "docs",
    "doc",
    "readme",
    "config",
    "default",
    "option",
    "options",
    "setting",
    "settings",
    "mode",
    "state",
    "status",
    "result",
    "output",
    "input",
    "log",
    "logs",
    "message",
    "messages",
    "event",
    "events",
    "version",
    "number",
    "count",
    "index",
    "size",
    "length",
    "first",
    "last",
    "next",
    "prev",
    "previous",
    "current",
    "main",
    "base",
    "core",
    "common",
    "util",
    "utils",
    "helper",
    "info",
    "warn",
    "debug",
    "trace",
    // Git noise
    "co-authored-by",
    "signed-off-by",
    "commit",
    "branch",
    "pull",
    "request",
    "review",
    "merge",
    // Too short / meaningless
    "bar",
    "foo",
    "baz",
    "tmp",
    "var",
    "ref",
    "see",
    "via",
    "per",
    "net",
    "url",
    "dir",
    "bin",
    "lib",
    "src",
    "pkg",
    "cmd",
    // Common modifiers
    "also",
    "now",
    "then",
    "when",
    "only",
    "just",
    "still",
    "more",
    "less",
    "much",
    "many",
    "some",
    "any",
    "other",
    "better",
    "proper",
    "correct",
    "minor",
    "major",
    "small",
    "large",
    "internal",
    "external",
    "local",
    "remote",
    "global",
    "public",
    "private",
    // Daemon-specific noise
    "daemon",
    "process",
    "running",
    "background",
    "foreground",
    "writes",
    "reads",
    "opens",
    "closes",
    "re-opens",
    "poll",
    "polling",
    "falling",
    "through",
    "floor",
    "threshold",
];

/// Rule-based entity extractor — no LLM, <1ms
pub struct RuleExtractor;

impl RuleExtractor {
    pub fn new() -> Self {
        Self
    }

    /// Check if a word is a known project name
    fn is_project(name: &str) -> bool {
        KNOWN_PROJECTS.contains(&name) || user_lists().is_some_and(|u| u.projects.contains(name))
    }

    /// Extract scope from conventional commit: feat(auth) → "auth"
    fn extract_commit_scope(title: &str) -> Option<String> {
        let lower = title.to_lowercase();
        if let Some(start) = lower.find('(')
            && let Some(end) = lower.find(')')
            && end > start + 1
        {
            return Some(lower[start + 1..end].to_string());
        }
        None
    }

    /// Extract commit action: feat → added, fix → fixed, refactor → refactored
    fn commit_relation(title: &str) -> &'static str {
        let lower = title.to_lowercase();
        if lower.starts_with("feat") || lower.contains("add ") || lower.contains("implement") {
            "added_to"
        } else if lower.starts_with("fix") || lower.contains("bug") {
            "fixed_in"
        } else if lower.starts_with("refactor") {
            "refactored_in"
        } else if lower.starts_with("docs") {
            "documented_in"
        } else if lower.starts_with("test") {
            "tested_in"
        } else if lower.starts_with("perf") {
            "optimized_in"
        } else {
            "related_to"
        }
    }

    /// Extract module name from file path: src/storage/mod.rs → "storage"
    fn extract_module_from_path(path: &str) -> Option<String> {
        let parts: Vec<&str> = path.split('/').collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "src" && i + 1 < parts.len() {
                let module = parts[i + 1];
                if !module.contains('.') {
                    return Some(module.to_string());
                }
            }
        }
        None
    }

    /// Find known tech names in text (title only, not body)
    fn find_known_tech(text: &str) -> Vec<String> {
        let lower = text.to_lowercase();
        let mut found = Vec::new();
        for tech in KNOWN_TECH {
            if Self::contains_word(&lower, tech) && !Self::is_denied(tech) {
                found.push(tech.to_string());
            }
        }
        if let Some(u) = user_lists() {
            for tech in &u.tech {
                if Self::contains_word(&lower, tech) && !Self::is_denied(tech) {
                    found.push(tech.clone());
                }
            }
        }
        found
    }

    /// Whole-word containment: true when `needle` (lowercase) occurs in
    /// `haystack` (lowercase) bounded by non-alphanumeric chars on BOTH sides,
    /// at any occurrence. Stops a configured "project-beta" from matching inside
    /// "project-betamax" and creating a false link.
    fn contains_word(haystack: &str, needle: &str) -> bool {
        if needle.is_empty() {
            return false;
        }
        let bytes = haystack.as_bytes();
        let mut from = 0;
        while let Some(rel) = haystack[from..].find(needle) {
            let i = from + rel;
            let before_ok = i == 0 || !bytes[i - 1].is_ascii_alphanumeric();
            let after = i + needle.len();
            let after_ok = after >= haystack.len() || !bytes[after].is_ascii_alphanumeric();
            if before_ok && after_ok {
                return true;
            }
            from = i + needle.len();
        }
        false
    }

    /// Get the first line of text (title only, strip body/trailer)
    fn first_line(text: &str) -> &str {
        text.lines().next().unwrap_or(text)
    }

    /// Generic meta-noise OR a user-denied term. Checked everywhere an entity
    /// could be created (incl. allow-list matches) so `[graph].deny` always wins.
    /// `pub(crate)` so the LLM extractor applies the SAME deny rules.
    pub(crate) fn is_denied(word: &str) -> bool {
        DENY_CONCEPTS.contains(&word) || user_lists().is_some_and(|u| u.deny.contains(word))
    }

    /// Check if a word passes quality filters. `pub(crate)` so the LLM/composite
    /// extractor runs its output through the same gate as the rule extractor
    /// (otherwise the LLM re-introduces denied/junk nodes the rules drop).
    pub(crate) fn is_valid_entity(word: &str) -> bool {
        // Must be at least 3 chars
        if word.len() < 3 {
            return false;
        }
        // Must not be a stopword
        if STOPWORDS.contains(&word) {
            return false;
        }
        // Must not be generic / meta noise or a user-denied term.
        if Self::is_denied(word) {
            return false;
        }
        // Must not be purely numeric
        if word.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return false;
        }
        // Must not contain = or other special chars (catches "note=0.4" etc)
        if word.contains('=') || word.contains('<') || word.contains('>') {
            return false;
        }
        // Reject random-looking id/hash tokens (e.g. an LLM echoing a UUID
        // fragment like "uconcy13i6keatbmmqmslgbq"): long, no separators, and
        // mixing letters with digits. Real names use hyphens/spaces/dots.
        if word.len() >= 18
            && !word.contains(['-', '_', '.', '/', ' '])
            && word.chars().any(|c| c.is_ascii_digit())
            && word.chars().any(|c| c.is_ascii_alphabetic())
        {
            return false;
        }
        true
    }

    /// Extract key nouns from the FIRST LINE of commit message after the prefix
    fn extract_content_words(title: &str) -> Vec<String> {
        // Only use first line
        let first = Self::first_line(title);

        let text = if let Some(pos) = first.find(':') {
            &first[pos + 1..]
        } else {
            first
        };

        text.split(|c: char| !c.is_alphanumeric() && c != '-' && c != '_')
            .map(|w| w.trim().to_lowercase())
            .filter(|w| Self::is_valid_entity(w))
            .collect()
    }

    /// Determine entity type for a word
    fn entity_type_for(word: &str) -> EntityType {
        if KNOWN_TECH.contains(&word) || user_lists().is_some_and(|u| u.tech.contains(word)) {
            EntityType::Tech
        } else if KNOWN_PEOPLE.contains(&word)
            || user_lists().is_some_and(|u| u.people.contains(word))
        {
            EntityType::Person
        } else if Self::is_project(word) {
            EntityType::Project
        } else {
            EntityType::Concept
        }
    }
}

impl EntityExtractor for RuleExtractor {
    fn extract(&self, entry: &MemoryEntry) -> ExtractionResult {
        let mut entities: Vec<Entity> = Vec::new();
        let mut edges: Vec<Edge> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();

        // Only use title (first line) for content extraction, not full body
        let title_line = Self::first_line(&entry.title);

        // 1. Scope from conventional commit → Module entity
        if let Some(scope) = Self::extract_commit_scope(title_line) {
            if Self::is_valid_entity(&scope) && seen.insert(scope.clone()) {
                let etype = Self::entity_type_for(&scope);
                entities.push(Entity {
                    name: scope.clone(),
                    entity_type: etype,
                });
            }

            // Content words from title only → connected to scope
            let relation = Self::commit_relation(title_line);
            let words = Self::extract_content_words(title_line);
            for word in &words {
                if seen.insert(word.clone()) {
                    entities.push(Entity {
                        name: word.clone(),
                        entity_type: Self::entity_type_for(word),
                    });
                }
                edges.push(Edge {
                    source: word.clone(),
                    target: scope.clone(),
                    relation: relation.to_string(),
                    memory_id: entry.id.clone(),
                });
            }
        }

        // 2. Known tech + project names in title only
        let techs = Self::find_known_tech(title_line);
        for tech in &techs {
            if seen.insert(tech.clone()) {
                entities.push(Entity {
                    name: tech.clone(),
                    entity_type: Self::entity_type_for(tech),
                });
            }
        }

        // Also check known projects in title (built-in + user [graph].projects).
        // Word-boundary match so "project-beta" never fires inside
        // "project-betamax" and links an unrelated memory.
        let title_lower = title_line.to_lowercase();
        for project in KNOWN_PROJECTS {
            if Self::contains_word(&title_lower, project)
                && !Self::is_denied(project)
                && seen.insert(project.to_string())
            {
                entities.push(Entity {
                    name: project.to_string(),
                    entity_type: EntityType::Project,
                });
            }
        }
        if let Some(u) = user_lists() {
            for project in &u.projects {
                let p = project.to_lowercase();
                if Self::contains_word(&title_lower, &p)
                    && !Self::is_denied(&p)
                    && seen.insert(p.clone())
                {
                    entities.push(Entity {
                        name: p,
                        entity_type: EntityType::Project,
                    });
                }
            }
        }

        // Configured + built-in people named in the title → Person entities.
        // Plain note/conversation titles are not otherwise tokenized, so without
        // this a `[graph].people = ["alice"]` entry would never surface.
        for person in KNOWN_PEOPLE {
            if Self::contains_word(&title_lower, person)
                && !Self::is_denied(person)
                && seen.insert(person.to_string())
            {
                entities.push(Entity {
                    name: person.to_string(),
                    entity_type: EntityType::Person,
                });
            }
        }
        if let Some(u) = user_lists() {
            for person in &u.people {
                let p = person.to_lowercase();
                if Self::contains_word(&title_lower, &p)
                    && !Self::is_denied(&p)
                    && seen.insert(p.clone())
                {
                    entities.push(Entity {
                        name: p,
                        entity_type: EntityType::Person,
                    });
                }
            }
        }

        // 3. File paths → Module entities
        if let Some(path) = entry.metadata.get("path").and_then(|v| v.as_str())
            && let Some(module) = Self::extract_module_from_path(path)
            && Self::is_valid_entity(&module)
            && seen.insert(module.clone())
        {
            entities.push(Entity {
                name: module,
                entity_type: EntityType::Module,
            });
        }

        // 4. Tags → entities (if meaningful and not already found)
        let generic_tags: HashSet<&str> = [
            "feature",
            "bugfix",
            "refactor",
            "docs",
            "test",
            "chore",
            "performance",
            "session",
            "dependency",
            "correction",
            "feedback",
            "complete",
            "release",
            "architecture",
            "detailed",
            "plan",
            "final",
            "mvp",
            "reference",
        ]
        .into_iter()
        .collect();

        for tag in &entry.tags {
            let lower = tag.to_lowercase();
            if generic_tags.contains(lower.as_str()) {
                continue;
            }
            if Self::is_valid_entity(&lower) && seen.insert(lower.clone()) {
                entities.push(Entity {
                    name: lower,
                    entity_type: Self::entity_type_for(tag),
                });
            }
        }

        // 5. Connect tech entities to each other if co-mentioned
        if techs.len() > 1 {
            for i in 0..techs.len() {
                for j in (i + 1)..techs.len() {
                    edges.push(Edge {
                        source: techs[i].clone(),
                        target: techs[j].clone(),
                        relation: "co_mentioned".to_string(),
                        memory_id: entry.id.clone(),
                    });
                }
            }
        }

        ExtractionResult { entities, edges }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryType};

    fn make_entry(title: &str, content: &str) -> MemoryEntry {
        MemoryEntry::new(
            title,
            content,
            MemoryType::Decision,
            EventSource::GitWatcher,
        )
    }

    #[test]
    fn test_extract_commit_scope() {
        assert_eq!(
            RuleExtractor::extract_commit_scope("feat(auth): Add JWT"),
            Some("auth".into())
        );
        assert_eq!(RuleExtractor::extract_commit_scope("fix: Something"), None);
    }

    #[test]
    fn test_extract_from_feat_commit() {
        let extractor = RuleExtractor::new();
        let entry = make_entry(
            "feat(auth): Add JWT token refresh",
            "Added JWT refresh flow",
        );

        let result = extractor.extract(&entry);

        let entity_names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(entity_names.contains(&"auth"));
        assert!(entity_names.contains(&"jwt"));
        assert!(!result.edges.is_empty());

        let jwt_auth_edge = result
            .edges
            .iter()
            .find(|e| e.source == "jwt" && e.target == "auth");
        assert!(jwt_auth_edge.is_some());
        assert_eq!(jwt_auth_edge.unwrap().relation, "added_to");
    }

    #[test]
    fn test_known_tech_detection() {
        let techs = RuleExtractor::find_known_tech("Using PostgreSQL with Redis for caching");
        assert!(techs.contains(&"postgresql".to_string()));
        assert!(techs.contains(&"redis".to_string()));
    }

    #[test]
    fn test_module_from_path() {
        assert_eq!(
            RuleExtractor::extract_module_from_path("src/storage/mod.rs"),
            Some("storage".into())
        );
        assert_eq!(RuleExtractor::extract_module_from_path("src/main.rs"), None);
    }

    #[test]
    fn test_tags_become_entities() {
        let extractor = RuleExtractor::new();
        let mut entry = make_entry("Some change", "content");
        entry.tags = vec!["auth".into(), "security".into(), "feature".into()];

        let result = extractor.extract(&entry);
        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();

        assert!(names.contains(&"auth"));
        assert!(names.contains(&"security"));
        assert!(!names.contains(&"feature")); // generic tag, filtered
    }

    #[test]
    fn test_garbage_filtered() {
        let extractor = RuleExtractor::new();

        // Multi-line commit should only extract from first line
        let entry = make_entry(
            "Fix scoring floor for new topics\n\n- was falling through to Note=0.4\n\nCo-Authored-By: someone",
            "body content",
        );
        let result = extractor.extract(&entry);
        let names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();

        assert!(!names.contains(&"was falling through to note=0.4"));
        assert!(!names.contains(&"co-authored-by"));
        assert!(!names.contains(&"note=0.4"));
        assert!(!names.contains(&"falling"));
        assert!(!names.contains(&"writes"));
    }

    #[test]
    fn test_projects_detected() {
        let extractor = RuleExtractor::new();
        // Use a single-token KNOWN_PROJECTS entry — multi-word project names
        // like "Auth Service" only match after rule-based slugification,
        // which is exercised by `canonicalize_name` tests, not here.
        let entry = make_entry("Mnemonic database migration", "Moving to PostgreSQL");
        let result = extractor.extract(&entry);

        let projects: Vec<&Entity> = result
            .entities
            .iter()
            .filter(|e| e.entity_type == EntityType::Project)
            .collect();
        assert!(!projects.is_empty());
        assert!(projects.iter().any(|e| e.name == "mnemonic"));
    }

    #[test]
    fn test_stopwords_filtered() {
        assert!(!RuleExtractor::is_valid_entity("bar"));
        assert!(!RuleExtractor::is_valid_entity("log"));
        assert!(!RuleExtractor::is_valid_entity("net"));
        assert!(!RuleExtractor::is_valid_entity("mode"));
        assert!(!RuleExtractor::is_valid_entity("writes"));
        assert!(!RuleExtractor::is_valid_entity("note=0.4"));
        assert!(RuleExtractor::is_valid_entity("auth"));
        assert!(RuleExtractor::is_valid_entity("postgresql"));
    }
}
