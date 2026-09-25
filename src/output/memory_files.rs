use anyhow::Result;
use std::path::PathBuf;
use tracing::debug;

use super::file_export;
use crate::event::MemoryEntry;
use crate::storage::OutputSink;

/// Writes memory entries as markdown files compatible with Claude Code auto-memory
pub struct MemoryFileSink {
    base_path: PathBuf,
}

impl MemoryFileSink {
    pub fn new(base_path: PathBuf) -> Self {
        Self { base_path }
    }

    fn resolve_project_memory_dir(&self) -> Result<PathBuf> {
        // Try to find the current project's memory directory
        // Claude Code uses: ~/.claude/projects/-{encoded-path}/memory/
        let cwd = std::env::current_dir()?;
        let encoded = cwd
            .to_string_lossy()
            .replace('/', "-")
            .trim_start_matches('-')
            .to_string();

        let memory_dir = self.base_path.join(format!("-{encoded}")).join("memory");
        // In an isolated profile a symlink planted under the sink folder must
        // not carry memories out of it.
        crate::config::Config::ensure_profile_output(&memory_dir)?;
        std::fs::create_dir_all(&memory_dir)?;
        Ok(memory_dir)
    }

    fn entry_to_markdown(entry: &MemoryEntry) -> String {
        let date = entry.timestamp.format("%Y-%m-%d");
        let tags_str = entry.tags.join(", ");

        format!(
            "---\n\
             title: \"{}\"\n\
             type: {}\n\
             date: {}\n\
             tags: [{}]\n\
             importance: {:.1}\n\
             source: mnemonic\n\
             ---\n\
             \n\
             {}\n",
            entry.title.replace('"', "'"),
            entry.memory_type,
            date,
            tags_str,
            entry.importance,
            entry.content,
        )
    }

    fn slug(title: &str) -> String {
        title
            .to_lowercase()
            .chars()
            .map(|c| {
                if c.is_alphanumeric() || c == '-' {
                    c
                } else if c == ' ' {
                    '-'
                } else {
                    '_'
                }
            })
            .collect::<String>()
            .chars()
            .take(60)
            .collect()
    }
}

impl OutputSink for MemoryFileSink {
    fn write(&self, entry: &MemoryEntry) -> Result<()> {
        let dir = self.resolve_project_memory_dir()?;
        let filename = file_export::filename_for(entry, &Self::slug(&entry.title))?;
        let path = dir.join(&filename);
        crate::config::Config::ensure_profile_output(&path)?;

        let content = Self::entry_to_markdown(entry);
        file_export::write_atomic(&path, &content)?;

        debug!("Wrote memory file: {}", path.display());
        Ok(())
    }

    fn name(&self) -> &str {
        "memory_files"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryType};

    #[test]
    fn test_entry_to_markdown() {
        let entry = MemoryEntry {
            id: "test-id".into(),
            timestamp: chrono::Utc::now(),
            title: "Add JWT auth".into(),
            content: "Decided to use JWT with refresh tokens for API auth".into(),
            memory_type: MemoryType::Decision,
            tags: vec!["auth".into(), "feature".into()],
            source: EventSource::GitWatcher,
            importance: 0.7,
            metadata: serde_json::Value::Null,
        };

        let md = MemoryFileSink::entry_to_markdown(&entry);
        assert!(md.contains("title: \"Add JWT auth\""));
        assert!(md.contains("type: decision"));
        assert!(md.contains("source: mnemonic"));
        assert!(md.contains("JWT with refresh tokens"));
    }

    #[test]
    fn test_slug() {
        assert_eq!(MemoryFileSink::slug("Add JWT auth"), "add-jwt-auth");
        assert_eq!(
            MemoryFileSink::slug("feat(api): implement OAuth"),
            "feat_api__-implement-oauth"
        );
    }

    #[test]
    fn same_slug_memories_have_distinct_stable_files() {
        let root = crate::test_support::temp_dir("mnemonic-memory-files-");
        let sink = MemoryFileSink::new(root.path().to_path_buf());
        let mut first = MemoryEntry::new(
            format!("../{}", "𐐀".repeat(80)),
            "First body",
            MemoryType::Note,
            EventSource::Manual,
        );
        first.id = "00000000-0000-4000-8000-000000000001".into();
        let mut second = first.clone();
        second.id = "00000000-0000-4000-8000-000000000002".into();
        second.content = "Second body".into();
        sink.write(&first).unwrap();
        sink.write(&second).unwrap();
        first.content = "Updated first body".into();
        sink.write(&first).unwrap();
        let dir = sink.resolve_project_memory_dir().unwrap();
        let files: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect();
        assert_eq!(files.len(), 2);
        for path in &files {
            assert!(path.file_name().unwrap().to_str().unwrap().len() <= 240);
            assert_eq!(path.extension().unwrap(), "md");
        }
        let first_path = dir
            .join(file_export::filename_for(&first, &MemoryFileSink::slug(&first.title)).unwrap());
        let second_path = dir.join(
            file_export::filename_for(&second, &MemoryFileSink::slug(&second.title)).unwrap(),
        );
        assert!(
            std::fs::read_to_string(first_path)
                .unwrap()
                .contains("Updated first body")
        );
        assert!(
            std::fs::read_to_string(second_path)
                .unwrap()
                .contains("Second body")
        );
    }
}
