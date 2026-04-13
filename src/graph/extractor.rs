use crate::event::MemoryEntry;
use crate::graph::{Edge, Entity, EntityType};
use std::collections::HashSet;

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
    "rust", "python", "typescript", "javascript", "go", "java", "swift",
    "react", "nextjs", "next.js", "vue", "svelte", "angular",
    "postgresql", "postgres", "sqlite", "mongodb", "redis", "mysql",
    "docker", "kubernetes", "k8s", "nginx", "caddy",
    "tokio", "axum", "hyper", "actix", "warp",
    "fastapi", "flask", "django", "express", "nestjs", "hono",
    "tailwind", "shadcn", "swiftui",
    "vercel", "cloudflare", "aws", "gcp",
    "jwt", "oauth", "ssh", "tls", "ssl",
    "git", "github", "gitlab",
    "telegram", "slack", "discord",
    "openai", "anthropic", "claude", "gemini", "elevenlabs",
    "twilio", "stripe",
    "chromadb", "lancedb", "cozodb", "pinecone",
    "mcp", "grpc", "graphql", "rest",
    "supabase", "firebase", "prisma",
];

/// Known project names → EntityType::Project
const KNOWN_PROJECTS: &[&str] = &[
    "agentcrm", "agent-crm",
    "agentforgeai", "agent-forge-ai",
    "mnemonic",
    "luna", "luna-voice",
    "pixel-office", "pixeloffice",
    "openclaw",
];

/// Words that should never become entities
const STOPWORDS: &[&str] = &[
    // English function words
    "a", "an", "the", "and", "or", "but", "in", "on", "at", "to", "for",
    "of", "with", "by", "from", "is", "are", "was", "were", "be", "been",
    "being", "have", "has", "had", "do", "does", "did", "will", "would",
    "could", "should", "may", "might", "shall", "can", "need", "must",
    "not", "all", "each", "every", "this", "that", "it", "its",
    // Common verbs in commits
    "add", "fix", "update", "remove", "delete", "change", "modify",
    "implement", "refactor", "resolve", "use", "set", "get", "make",
    "move", "rename", "merge", "revert", "apply", "handle", "ensure",
    "check", "verify", "enable", "disable", "allow", "prevent",
    "create", "write", "read", "run", "stop", "start", "init",
    "support", "include", "exclude", "skip", "show", "hide",
    "bump", "prepare", "release", "deploy", "publish", "ship",
    // Common nouns that are too generic
    "new", "old", "file", "files", "code", "data", "type", "types",
    "name", "value", "key", "list", "item", "items", "path", "error",
    "bug", "issue", "test", "tests", "docs", "doc", "readme",
    "config", "default", "option", "options", "setting", "settings",
    "mode", "state", "status", "result", "output", "input",
    "log", "logs", "message", "messages", "event", "events",
    "version", "number", "count", "index", "size", "length",
    "first", "last", "next", "prev", "previous", "current",
    "main", "base", "core", "common", "util", "utils", "helper",
    "info", "warn", "debug", "trace",
    // Git noise
    "co-authored-by", "signed-off-by", "commit", "branch", "pull",
    "request", "review", "merge",
    // Too short / meaningless
    "bar", "foo", "baz", "tmp", "var", "ref", "see", "via", "per",
    "net", "url", "dir", "bin", "lib", "src", "pkg", "cmd",
    // Common modifiers
    "also", "now", "then", "when", "only", "just", "still",
    "more", "less", "much", "many", "some", "any", "other",
    "better", "proper", "correct", "minor", "major", "small", "large",
    "internal", "external", "local", "remote", "global", "public", "private",
    // Daemon-specific noise
    "daemon", "process", "running", "background", "foreground",
    "writes", "reads", "opens", "closes", "re-opens", "poll", "polling",
    "falling", "through", "floor", "threshold",
];

/// Rule-based entity extractor — no LLM, <1ms
pub struct RuleExtractor;

impl RuleExtractor {
    pub fn new() -> Self {
        Self
    }

    /// Check if a word is a known project name
    fn is_project(name: &str) -> bool {
        KNOWN_PROJECTS.contains(&name)
    }

    /// Extract scope from conventional commit: feat(auth) → "auth"
    fn extract_commit_scope(title: &str) -> Option<String> {
        let lower = title.to_lowercase();
        if let Some(start) = lower.find('(') {
            if let Some(end) = lower.find(')') {
                if end > start + 1 {
                    return Some(lower[start + 1..end].to_string());
                }
            }
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
            if let Some(pos) = lower.find(tech) {
                let before_ok = pos == 0
                    || !lower.as_bytes()[pos - 1].is_ascii_alphanumeric();
                let after_pos = pos + tech.len();
                let after_ok = after_pos >= lower.len()
                    || !lower.as_bytes()[after_pos].is_ascii_alphanumeric();
                if before_ok && after_ok {
                    found.push(tech.to_string());
                }
            }
        }
        found
    }

    /// Get the first line of text (title only, strip body/trailer)
    fn first_line(text: &str) -> &str {
        text.lines().next().unwrap_or(text)
    }

    /// Check if a word passes quality filters
    fn is_valid_entity(word: &str) -> bool {
        // Must be at least 3 chars
        if word.len() < 3 {
            return false;
        }
        // Must not be a stopword
        if STOPWORDS.contains(&word) {
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
        if KNOWN_TECH.contains(&word) {
            EntityType::Tech
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

        // Also check known projects in title
        let title_lower = title_line.to_lowercase();
        for project in KNOWN_PROJECTS {
            if title_lower.contains(project) && seen.insert(project.to_string()) {
                entities.push(Entity {
                    name: project.to_string(),
                    entity_type: EntityType::Project,
                });
            }
        }

        // 3. File paths → Module entities
        if let Some(path) = entry.metadata.get("path").and_then(|v| v.as_str()) {
            if let Some(module) = Self::extract_module_from_path(path) {
                if Self::is_valid_entity(&module) && seen.insert(module.clone()) {
                    entities.push(Entity {
                        name: module,
                        entity_type: EntityType::Module,
                    });
                }
            }
        }

        // 4. Tags → entities (if meaningful and not already found)
        let generic_tags: HashSet<&str> = [
            "feature", "bugfix", "refactor", "docs", "test", "chore",
            "performance", "session", "dependency", "correction", "feedback",
            "complete", "release", "architecture", "detailed", "plan",
            "final", "mvp", "reference",
        ].into_iter().collect();

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
        MemoryEntry::new(title, content, MemoryType::Decision, EventSource::GitWatcher)
    }

    #[test]
    fn test_extract_commit_scope() {
        assert_eq!(
            RuleExtractor::extract_commit_scope("feat(auth): Add JWT"),
            Some("auth".into())
        );
        assert_eq!(
            RuleExtractor::extract_commit_scope("fix: Something"),
            None
        );
    }

    #[test]
    fn test_extract_from_feat_commit() {
        let extractor = RuleExtractor::new();
        let entry = make_entry("feat(auth): Add JWT token refresh", "Added JWT refresh flow");

        let result = extractor.extract(&entry);

        let entity_names: Vec<&str> = result.entities.iter().map(|e| e.name.as_str()).collect();
        assert!(entity_names.contains(&"auth"));
        assert!(entity_names.contains(&"jwt"));
        assert!(!result.edges.is_empty());

        let jwt_auth_edge = result.edges.iter().find(|e| e.source == "jwt" && e.target == "auth");
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
        assert_eq!(
            RuleExtractor::extract_module_from_path("src/main.rs"),
            None
        );
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
            "body content"
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
        let entry = make_entry("AgentCRM database migration", "Moving to PostgreSQL");
        let result = extractor.extract(&entry);

        let projects: Vec<&Entity> = result.entities.iter()
            .filter(|e| e.entity_type == EntityType::Project)
            .collect();
        assert!(!projects.is_empty());
        assert!(projects.iter().any(|e| e.name == "agentcrm"));
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
