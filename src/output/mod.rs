pub mod memory_api;
pub mod memory_files;
pub mod obsidian;
pub mod whisper;

use crate::config::Config;
use crate::storage::OutputSink;
use tracing::warn;

/// Build the configured output sinks. Shared by the daemon's event loop
/// and the MCP server so every save path writes through the SAME set —
/// the MCP server used to hand-roll a subset (memory_files + obsidian)
/// and silently skip the Memory API sink, so MCP saves never synced.
pub fn build_sinks(config: &Config) -> Vec<Box<dyn OutputSink>> {
    let mut sinks: Vec<Box<dyn OutputSink>> = Vec::new();
    if config.output.memory_files_enabled {
        sinks.push(Box::new(memory_files::MemoryFileSink::new(
            config.output.memory_files_path.clone(),
        )));
    }
    if config.output.obsidian_enabled {
        sinks.push(Box::new(obsidian::ObsidianSink::new(
            config.output.obsidian_path.clone(),
        )));
    }
    if config.output.memory_api_enabled && !config.output.memory_api_url.is_empty() {
        let memory_api_key = config.get_memory_api_key();
        if memory_api_key.trim().is_empty() {
            warn!("Memory API key missing — sync disabled");
        } else {
            sinks.push(Box::new(memory_api::MemoryApiSink::new(
                config.output.memory_api_url.clone(),
                memory_api_key,
            )));
        }
    }
    sinks
}
