use anyhow::Result;
use std::path::PathBuf;
use tracing::debug;

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
        std::fs::create_dir_all(&notes_dir)?;

        let date = entry.timestamp.format("%Y-%m-%d");
        let slug = Self::slug(&entry.title);
        let filename = format!("{date}-{slug}.md");
        let path = notes_dir.join(&filename);

        let content = Self::entry_to_markdown(entry);
        std::fs::write(&path, content)?;

        debug!("Wrote obsidian note: {}", path.display());
        Ok(())
    }

    fn name(&self) -> &str {
        "obsidian"
    }
}
