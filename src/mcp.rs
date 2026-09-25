use crate::config::Config;
use crate::embedding::Embedder;
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
        // Embedding strategy (memory-reviewed with Codex):
        // 1. Ask the DAEMON over its unix socket — it already holds the one
        //    resident model copy, so this process never loads ~2 GB.
        // 2. If the daemon is unreachable, fall back to a lazy in-process
        //    model (loaded only when an embed actually happens).
        // 3. If the daemon's vectors don't match the store's dimension
        //    (split-brain after an upgrade), fail hard — never mix vectors.
        let expected_dim = {
            let dims = storage.active_embedding_dims();
            if dims.len() == 1 { Some(dims[0]) } else { None }
        };
        let embedder = crate::embedding::daemon_client::FallbackEmbedder::new(
            self.config.daemon.socket_path.clone(),
            expected_dim,
        );
        let scorer = ImportanceScorer::default();
        // Same sinks + peer attribution as the daemon's event loop, so a
        // memory saved over MCP is indistinguishable from one the daemon
        // captured: it syncs to every configured sink and is visible to
        // peer-filtered retrieval. Previously MCP hand-rolled two of the
        // sinks and skipped attribution entirely.
        let sinks = crate::output::build_sinks(&self.config);
        let attributor = if self.config.peers.auto_tag {
            match crate::daemon::PeerAttributor::init(&storage, &self.config.peers) {
                Ok(a) => Some(a),
                Err(e) => {
                    tracing::warn!("MCP: peer attributor init failed, saves untagged: {e}");
                    None
                }
            }
        } else {
            None
        };

        let stdin = io::stdin();
        let mut stdout = io::stdout();

        // Read line-delimited JSON-RPC
        for line in stdin.lock().lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => break,
            };
            if let Some(out) = self.response_for_line(
                &line,
                &storage,
                &embedder,
                &scorer,
                attributor.as_ref(),
                &sinks,
            ) {
                writeln!(stdout, "{out}")?;
                stdout.flush()?;
            }
        }

        Ok(())
    }

    /// Handle one line of input. Returns the serialized response to write,
    /// or `None` when the line demands no reply (blank lines and — per
    /// JSON-RPC 2.0 — notifications). Split out of `run` so the protocol
    /// behavior is unit-testable without faking stdio.
    fn response_for_line(
        &self,
        line: &str,
        storage: &Storage,
        embedder: &dyn Embedder,
        scorer: &ImportanceScorer,
        attributor: Option<&crate::daemon::PeerAttributor>,
        sinks: &[Box<dyn OutputSink>],
    ) -> Option<String> {
        if line.trim().is_empty() {
            return None;
        }

        let request: JsonRpcRequest = match serde_json::from_str(line) {
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
                return serde_json::to_string(&err).ok();
            }
        };

        let response = self.handle_request(&request, storage, embedder, scorer, attributor, sinks);

        // JSON-RPC 2.0: a request without an id is a NOTIFICATION — the
        // server MUST NOT reply, not even with an error. MCP clients send
        // notifications/initialized, notifications/cancelled, etc.;
        // answering them (previously with id:null "Unknown method" errors)
        // violates the protocol and trips strict clients.
        if request.id.is_none() {
            if let Err(e) = response {
                tracing::debug!(
                    "MCP notification {} handler error (no reply sent): {e}",
                    request.method
                );
            }
            return None;
        }

        let id = request.id.clone().unwrap_or(Value::Null);
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
        serde_json::to_string(&resp).ok()
    }

    fn handle_request(
        &self,
        req: &JsonRpcRequest,
        storage: &Storage,
        embedder: &dyn Embedder,
        scorer: &ImportanceScorer,
        attributor: Option<&crate::daemon::PeerAttributor>,
        sinks: &[Box<dyn OutputSink>],
    ) -> Result<Value> {
        match req.method.as_str() {
            // MCP protocol methods
            "initialize" => self.handle_initialize(),
            "tools/list" => self.handle_tools_list(),
            "tools/call" => {
                self.handle_tools_call(&req.params, storage, embedder, scorer, attributor, sinks)
            }

            // Protocol notifications (initialized, cancelled, …) are
            // valid traffic, not unknown methods. No work to do for any
            // of them today; the notification check in response_for_line
            // already suppresses the reply.
            m if m.starts_with("notifications/") => Ok(Value::Null),

            // Direct methods (for non-MCP clients)
            "memory_search" => self.handle_search(&req.params, storage),
            "memory_save" => {
                self.handle_save(&req.params, storage, embedder, scorer, attributor, sinks)
            }
            "memory_recent" => self.handle_recent(&req.params, storage),
            "memory_similar" => self.handle_similar(&req.params, storage, embedder),
            "memory_context" => self.handle_context(&req.params, storage),
            "memory_status" => self.handle_status(storage),
            "memory_graph" => self.handle_graph(&req.params, storage),

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
        let mut list = json!({
            "tools": [
                {
                    "name": "memory_search",
                    "description": "Full-text search across all memories. A hit a newer memory replaced carries replaced_by (the memory that holds now); a hit that stated a fact carries facts (this value, the current one, is_current).",
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
                    "description": "Save a new memory entry with automatic dedup and importance scoring. A near duplicate that states a changed value (price, commission, terms, deadline, a limit) is saved and linked as an update of the old one; the response lists it under `updates`. A skipped save names the memory it duplicates in `duplicate_of`.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "title": {"type": "string", "description": "Short title"},
                            "content": {"type": "string", "description": "Memory content"},
                            "memory_type": {"type": "string", "enum": ["decision", "feedback", "note", "session_summary"], "description": "Type (default: note)"},
                            "tags": {"type": "string", "description": "Comma-separated tags"},
                            "project": {"type": "string", "description": "Project this memory belongs to. Values saved under different projects never replace each other."}
                        },
                        "required": ["title", "content"]
                    }
                },
                {
                    "name": "memory_recent",
                    "description": "Get most recent memories. A hit a newer memory replaced carries replaced_by (the memory that holds now); a hit that stated a fact carries facts (this value, the current one, is_current).",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "limit": {"type": "integer", "description": "Max results (default: 10)"}
                        }
                    }
                },
                {
                    "name": "memory_similar",
                    "description": "Find semantically similar memories. A hit a newer memory replaced carries replaced_by (the memory that holds now); a hit that stated a fact carries facts (this value, the current one, is_current).",
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
                },
                {
                    "name": "memory_graph",
                    "description": "Query knowledge graph: find all connections, related memories, and neighbors for an entity",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "entity": {"type": "string", "description": "Entity name to look up (e.g. 'auth', 'postgresql', 'jwt')"},
                            "list_all": {"type": "boolean", "description": "If true, list all known entities instead of querying one"}
                        },
                        "required": ["entity"]
                    }
                },
                {
                    "name": "memory_followups",
                    "description": "List follow-ups (explicit open work items per project). Returns proposed + open by default. 'proposed' items are unconfirmed keyword guesses; 'open' items are authoritative.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "project": {"type": "string", "description": "Project name filter (optional)"},
                            "include_done": {"type": "boolean", "description": "Also return closed and dismissed items"},
                            "limit": {"type": "integer", "description": "Page size, 1-200 (default 50)"},
                            "offset": {"type": "integer", "description": "Rows to skip. When the result has has_more=true, call again with next_offset to reach older items"}
                        }
                    }
                },
                {
                    "name": "memory_followup_open",
                    "description": "Record an open follow-up for a project: something that is genuinely still to be done. Use at the end of work when a concrete next step remains.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "project": {"type": "string", "description": "Stable project name (e.g. 'mnemonic', 'demoapp')"},
                            "title": {"type": "string", "description": "What remains to be done, one line"},
                            "request_id": {"type": "string", "description": "Optional idempotency key; a retry with the same key is a no-op"}
                        },
                        "required": ["project", "title"]
                    }
                },
                {
                    "name": "memory_followup_update",
                    "description": "Change a follow-up's state by id (8+ char prefix accepted): confirm a proposal, close it as done, reopen it, or dismiss it. Closing requires the id; nothing is ever closed by inference from text.",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "id": {"type": "string", "description": "Follow-up id or unambiguous prefix"},
                            "action": {"type": "string", "enum": ["confirm", "close", "reopen", "dismiss"]},
                            "project": {"type": "string", "description": "Optional guard: reject if the item belongs to another project"},
                            "expected_revision": {"type": "integer", "description": "Optional optimistic-lock guard"},
                            "evidence_memory_id": {"type": "string", "description": "Optional memory id evidencing the completion"},
                            "request_id": {"type": "string", "description": "Optional idempotency key"}
                        },
                        "required": ["id", "action"]
                    }
                }
            ]
        });
        if let Some(tools) = list["tools"].as_array_mut() {
            tools.extend(crate::facts::mcp_tools::tool_list());
        }
        Ok(list)
    }

    fn handle_tools_call(
        &self,
        params: &Value,
        storage: &Storage,
        embedder: &dyn Embedder,
        scorer: &ImportanceScorer,
        attributor: Option<&crate::daemon::PeerAttributor>,
        sinks: &[Box<dyn OutputSink>],
    ) -> Result<Value> {
        let tool_name = params
            .get("name")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing tool name"))?;

        let arguments = params.get("arguments").cloned().unwrap_or(json!({}));

        let result = match tool_name {
            "memory_search" => self.handle_search(&arguments, storage)?,
            "memory_save" => {
                self.handle_save(&arguments, storage, embedder, scorer, attributor, sinks)?
            }
            "memory_recent" => self.handle_recent(&arguments, storage)?,
            "memory_similar" => self.handle_similar(&arguments, storage, embedder)?,
            "memory_context" => self.handle_context(&arguments, storage)?,
            "memory_status" => self.handle_status(storage)?,
            "memory_graph" => self.handle_graph(&arguments, storage)?,
            "memory_followups" => self.handle_followups(&arguments, storage)?,
            "memory_followup_open" => self.handle_followup_open(&arguments, storage)?,
            "memory_followup_update" => self.handle_followup_update(&arguments, storage)?,
            "memory_fact_set" => crate::facts::mcp_tools::fact_set(
                &arguments,
                storage,
                embedder,
                self.config.classifier.dedup_threshold,
            )?,
            "memory_facts" => crate::facts::mcp_tools::facts(&arguments, storage)?,
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
        let mut entries: Vec<Value> = results.iter().map(entry_to_json).collect();
        crate::updates::recall::annotate(storage, &mut entries)?;

        Ok(json!({
            "results": entries,
            "count": entries.len()
        }))
    }

    fn handle_save(
        &self,
        params: &Value,
        storage: &Storage,
        embedder: &dyn Embedder,
        scorer: &ImportanceScorer,
        attributor: Option<&crate::daemon::PeerAttributor>,
        sinks: &[Box<dyn OutputSink>],
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
        if let Some(project) = params
            .get("project")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            crate::updates::plan::set_project(storage, &mut entry, project)?;
        }
        let mut updates = Vec::new();

        // Embedding + dedup + scoring. Only the EMBEDDING text is truncated:
        // e5 attends to ~512 tokens anyway, so a few KB carries the full
        // semantic signal, and staying far under the daemon's /embed caps
        // means a large-but-valid memory can never be rejected with HTTP 400
        // (Codex review: the memory itself must always be savable in full).
        let embed_text = format!("{} {}", title, content);
        let embed_text = truncate_for_embedding(&embed_text, EMBED_TEXT_MAX_BYTES);
        match embedder.embed(embed_text) {
            Ok(emb) => {
                // `?`: a dimension-mismatch error (model swapped without a reembed) must
                // abort the save, not fall through and write a mixed-dim vector.
                let plan = crate::updates::plan::plan_settled(
                    storage,
                    &entry,
                    &emb,
                    self.config.classifier.dedup_threshold,
                    crate::updates::plan::Mode::Normal,
                    self.config.classifier.value_aware_dedup,
                )?;
                let link = match plan {
                    crate::updates::plan::Plan::Duplicate { of, similarity } => {
                        return Ok(json!({
                            "status": "skipped",
                            "reason": "duplicate",
                            "similarity": similarity,
                            "duplicate_of": of
                        }));
                    }
                    crate::updates::plan::Plan::Save { link, .. } => link,
                };

                if let Ok(score) = scorer.score(
                    &emb,
                    &crate::event::EventKind::Custom("mcp".into()),
                    &mt,
                    &storage.conn,
                ) {
                    entry.importance = score;
                }

                if let Some(link) =
                    storage.save_with_links(&entry, Some(&emb), link.as_ref(), "mcp")?
                {
                    updates.push(crate::updates::plan::link_json(&entry.id, &link));
                }
            }
            // Non-retryable embed failures (dimension split-brain, daemon 400
            // e.g. text over the /embed caps) must abort the save — silently
            // saving unembedded would hide the error from vector search/dedup.
            Err(e) if crate::embedding::daemon_client::is_hard_failure(&e) => return Err(e),
            Err(_) => {
                entry.importance = 0.7;
                storage.save(&entry)?;
            }
        }

        // Peer attribution — same fallback regime as the daemon's loop
        // (Socket source, no role metadata → user peer as speaker). No
        // session tracker here: MCP saves don't carry a JSONL path.
        if let Some(att) = attributor {
            att.attribute(storage, &entry, None);
        }

        // Enqueue for the daemon's extraction worker — same path the file/git
        // watchers use. Without this, MCP-saved memories never get graph
        // entities or a project backlink, so they never become an attribution
        // signal until a manual `backlink-projects`. The running daemon drains
        // the queue (shared SQLite), reconciles project links, and its
        // attribution worker then credits the time. No-op if the daemon is down
        // — the row simply waits, which is still strictly better than dropping
        // the signal entirely.
        if let Err(e) = storage.enqueue_extraction(&entry.id) {
            tracing::warn!("MCP save: enqueue_extraction failed for {}: {e}", entry.id);
        }

        // Write to the same output sinks the daemon uses (memory files,
        // Obsidian, Memory API) — see crate::output::build_sinks.
        for sink in sinks {
            if let Err(e) = sink.write(&entry) {
                tracing::warn!("MCP save: sink {} error: {e}", sink.name());
            }
        }

        let mut reply = json!({
            "status": "saved",
            "id": entry.id,
            "importance": entry.importance,
            "memory_type": entry.memory_type.to_string(),
            "updates": updates
        });
        if let Some(project) = entry.metadata.get("project") {
            reply["project"] = project.clone();
        }
        Ok(reply)
    }

    fn handle_recent(&self, params: &Value, storage: &Storage) -> Result<Value> {
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(10) as usize;
        let results = storage.recent(limit)?;
        let mut entries: Vec<Value> = results.iter().map(entry_to_json).collect();
        crate::updates::recall::annotate(storage, &mut entries)?;

        Ok(json!({
            "results": entries,
            "count": entries.len()
        }))
    }

    fn handle_similar(
        &self,
        params: &Value,
        storage: &Storage,
        embedder: &dyn Embedder,
    ) -> Result<Value> {
        let query = params
            .get("query")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("Missing 'query'"))?;
        let limit = params.get("limit").and_then(|v| v.as_u64()).unwrap_or(5) as usize;

        // Same truncation as the save path: keeps oversized queries under
        // the daemon /embed caps (and e5 wouldn't attend past ~512 tokens).
        let query = truncate_for_embedding(query, EMBED_TEXT_MAX_BYTES);
        let emb = embedder.embed_query(query)?;
        let results = storage.find_similar(&emb, limit)?;
        let mut entries: Vec<Value> = results
            .iter()
            .map(|(entry, sim)| {
                let mut j = entry_to_json(entry);
                j.as_object_mut()
                    .unwrap()
                    .insert("similarity".into(), json!(sim));
                j
            })
            .collect();
        crate::updates::recall::annotate(storage, &mut entries)?;

        Ok(json!({
            "results": entries,
            "count": entries.len()
        }))
    }

    fn handle_context(&self, params: &Value, storage: &Storage) -> Result<Value> {
        let topic = params.get("topic").and_then(|v| v.as_str());

        let output_path = self.config.output.memory_files_path.join("CONTEXT.md");
        Config::ensure_profile_output(&output_path)?;
        let whisper = Whisper::new(output_path);

        let content = match topic {
            Some(t) => whisper.generate_for_topic(storage, t, 10)?,
            None => whisper.generate(storage)?,
        };

        Ok(json!({
            "context": content
        }))
    }

    fn handle_graph(&self, params: &Value, storage: &Storage) -> Result<Value> {
        let entity = params.get("entity").and_then(|v| v.as_str()).unwrap_or("");
        let list_all = params
            .get("list_all")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if list_all || entity.is_empty() {
            let entities = storage.list_entities(50)?;
            let (entity_count, edge_count) = storage.graph_stats()?;
            let list: Vec<Value> = entities
                .iter()
                .map(|(name, etype, count)| json!({"name": name, "type": etype, "mentions": count}))
                .collect();
            return Ok(json!({
                "entities": list,
                "total_entities": entity_count,
                "total_edges": edge_count
            }));
        }

        let result = storage.graph_query(entity)?;
        if !result.found {
            return Ok(json!({
                "found": false,
                "entity": entity,
                "message": "Entity not found. Use list_all=true to see known entities."
            }));
        }

        Ok(json!({
            "found": true,
            "entity": result.entity_name,
            "type": result.entity_type,
            "mentions": result.mention_count,
            "first_seen": result.first_seen,
            "last_seen": result.last_seen,
            "edges": result.edges.iter().map(|e| json!({
                "source": e.source,
                "target": e.target,
                "relation": e.relation,
                "weight": e.weight
            })).collect::<Vec<_>>(),
            "neighbors": result.neighbors.iter().map(|n| json!({
                "name": n.name,
                "type": n.entity_type,
                "mentions": n.mention_count
            })).collect::<Vec<_>>(),
            "memories": result.memories.iter().map(|m| json!({
                "title": m.title,
                "type": m.memory_type,
                "importance": m.importance,
                "timestamp": m.timestamp
            })).collect::<Vec<_>>()
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

    fn handle_followups(&self, params: &Value, storage: &Storage) -> Result<Value> {
        let project = params.get("project").and_then(|v| v.as_str());
        let include_done = params
            .get("include_done")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        let limit = params
            .get("limit")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(50)
            .clamp(1, 200);
        let offset = params
            .get("offset")
            .and_then(|v| v.as_u64())
            .map(|n| n as usize)
            .unwrap_or(0);
        let page = crate::followups::list_page(storage, project, include_done, limit, offset)?;
        // has_more + next_offset make the whole backlog reachable: an agent
        // without a saved id can still find an old item to close it.
        let next_offset = page.has_more.then(|| offset + page.items.len());
        Ok(json!({
            "count": page.items.len(),
            "offset": offset,
            "has_more": page.has_more,
            "next_offset": next_offset,
            "followups": page.items
        }))
    }

    fn handle_followup_open(&self, params: &Value, storage: &Storage) -> Result<Value> {
        let project = params
            .get("project")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("project is required"))?;
        let title = params
            .get("title")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("title is required"))?;
        let request_id = match params.get("request_id").and_then(|v| v.as_str()) {
            Some(r) if !r.trim().is_empty() => format!("mcp:{r}"),
            _ => format!("mcp:{}", uuid::Uuid::new_v4()),
        };
        let f = crate::followups::create(
            storage,
            crate::followups::NewFollowup {
                project,
                title,
                source_memory_id: None,
                authoritative: true,
                actor: "mcp",
                request_id: &request_id,
            },
            chrono::Utc::now(),
        )?;
        Ok(json!({ "followup": f }))
    }

    fn handle_followup_update(&self, params: &Value, storage: &Storage) -> Result<Value> {
        use crate::followups::Action;
        let id = params
            .get("id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| anyhow::anyhow!("id is required"))?;
        let action = match params.get("action").and_then(|v| v.as_str()) {
            Some("confirm") => Action::Open,
            Some("close") => Action::Close,
            Some("reopen") => Action::Reopen,
            Some("dismiss") => Action::Dismiss,
            other => {
                return Err(anyhow::anyhow!(
                    "action must be confirm | close | reopen | dismiss, got {other:?}"
                ));
            }
        };
        let request_id = match params.get("request_id").and_then(|v| v.as_str()) {
            Some(r) if !r.trim().is_empty() => format!("mcp:{r}"),
            _ => format!("mcp:{}", uuid::Uuid::new_v4()),
        };
        let full = crate::followups::resolve_id(storage, id)?;
        let f = crate::followups::transition(
            storage,
            crate::followups::Transition {
                id: &full,
                action,
                expected_revision: params.get("expected_revision").and_then(|v| v.as_i64()),
                project: params.get("project").and_then(|v| v.as_str()),
                evidence_memory_id: params.get("evidence_memory_id").and_then(|v| v.as_str()),
                actor: "mcp",
                request_id: &request_id,
            },
            chrono::Utc::now(),
        )?;
        Ok(json!({ "followup": f }))
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

/// Max bytes of text sent to the embedder. e5 attends to ~512 tokens, so
/// this loses no retrieval signal, and it stays far under the daemon's
/// /embed request caps (64 KiB text / 32 texts) so a large-but-valid
/// memory can never be bounced with HTTP 400.
const EMBED_TEXT_MAX_BYTES: usize = 8 * 1024;

/// Truncate to at most `max_bytes`, backing up to a UTF-8 char boundary so
/// multi-byte text (Cyrillic notes!) never panics the slice.
fn truncate_for_embedding(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

#[cfg(test)]
#[path = "fact_updates_tests.rs"]
mod fact_updates_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::HashEmbedder;

    fn tmp_dir() -> tempfile::TempDir {
        crate::test_support::temp_dir("mnemonic-mcp-test-")
    }

    fn test_setup(dir: &std::path::Path) -> (McpServer, Storage, Vec<Box<dyn OutputSink>>) {
        let mut config = Config::default();
        config.storage.db_path = dir.join("memory.db");
        config.output.memory_files_enabled = true;
        config.output.memory_files_path = dir.join("memory-files");
        config.output.obsidian_enabled = false;
        config.output.memory_api_enabled = false;
        config.peers.auto_tag = true;
        let storage = Storage::open(&config.storage.db_path).unwrap();
        let sinks = crate::output::build_sinks(&config);
        (McpServer::new(config), storage, sinks)
    }

    /// Only the EMBEDDING text is capped — never the saved memory. The cut
    /// must respect UTF-8 char boundaries (Cyrillic is 2 bytes/char).
    #[test]
    fn truncate_for_embedding_respects_utf8_boundaries() {
        // Under the cap: untouched.
        assert_eq!(truncate_for_embedding("abcdef", 10), "abcdef");
        // ASCII: exact cut.
        assert_eq!(truncate_for_embedding("abcdef", 4), "abcd");
        // Cyrillic: byte 5 lands mid-char, must back up to a boundary.
        let ru = "привет"; // 12 bytes, 2 per char
        let cut = truncate_for_embedding(ru, 5);
        assert_eq!(cut, "пр");
        assert!(ru.starts_with(cut));
        // Oversized input always lands at or under the cap, non-empty.
        let big = "я".repeat(EMBED_TEXT_MAX_BYTES); // 2x the cap in bytes
        let cut = truncate_for_embedding(&big, EMBED_TEXT_MAX_BYTES);
        assert!(cut.len() <= EMBED_TEXT_MAX_BYTES && !cut.is_empty());
    }

    /// JSON-RPC 2.0: requests without an id are notifications and MUST NOT
    /// be answered — not even with an error. The server used to reply to
    /// notifications/initialized with an id:null "Unknown method" error.
    #[test]
    fn notifications_get_no_reply() {
        let tmp = tmp_dir();
        let dir = tmp.path();
        let (server, storage, sinks) = test_setup(dir);
        let emb = HashEmbedder::new();
        let scorer = ImportanceScorer::default();

        // Known MCP notification — silence.
        assert!(
            server
                .response_for_line(
                    r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
                    &storage,
                    &emb,
                    &scorer,
                    None,
                    &sinks,
                )
                .is_none()
        );
        // Unknown method without id is STILL a notification — the error
        // must be suppressed, not sent back.
        assert!(
            server
                .response_for_line(
                    r#"{"jsonrpc":"2.0","method":"no/such/method"}"#,
                    &storage,
                    &emb,
                    &scorer,
                    None,
                    &sinks,
                )
                .is_none()
        );
        // The same unknown method WITH an id is a request — it errors.
        let out = server
            .response_for_line(
                r#"{"jsonrpc":"2.0","id":7,"method":"no/such/method"}"#,
                &storage,
                &emb,
                &scorer,
                None,
                &sinks,
            )
            .unwrap();
        assert!(out.contains("\"error\""), "{out}");
        assert!(out.contains("\"id\":7"), "{out}");
        // Parse errors keep the id:null reply (id is unknowable).
        let out = server
            .response_for_line("{garbage", &storage, &emb, &scorer, None, &sinks)
            .unwrap();
        assert!(out.contains("-32700"), "{out}");
    }

    /// Save parity with the daemon path: an MCP save must come out peer-
    /// attributed and written through the shared sink set — previously it
    /// skipped attribution entirely and hand-rolled a subset of sinks.
    #[test]
    fn mcp_save_attributes_peers_and_writes_sinks() {
        let tmp = tmp_dir();
        let dir = tmp.path();
        let (server, storage, sinks) = test_setup(dir);
        let attributor =
            crate::daemon::PeerAttributor::init(&storage, &server.config.peers).unwrap();
        let emb = HashEmbedder::new();
        let scorer = ImportanceScorer::default();

        let line = r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"memory_save","arguments":{"title":"MCP parity test","content":"unique zorblefrag content for parity","memory_type":"decision","tags":"mcp,test"}}}"#;
        let out = server
            .response_for_line(line, &storage, &emb, &scorer, Some(&attributor), &sinks)
            .unwrap();
        assert!(out.contains("saved"), "{out}");

        // Linked to a peer, same as a daemon-captured memory.
        {
            let conn = storage.conn.lock().unwrap();
            let peers: i64 = conn
                .query_row(
                    "SELECT COUNT(*) FROM memory_peers mp
                       JOIN memories m ON m.id = mp.memory_id
                      WHERE m.title = 'MCP parity test'",
                    [],
                    |r| r.get(0),
                )
                .unwrap();
            assert!(peers >= 1, "MCP save must carry peer attribution");
        }

        // Went through the shared sinks (memory_files here).
        let files = std::fs::read_dir(dir.join("memory-files"))
            .map(|d| d.count())
            .unwrap_or(0);
        assert!(files >= 1, "memory_files sink must receive MCP saves");
    }
}
