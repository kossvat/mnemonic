# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-04-12

### Added
- Background daemon with file watcher (FSEvents/inotify) and git watcher
- Rule-based classifier for memory types (decision, feedback, note, security, session_summary)
- SQLite storage with FTS5 full-text search
- SimHash embeddings (256-dim) for semantic deduplication
- Dynamic importance scoring (frequency x 0.3 + recency x 0.3 + signal x 0.4)
- Claude Code memory file output (auto-detected project paths)
- Obsidian vault output (optional, disabled by default)
- Whisper context injection -- generates CONTEXT.md with prioritized memories
- MCP server with 6 tools (memory_search, memory_save, memory_recent, memory_similar, memory_context, memory_status)
- CLI with 14 commands (start, stop, status, query, similar, recent, save, context, export, import, cleanup, doctor, mcp, init)
- Export/import for backup and migration
- Memory cleanup with configurable TTL and importance threshold
- Doctor command for diagnosing setup issues
- Auto-start via Claude Code SessionStart hook
