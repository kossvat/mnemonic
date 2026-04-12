use crate::config::Config;
use crate::embedding::{Embedder, HashEmbedder};
use crate::event::{EventSource, MemoryEntry, MemoryType};
use crate::output::whisper::Whisper;
use crate::scoring::ImportanceScorer;
use crate::storage::{OutputSink, Storage};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::io::{self, BufRead, Write};

/// MCP Server — JSON-RPC 2.0 over stdio
/// Protocol: https://modelcontextprotocol.io/specification
pub struct McpServer {
    config: Config,
}

#[derive(Deserialize)]
struct JsonRpcRequest {
    #[allow(dead_code)]
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    #[serde(default)]
    params: Value,
}

#[derive(Serialize)]
struct JsonRpcResponse {
    jsonrpc: String,
    id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<JsonRpcError>,
}

#[derive(Serialize)]
struct JsonRpcError {
    code: i32,
    message: String,
}

impl McpServer {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub fn run(&self) -> Result<()> {
        let storage = Storage::open(&self.config.storage.db_path)?;
        let embedder = HashEmbedder::new();
        let scorer = ImportanceScorer::default();

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        // Read line-delimited JSON-RPC
        for line in stdin.lock().lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };

            if line.trim().is_empty() {
                continue;
            }

            let request: JsonRpcRequest = match serde_json::from_str(&line) {
                Ok(r) => r,
                Err(e) => {
                    let err = JsonRpcResponse {
                        jsonrpc: "2.0".into(),
                        id: Value::Null,
                        result: None,
                        error: Some(JsonRpcError {
                            code: -32700,
                            message: format!("Parse error: {e}"),
                        }),
                    };
                    writeln!(stdout, "{}", serde_json::to_string(&err)?)?;
                    stdout.flush()?;
                    continue;
                }
            };

            let id = request.id.clone().unwrap_or(Value::Null);
            let response = self.handle_request(&request, &storage, &embedder, &scorer);

            let resp = match response {
                Ok(result) => JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id,
                    result: Some(result),
                    error: None,
                },
                Err(e) => JsonRpcResponse {
                    jsonrpc: "2.0".into(),
                    id,
                    result: None,
                    error: Some(JsonRpcError {
                        code: -32603,
                        message: e.to_string(),
                    }),
                },
            };

            writeln!(stdout, "{}", serde_json::to_string(&resp)?)?;
            stdout.flush()?;
        }

        Ok(())
    }

    fn handle_request(
        &self,
        req: &JsonRpcRequest,
        storage: &Storage,
        embedder: &HashEmbedder,
        scorer: &ImportanceScorer,
    ) -> Result<Value> {
        match req.method.as_str() {
            // MCP protocol methods
            "initialize" => self.handle_initialize(),
            "tools/list" => self.handle_tools_list(),
            "tools/call" => self.handle_tools_call(&req.params, storage, embedder, scorer),

            // Direct methods (for non-MCP clients)
            "memory_search" => self.handle_search(&req.params, storage),
            "memory_save" => self.handle_save(&req.params, storage, embedder, scorer),
            "memory_recent" => self.handle_recent(&req.params, storage),
            "memory_similar" => self.handle_similar(&req.params, storage, embedder),
            "memory_context" => self.handle_context(&req.params, storage),
            "memory_status" => self.handle_status(storage),

            _ => Err(anyhow::anyhow!("Unknown method: {}", req.method)),
        }
    }

    fn handle_initialize(&self) -> Result<Value> {
        Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {
                "tools": {}
            },
            "serverInfo": {
                "name": "mnemonic",
                "version": env!("CARGO_PKG_VERSION")
            }
        }))
    }

    fn handle_tools_list(&self) -> Result<Value> {
        Ok(json!({
            "tools": [
                {
                    "name": "memory_search",
                    "description": "Full-text search across all memories",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string", "description": "Search text"},
                            "limit": {"type": "integer", "description": "Max results (default: 10)"}
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "memory_save",
                    "description": "Save a new memory entry with automatic dedup and importance scoring",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string", "description": "Short title"},
                            "content": {"type": "string", "description": "Memory content"},
                            "memory_type": {"type": "string", "enum": ["decision", "feedback", "note", "session_summary"], "description": "Type (default: note)"},
                            "tags": {"type": "string", "description": "Comma-separated tags"}
                        },
                        "required": ["title", "content"]
                    }
                },
                {
                    "name": "memory_recent",
                    "description": "Get most recent memories",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type": "integer", "description": "Max results (default: 10)"}
                        }
                    }
                },
                {
                    "name": "memory_similar",
                    "description": "Find semantically similar memories",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "query": {"type": "string", "description": "Search text"},
                            "limit": {"type": "integer", "description": "Max results (default: 5)"}
                        },
                        "required": ["query"]
                    }
                },
                {
                    "name": "memory_context",
                    "description": "Generate context summary with relevant memories (Whisper)",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "topic": {"type": "string", "description": "Optional topic to focus on"}
                        }
                    }
                },
                {
                    "name": "memory_status",
                    "description": "Get daemon status and memory stats",
                    "inputSchema": {
                        "type": "object",
                        "properties": {}
                    }
                }
            ]
        }))
    }

    fn handle_tools_call(
        &self,
        params: &Value,
        storage: &Storage,
        embedder: &HashEmbedder,
        scorer: &ImportanceScorer,
    ) -> Result<Value> {
        let tool_name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;

        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = match tool_name {
            "memory_search" => self.handle_search(&arguments, storage)?,
            "memory_save" => self.handle_save(&arguments, storage, embedder, scorer)?,
            "memory_recent" => self.handle_recent(&arguments, storage)?,
            "memory_similar" => self.handle_similar(&arguments, storage, embedder)?,
            "memory_context" => self.handle_context(&arguments, storage)?,
            "memory_status" => self.handle_status(storage)?,
            _ => return Err(anyhow::anyhow!("Unknown tool: {tool_name}")),
        };

        Ok(json!({
            "content": [{
                "type": "text",
                "text": serde_json::to_string_pretty(&result)?
            }]
        }))
    }

    fn handle_search(&self, params: &Value, storage: &Storage) -> Result<Value> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query'"))?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;

        let results = storage.search(query, limit)?;
        let entries: Vec<Value> = results.iter().map(entry_to_json).collect();

        Ok(json!({
            "results": entries,
            "count": entries.len()
        }))
    }

    fn handle_save(
        &self,
        params: &Value,
        storage: &Storage,
        embedder: &HashEmbedder,
        scorer: &ImportanceScorer,
    ) -> Result<Value> {
        let title = params
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'title'"))?;
        let content = params
            .get("content")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'content'"))?;
        let memory_type = params
            .get("memory_type")
            .and_then(|v| v.as_str())
            .unwrap_or("note");
        let tags_str = params.get("tags").and_then(|v| v.as_str()).unwrap_or("");

        let mt = match memory_type {
            "decision" => MemoryType::Decision,
            "feedback" => MemoryType::Feedback,
            "session_summary" => MemoryType::SessionSummary,
            _ => MemoryType::Note,
        };

        let tag_list: Vec<String> = tags_str
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let mut entry = MemoryEntry::new(title, content, mt.clone(), EventSource::Socket);
        entry.tags = tag_list;

        // Embedding + dedup + scoring
        let embed_text = format!("{} {}", title, content);
        if let Ok(emb) = embedder.embed(&embed_text) {
            if let Ok(Some(sim)) =
                storage.is_duplicate(&emb, self.config.classifier.dedup_threshold)
            {
                return Ok(json!({
                    "status": "skipped",
                    "reason": "duplicate",
                    "similarity": sim
                }));
            }

            if let Ok(score) = scorer.score(
                &emb,
                &crate::event::EventKind::Custom("mcp".into()),
                &mt,
                &storage.conn,
            ) {
                entry.importance = score;
            }

            storage.save_with_embedding(&entry, Some(&emb))?;
        } else {
            entry.importance = 0.7;
            storage.save(&entry)?;
        }

        // Write to output sinks
        if self.config.output.memory_files_enabled {
            let sink = crate::output::memory_files::MemoryFileSink::new(
                self.config.output.memory_files_path.clone(),
            );
            let _ = sink.write(&entry);
        }
        if self.config.output.obsidian_enabled {
            let sink = crate::output::obsidian::ObsidianSink::new(
                self.config.output.obsidian_path.clone(),
            );
            let _ = sink.write(&entry);
        }

        Ok(json!({
            "status": "saved",
            "id": entry.id,
            "importance": entry.importance,
            "memory_type": entry.memory_type.to_string()
        }))
    }

    fn handle_recent(&self, params: &Value, storage: &Storage) -> Result<Value> {
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let results = storage.recent(limit)?;
        let entries: Vec<Value> = results.iter().map(entry_to_json).collect();

        Ok(json!({
            "results": entries,
            "count": entries.len()
        }))
    }

    fn handle_similar(
        &self,
        params: &Value,
        storage: &Storage,
        embedder: &HashEmbedder,
    ) -> Result<Value> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query'"))?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        let emb = embedder.embed(query)?;
        let results = storage.find_similar(&emb, limit)?;
        let entries: Vec<Value> = results
            .iter()
            .map(|(entry, sim)| {
                let mut j = entry_to_json(entry);
                j.as_object_mut()
                    .unwrap()
                    .insert("similarity".into(), json!(sim));
                j
            })
            .collect();

        Ok(json!({
            "results": entries,
            "count": entries.len()
        }))
    }

    fn handle_context(&self, params: &Value, storage: &Storage) -> Result<Value> {
        let topic = params.get("topic").and_then(|v| v.as_str());

        let output_path = self.config.output.memory_files_path.join("CONTEXT.md");
        let whisper = Whisper::new(output_path);

        let content = match topic {
            Some(t) => whisper.generate_for_topic(storage, t, 10)?,
            None => whisper.generate(storage)?,
        };

        Ok(json!({
            "context": content
        }))
    }

    fn handle_status(&self, storage: &Storage) -> Result<Value> {
        let stats = storage.stats()?;
        let is_running = crate::daemon::Daemon::is_running(&self.config);

        Ok(json!({
            "daemon_running": is_running.is_some(),
            "daemon_pid": is_running,
            "total_memories": stats.total,
            "by_type": stats.by_type.iter().map(|(t, c)| json!({"type": t, "count": c})).collect::<Vec<_>>(),
            "db_path": self.config.storage.db_path.to_string_lossy()
        }))
    }
}

fn entry_to_json(entry: &MemoryEntry) -> Value {
    json!({
        "id": entry.id,
        "title": entry.title,
        "content": entry.content,
        "memory_type": entry.memory_type.to_string(),
        "tags": entry.tags,
        "importance": entry.importance,
        "timestamp": entry.timestamp.to_rfc3339(),
    })
}
