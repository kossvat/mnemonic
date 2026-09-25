# Contributing to Mnemonic

## Requirements

- Rust 1.70+
- git

## Build

```bash
cargo build
```

## Test

```bash
cargo test
```

## Run locally

```bash
cargo run -- start
```

## Code style

- Format with `cargo fmt` before committing
- Run `cargo clippy` and fix all warnings -- zero warnings policy

## Architecture overview

The codebase is trait-based with four core abstractions:

| Trait | Purpose |
|-------|---------|
| `Embedder` | Generates SimHash vectors for semantic similarity |
| `Classifier` | Assigns memory type (decision, feedback, note, etc.) |
| `OutputSink` | Writes memories to a destination (Claude Code files, Obsidian, etc.) |
| `Watcher` | Monitors sources for new content (filesystem, git) |

Storage is SQLite with FTS5. The daemon runs async on tokio.

## Adding a new output sink

1. Create a new module under `src/output/`
2. Implement the `OutputSink` trait
3. Register it in the sink registry (`src/output/mod.rs`)
4. Add configuration options to `MnemonicConfig`

## Pull requests

- Describe **what** changed and **why**
- Add tests for new features
- Ensure `cargo test` and `cargo clippy` pass
- Keep commits focused -- one logical change per commit

## License

MIT
