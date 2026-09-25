use anyhow::Result;
use std::path::PathBuf;
use tracing::debug;

use super::file_export;
use crate::event::MemoryEntry;
use crate::storage::OutputSink;

/// Writes memory entries to Obsidian vault as markdown files
pub struct ObsidianSink {
    vault_path: PathBuf,
}

impl ObsidianSink {
    pub fn new(vault_path: PathBuf) -> Self {
        Self { vault_path }
    }

    fn entry_to_markdown(entry: &MemoryEntry) -> String {
        let date = entry.timestamp.format("%Y-%m-%d");
        let tags_str = entry
            .tags
            .iter()
            .map(|t| format!("#{t}"))
            .collect::<Vec<_>>()
            .join(" ");

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
             {}\n\
             \n\
             {}\n",
            entry.title.replace('"', "'"),
            entry.memory_type,
            date,
            entry.tags.join(", "),
            entry.importance,
            entry.content,
            tags_str,
        )
    }

    pub fn slug_for(title: &str) -> String {
        Self::slug(title)
    }

    /// Stable identity suffix prevents distinct memories with the same title
    /// from overwriting each other. Legacy title-only files are left untouched.
    pub fn filename_for(entry: &MemoryEntry) -> Result<String> {
        file_export::filename_for(entry, &Self::slug_for(&entry.title))
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

impl OutputSink for ObsidianSink {
    fn write(&self, entry: &MemoryEntry) -> Result<()> {
        let notes_dir = self.vault_path.join("Agents/Mnemonic/Notes");
        // In an isolated profile a symlink planted under the vault folder must
        // not carry memories out of it.
        crate::config::Config::ensure_profile_output(&notes_dir)?;
        std::fs::create_dir_all(&notes_dir)?;

        let filename = Self::filename_for(entry)?;
        let path = notes_dir.join(&filename);
        crate::config::Config::ensure_profile_output(&path)?;

        let content = Self::entry_to_markdown(entry);
        file_export::write_atomic(&path, &content)?;

        debug!("Wrote obsidian note: {}", path.display());
        Ok(())
    }

    fn name(&self) -> &str {
        "obsidian"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event::{EventSource, MemoryType};

    #[test]
    fn distinct_ids_survive_same_title_and_repeated_exports() {
        let tmp = crate::test_support::temp_dir("mnemonic-obsidian-");
        let root = tmp.path();
        let sink = ObsidianSink::new(root.to_path_buf());
        let mut first = MemoryEntry::new(
            "Shared title",
            "First body",
            MemoryType::Note,
            EventSource::Manual,
        );
        first.id = "00000000-0000-4000-8000-000000000001".into();
        let mut second = first.clone();
        second.id = "00000000-0000-4000-8000-000000000002".into();
        second.content = "Second body".into();
        let notes = root.join("Agents/Mnemonic/Notes");
        std::fs::create_dir_all(&notes).unwrap();
        let legacy = notes.join(format!(
            "{}-shared-title.md",
            first.timestamp.format("%Y-%m-%d")
        ));
        std::fs::write(&legacy, "Keep legacy note").unwrap();
        sink.write(&first).unwrap();
        sink.write(&second).unwrap();
        first.content = "Updated first body".into();
        sink.write(&first).unwrap();
        assert_eq!(std::fs::read_dir(&notes).unwrap().count(), 3);
        assert!(
            std::fs::read_to_string(notes.join(ObsidianSink::filename_for(&first).unwrap()))
                .unwrap()
                .contains("Updated first body")
        );
        assert!(
            std::fs::read_to_string(notes.join(ObsidianSink::filename_for(&second).unwrap()))
                .unwrap()
                .contains("Second body")
        );
        assert_eq!(std::fs::read_to_string(legacy).unwrap(), "Keep legacy note");
    }

    #[test]
    fn unicode_and_unsafe_titles_remain_bounded_single_filenames() {
        let entry = MemoryEntry::new(
            format!("../../\\\n{}", "𐐀".repeat(80)),
            "Body",
            MemoryType::Note,
            EventSource::Manual,
        );
        let filename = ObsidianSink::filename_for(&entry).unwrap();
        assert!(filename.len() <= 240);
        assert_eq!(std::path::Path::new(&filename).components().count(), 1);
        assert!(!filename.contains(['/', '\\', '\n']));
        assert!(filename.ends_with(&format!("-{}.md", entry.id)));
        let tmp = crate::test_support::temp_dir("mnemonic-obsidian-");
        let root = tmp.path();
        let sink = ObsidianSink::new(root.to_path_buf());
        sink.write(&entry).unwrap();
        sink.write(&entry).unwrap();
        assert_eq!(
            std::fs::read_dir(root.join("Agents/Mnemonic/Notes"))
                .unwrap()
                .count(),
            1
        );
    }
}
