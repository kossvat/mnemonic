# Setting up an isolated project memory (runbook for an AI agent)

You are an AI agent asked to install mnemonic on this machine and connect the
local coding agents (Claude Code, Codex, Hermes, Claude Desktop) to ONE
project's memory. Follow the steps in order. Stop and ask the human when a step
says so. Do not improvise paths: every path below must be absolute.

## What you are building

- One **isolated profile**: a directory that holds this project's config,
  database, socket and logs, and nothing else.
- Every agent reaches it through the same stdio MCP server, pinned with
  `--home <profile>` in its arguments.
- Automatic capture of decisions and corrections from Claude Code and Codex
  transcripts, limited to sessions that run inside the project directory.

## Rules

1. Never use `~/.mnemonic`, `~/.config/mnemonic`, the home directory, or
   anything inside them as the profile. mnemonic refuses these; do not look for
   a workaround.
2. Never create symlinks inside the profile and never copy a config from
   another mnemonic install.
3. Always pass `--home` in the MCP arguments. Do not rely on the
   `MNEMONIC_HOME` environment variable: launchers drop env blocks.
4. If a command fails with an isolation error, report the message to the
   human. Do not edit the config to make the error go away.
5. Ask the human before installing Rust or other system packages.

## 0. Preconditions

```bash
cargo --version && cc --version
```

- No `cargo`: ask the human, then install Rust with rustup (https://rustup.rs).
- macOS, `cc` exits with code 69: the Xcode license is pending. The human must
  run `sudo xcodebuild -license` (or
  `sudo xcode-select -s /Library/Developer/CommandLineTools`). You cannot.
- Network access is needed once: the first start downloads the embedding model.

## 1. Build and install

From the directory that contains this repository's `Cargo.toml`:

```bash
cargo install --path . --locked
mnemonic --version
```

The binary lands in `~/.cargo/bin/mnemonic`. The build takes 10 to 20 minutes.

## 2. Choose the two paths

Ask the human to confirm both before continuing.

- `PROJECT_ROOT`: the project's repository root. Inside the repo:
  `git rev-parse --show-toplevel`.

  Capture follows the folder a session was STARTED in, not the files it
  touches. Ask the human where they usually open Claude Code and Codex for
  this project. If that is a parent folder holding several projects (a
  monorepo), sessions there count as outside the project and nothing is
  captured automatically; say so, and suggest opening the agents inside the
  project folder instead. Never widen the root to the parent: every other
  project in it would flow into this store.
- `PROFILE`: `$HOME/.mnemonic-profiles/<project-name>` (short, lowercase, no
  spaces). Keep it short: the Unix socket path inside it must stay under 100
  bytes.

## 3. Create the profile

```bash
mnemonic --home "$PROFILE" init --project-root "$PROJECT_ROOT"
```

This validates everything before writing, then prints the exact wiring for
each agent with absolute paths. Keep that output: steps 4 and 5 use it.

If it says the config already exists, the profile was set up before. Do not
pass `--force` unless the human asks for a reset.

## 4. Start the profile daemon

```bash
cd "$HOME" && mnemonic --home "$PROFILE" start -d
```

The first start loads the embedding model (about 30 seconds; longer on the very
first run while it downloads). Wait until this succeeds:

```bash
mnemonic --home "$PROFILE" status
```

The daemon gives all agents one shared embedder and runs transcript capture.
It does not restart after a reboot yet: run the `start -d` line again.

## 5. Connect the agents

Use the lines printed in step 3. For reference, with `BIN` = the absolute path
of `mnemonic`:

- **Claude Code** (run inside `PROJECT_ROOT` so the server is scoped to this
  project): `claude mcp add mnemonic -- "$BIN" --home "$PROFILE" mcp`
- **Codex** (`~/.codex/config.toml`):
  ```toml
  [mcp_servers.mnemonic]
  command = "<BIN>"
  args = ["--home", "<PROFILE>", "mcp"]
  ```
- **Hermes** (`~/.hermes/config.yaml`, then `/reload-mcp`):
  ```yaml
  mcp_servers:
    mnemonic:
      command: "<BIN>"
      args: ["--home", "<PROFILE>", "mcp"]
  ```
- **Claude Desktop** (`claude_desktop_config.json`, under `"mcpServers"`):
  `"mnemonic": { "command": "<BIN>", "args": ["--home", "<PROFILE>", "mcp"] }`

Codex, Hermes and Claude Desktop configs are global. Capture is scoped to the
project, but an explicit `memory_save` from an unrelated session would still
land in this store. Tell the human, and only save project knowledge here.

## 6. Bring in the project's history

Capture only sees sessions from now on. Conversations about this project that
already happened can be read in once:

```bash
mnemonic --home "$PROFILE" ingest-history
```

This only counts: how many corrections and decisions it found inside the
project, how many are new, and how many it left out as other projects. Show
the numbers to the human. On their OK:

```bash
mnemonic --home "$PROFILE" ingest-history --apply
```

The running daemon files them within a minute or two, with their original
dates. Running it again adds nothing. `--since YYYY-MM-DD` limits it to recent
history.

## 7. Acceptance test

Report each result to the human.

1. `mnemonic --home "$PROFILE" doctor` prints `Isolated profile: <PROFILE>`.
2. In one agent call `memory_save` with a short test decision. In the other
   agents call `memory_search` for it. All of them must find it.
3. Save a correction of that decision from a second agent. `memory_search`
   should now return both entries.
4. Capture: in a Claude Code or Codex session running inside `PROJECT_ROOT`,
   have the human type a correction in plain words. After about 20 seconds:
   `mnemonic --home "$PROFILE" query "<words from the correction>"` finds it.
5. Isolation: a session started outside `PROJECT_ROOT` must add nothing.
   `mnemonic --home "$PROFILE" status` shows the same total before and after.
6. Facts: call `memory_fact_set` for subject `Widget`, predicate `price`,
   value `$5`, then again with `$6`. `memory_facts` for `Widget` shows `$6`
   as current and `$5` in its history.

## 8. How agents should use the memory

- Save decisions, corrections, handoffs and open questions. Include the date,
  the source (link, file, person) and what is still unresolved.
- Do not save secrets, credentials, tokens or customer personal data.
- For a value that can change (a price, commission, discount, budget,
  deadline, payment terms, an owner, a status), call `memory_fact_set` with
  the subject, the predicate, the value and the project. A new value
  replaces the current one and keeps the old one in its history; the same
  value again only reconfirms it; a value with an older `as_of` joins the
  history without becoming current.
- Before quoting such a value, call `memory_facts` for its subject. A search
  hit that carries `replaced_by`, or `facts` with `is_current: false`, is
  history, not the current value.
- Facts are for business terms only: never personal data, credentials or
  secrets.
- Search before asking the human something the project may already know.

## Troubleshooting

| Message | Meaning | Action |
|---|---|---|
| `overlaps the default store` | The profile path is inside or around `~/.mnemonic` | Use `~/.mnemonic-profiles/<name>` |
| `socket path is N bytes` | Profile path too long | Pick a shorter `PROFILE` |
| `--home ... and MNEMONIC_HOME ... disagree` | A stale variable is set | Unset `MNEMONIC_HOME` |
| `is outside the isolated profile` | A config path points out of the profile | Report it; do not edit around it |
| `contains the whole home directory` | `--project-root` is too broad | Pass the repository root |
| Agents are slow to start | No daemon, each MCP process loads the model | Run step 4 |
