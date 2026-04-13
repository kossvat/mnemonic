use anyhow::Result;
use tracing::{debug, warn};

use crate::event::MemoryEntry;
use crate::storage::OutputSink;

/// Sends memory entries to the shared Memory API (MagicBox)
/// so all OpenClaw agents (Vibe, Caramel, etc.) can access them.
pub struct MemoryApiSink {
    url: String,
    api_key: String,
    client: reqwest::blocking::Client,
}

impl MemoryApiSink {
    pub fn new(url: String, api_key: String) -> Self {
        let client = reqwest::blocking::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        Self {
            url,
            api_key,
            client,
        }
    }
}

impl OutputSink for MemoryApiSink {
    fn write(&self, entry: &MemoryEntry) -> Result<()> {
        let tags = entry.tags.join(",");
        let body = serde_json::json!({
            "content": entry.content,
            "title": entry.title,
            "memory_type": entry.memory_type.to_string(),
            "tags": tags,
            "write_obsidian": true,
        });

        let resp = self
            .client
            .post(format!("{}/save", self.url))
            .header("Authorization", format!("Bearer {}", self.api_key))
            .header("Content-Type", "application/json")
            .json(&body)
            .send();

        match resp {
            Ok(r) if r.status().is_success() => {
                debug!("Memory API: saved '{}'", entry.title);
            }
            Ok(r) => {
                warn!("Memory API error {}: {}", r.status(), entry.title);
            }
            Err(e) => {
                warn!("Memory API unreachable: {e}");
            }
        }

        // Never fail the pipeline — API is best-effort
        Ok(())
    }

    fn name(&self) -> &str {
        "memory_api"
    }
}
