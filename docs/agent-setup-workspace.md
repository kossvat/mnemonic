# Setting up mnemonic over a workspace of projects (runbook for an AI agent)

You are an AI agent asked to install mnemonic for one person who works on
several projects with coding agents (Claude Code, Codex). Done right, mnemonic
remembers their decisions and corrections, and also shows which projects are
in work right now, how much time went into each, and where each plan stands.
That second part only works when the projects are laid out the way mnemonic
reads them. Follow the steps in order. Stop and ask the human where a step
says so.

Which runbook is yours:

- One person, their own machine, many projects, one memory: this file.
- One project's memory kept apart from everything else, or shared by several
  agents of a team: [agent-setup.md](agent-setup.md).
- Several people on one hub: [shared-hub-setup.md](shared-hub-setup.md).

Words used below:

- **Profile**: one configuration and one store. This runbook uses the default
  profile: the configuration in `~/.config/mnemonic/config.toml`, the store
  and the log in `~/.mnemonic`. `MNEMONIC_HOME` and `--home` select another
  profile. For the default one the variable must not be set at all: run
  `unset MNEMONIC_HOME` in the shell you work in (an empty value is refused).
- **Unit**: one piece of a project's plan: a change, a release, or a named
  group of tasks. A project can have several.
- **Gate**: the checks a project runs before work counts as done. mnemonic
  only reads the receipts such checks leave; it never runs them.

## Rules

1. Never move, rename or delete the human's folders yourself. Propose the
   layout, show what would move, and let them do it or approve each move.
2. Ask before installing Rust or other system packages, and before editing an
   agent's global configuration.
3. `workspace` is an absolute path. `folder` is given relative to the
   workspace, and a plan's `authority` relative to its project.
4. Never edit a plan file, a task file or a receipt of the human's project to
   make a card look right. Report what mnemonic reads and let them decide.
5. Report what you find. Do not edit the configuration to make a message go
   away.

## 0. If mnemonic is already installed

Check first: `command -v mnemonic`, `ls -d ~/.mnemonic`. If neither is there,
go to step 1.

A newer binary upgrades the store on its first use, and an older binary then
refuses it. So the copy is taken before the new binary is installed:

1. Build the new version without installing it: in this repository's
   checkout, `cargo build --release --locked`.
2. Take the backup with the binary you just built, into a new folder the
   human names. It copies the stores as they are, while the daemon runs, and
   upgrades nothing:

   ```bash
   ./target/release/mnemonic backup /absolute/path/to/backup-folder
   ```

3. Stop the old daemon. If a launchd agent runs it, unload the agent, or it
   starts the daemon again at once. `launchctl list | grep mnemonic` shows
   whether there is one, and this unloads it:
   `launchctl bootout gui/$(id -u)/com.kossvat.mnemonic.daemon`.
   Otherwise `mnemonic stop`.
4. Continue with step 3 (install). An unloaded launchd agent is loaded again
   after the install, in place of the start line of step 5:
   `launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.kossvat.mnemonic.daemon.plist`.
   Then ask the human to restart the agent windows that were open: their
   memory server is still the old binary.

Going back means restoring that backup with the old binary.

## 1. How mnemonic sees projects

Read this before you look at the human's disk. Everything below follows from
it.

**One folder holds the projects.** The configuration names a workspace and,
optionally, a folder inside it. Every visible child folder of that folder is a
project, under the name of its folder. A hidden folder (its name starts with a
dot) and a file are not projects. Nothing else is discovered: a project that
lives somewhere else on the disk is not seen.

```text
workspace/
  projects/            <- the folder of projects
    api/               <- project "api"
    web-client/        <- project "web-client"
      ui/              <- a part, "web-client:ui", if it carries a marker
    notes/             <- project "notes" (a folder with no code is a project too)
    .scratch/          <- hidden: not a project
```

When no folder is named, the workspace itself is the folder of projects.

**Every child folder counts.** A folder of downloads, an archive or a scratch
folder that sits beside the projects becomes a project too. Keep only projects
in that folder.

**The key of a project** is its folder name in lower case, with every run of
characters that are not letters or digits turned into one dash and no dash at
either end. `My App` and `my-app` both give `my-app`. Two folders that give
the same key cannot both be projects: one of them is refused.

**A project inside a project.** A folder directly inside a project that
carries one of these markers is a part of it and is tracked under its parent:
`.git`, `openspec`, `Cargo.toml`, `package.json`, `Package.swift`,
`pyproject.toml`, `go.mod`. Only that one level is looked at:
`web-client/packages/ui` is not a part.

**A path names a project by the folder that follows the name of the folder of
projects**, wherever the checkout lies. The name is the last part of the path
of the folder of projects: `projects` in the layout above, or the name of the
workspace folder when no folder is named. With the layout above, any path that
contains `projects/api/` names the project `api`: in the workspace, in a git
worktree, in a review copy. Three things follow:

- Give the folder of projects a name that does not occur by accident in other
  paths. `projects`, `code` or `work` are fine. `src` or `lib` are not.
- A worktree or a second checkout is attributed only when its path keeps that
  segment. `/any/where/feature-x/projects/api` works.
  `/any/where/api-feature-x` does not.
- The match is by name only. A folder elsewhere on the disk that happens to
  be called `projects/api` is taken for the same project.

**Which project a session works on.** mnemonic reads the transcripts the
agents already write and applies these rules to each record, in order:

1. A session starts on the project of the folder it was started in. Started
   outside every project, it starts as a session without a project.
2. A call of a writing tool whose input names exactly one project switches
   the session to that project at once.
3. A call of a reading tool that names exactly one project switches it when
   two such calls in a row name the same project.
4. A shell call counts as a read. It is judged by the folder it runs in when
   that folder is a project's, otherwise by the one project its command names.
5. A record that names two or more projects changes nothing.
6. A change of the working folder into a project's folder switches at once.

A project is named by a path that holds the segment described above. A
path without it, such as a bare file name, names nothing.

Inside a stretch of work, the time up to the next record belongs to the
project the session was on. When a stretch begins without a project, its
beginning is given to the first project the stretch names. A pause as long
as the idle time (30 minutes by default, `[sessions] idle_timeout_secs`)
belongs to no one and ends the stretch. Time that no rule could give to a
project is shown as time without a project. It is never spread over the
others.

**When a session is live.** Running: its last record is in the middle of a
turn and under 2 minutes old. Quiet: in the middle of a turn and 2 to 30
minutes old (a long build). Waiting: the agent closed its turn and the human
has the move; after 10 minutes the session leaves the list.

## 2. Look at the layout with the human

Ask where their projects live and list that folder:

```bash
ls -d /absolute/path/to/folder/*/
```

Go through this with them before you configure anything:

- **Is there one folder that holds the projects?** If they are scattered over
  the disk, mnemonic can follow only one folder. Ask the human to choose it
  and to bring the others in. A project left outside is not seen at all: no
  card, no time. This is the human's decision, not part of the install: if
  they would rather not move anything, set up the folder they have and tell
  them which projects are not covered. Tell them what a move costs: the history recorded under the
  old location stays without a project, and git worktrees of a moved
  repository have to be repaired (`git worktree repair`). Do not build the
  folder out of symbolic links: the agents may record the real path, which
  would not name the project.
- **Is everything in that folder a project?** List the children that are not.
  The human moves them out, or accepts them as projects. A project can be
  archived later (step 12); it cannot be hidden by configuration.
- **Are the names right?** The name of the folder is the name the human will
  see everywhere. Renaming a folder later is fine: the former name is given to
  the configuration (step 4) and history follows.
- **Do two folders give the same key?** `My App` and `my-app` do. One of them
  must be renamed.
- **One repository or many?** Both work for live projects and time. A
  monorepo whose projects sit in one folder: the workspace is the repository
  root and `folder` is that folder. A plain folder of separate repositories:
  the workspace is that folder and `folder` is left out. One difference:
  commits are captured from one repository only (step 5).
- **Where do worktrees go?** See the rule about paths above. If their
  worktrees would not be attributed, propose a location that keeps the
  segment. The human relocates an existing one with
  `git worktree move <worktree> <new-path>`.

Write down two values and have the human confirm them:

- `WORKSPACE`: the absolute path of the workspace.
- `FOLDER`: the folder of projects inside it, or nothing when the workspace
  itself holds the projects.

## 3. Build and install

Preconditions:

```bash
cargo --version && cc --version
```

- No `cargo`: ask the human, then install Rust with rustup. The repository
  pins its compiler in `rust-toolchain.toml`; rustup fetches that version on
  the first build.
- macOS, `cc` exits with code 69: the Xcode license is pending. The human runs
  `sudo xcodebuild -license`. You cannot.
- Network access is needed for the build and once more at the first start,
  which downloads the embedding model, about 1 GB on disk. Tell the human.
  The default build includes neural embeddings; no feature flag is needed.

Clone the repository outside the folder of projects, unless the human wants
mnemonic itself listed as one of their projects. From the directory that
contains its `Cargo.toml`:

```bash
cargo install --path . --locked
mnemonic --version
```

The binary lands in `~/.cargo/bin/mnemonic`. The build can take 10 to 20
minutes. If `mnemonic` is not found afterwards, `~/.cargo/bin` is not on the
`PATH`.

Two values are used below. `CHECKOUT` is the absolute path of this
repository's checkout. `BIN` is the absolute path of the binary; set it in the
shell you work in, and write the path itself where a file needs it:

```bash
BIN="$(command -v mnemonic)"; echo "$BIN"
```

## 4. Configure

Create the configuration if there is none:

```bash
mnemonic init
```

It writes every section with its defaults. Open
`~/.config/mnemonic/config.toml` and change keys inside the sections that are
already there. Do not add a second `[ui]`, `[watchers]` or `[projects]`
section: a section that appears twice makes the file unreadable.

```toml
[ui]
enabled = true             # the widget and the app read this API

[watchers]
watch_paths = ["/absolute/path/to/workspace"]

[projects]
workspace = "/absolute/path/to/workspace"
folder = "projects"        # leave out when the workspace itself holds them

[projects.aliases]         # optional second names: alias = "folder name"
# web = "web-client"

[projects.former]          # optional: old folder name = "new folder name"
# old-name = "new-name"
```

Tell the human what these do before you save:

- `[ui] enabled` opens an API on the loopback interface only, port 3737
  unless `port` says otherwise, behind a token kept in `~/.mnemonic/auth.token`
  (`token_file`). The menu-bar widget and the app need it.
- `watch_paths` is where file changes are watched, with everything below. The
  default is the folder the daemon happens to be started in, and
  `~/.claude-flow`; naming the workspace makes it the same every time.
- By default mnemonic also writes memory files for Claude Code under
  `~/.claude/projects` (`[output] memory_files_enabled`, `memory_files_path`).
  Set the first to `false` if the human does not want that. The hook of step
  7 still writes one `CONTEXT.md` under that path at each session start.

Without `workspace` no folder is discovered. Memory works, and time is
attributed the earlier way, by the memories linked to each project. There are
no live projects.

## 5. Start the daemon and check what it found

Before the first start, tell the human what mnemonic reads and keeps: the
transcripts of their agents, git activity and an idle counter, all on this
machine. The README lists it under "What it reads and stores". Start on their
OK.

Start the daemon from the workspace:

```bash
cd /absolute/path/to/workspace && mnemonic start -d
mnemonic status
grep "Projects:" ~/.mnemonic/daemon.log | tail -1
```

The first start loads the embedding model, and downloads it the very first
time. `start -d` returns at once: run `mnemonic status` again until it reports
the daemon running.

A watcher of transcripts starts only when its folder exists: `~/.claude/projects`
for Claude Code, `~/.codex/sessions` for Codex. If an agent has never been run
on this machine, restart the daemon after its first session.

Commits are captured from one repository: the folder the daemon was started in
must hold a `.git` itself. In a monorepo that is the whole workspace. In a
plain folder of separate repositories there is no repository there, the log
says `No .git directory found, git watcher disabled`, and commits are not
captured. Commits made before the daemon started are not brought in either.
Decisions and corrections from the agents' sessions, live projects and time by
project do not depend on it. Say this to the human plainly.

The log line reads `Projects: P in the workspace (M folders mapped, R refused,
A aliases unset)`. P counts the projects found, parts included. M also counts
the former names of the configuration. Compare P with the folders the human
confirmed in step 2 plus their parts.

- `refused` above zero: a folder was not taken. Two folders give the same key,
  a name gives an empty key or is not valid text, or a name looks like a
  secret and the store will not keep it. To find which, compare the folders
  with the projects the API lists (`/api/projects`, called like the request
  of step 11), and tell the human.
- `aliases unset` above zero: an alias or a former name of the configuration
  could not be set. It points to a folder that is not a project, it collides
  with another name, or the folder a former name speaks of still exists.

The list of folders is read at start and again within ten minutes of a folder
being added or removed. After any change of the configuration, restart the
daemon, again from the workspace, because a daemon started by hand takes the
folder it is restarted in:

```bash
cd /absolute/path/to/workspace && mnemonic restart
```

`mnemonic start -d` does nothing when a daemon already runs.

**Starting at login.** No job is installed for you. With Claude Code, the hook
of step 7 starts the daemon with the first session after a reboot. With Codex
alone nothing does: the human runs the start line above, or asks for a login
job. On macOS that is a launchd agent, and mnemonic knows it by one label,
`com.kossvat.mnemonic.daemon`: when an agent with that label is loaded,
`mnemonic start -d` leaves the daemon to it and `mnemonic restart` restarts
through it. Only on the human's word, save this as
`~/Library/LaunchAgents/com.kossvat.mnemonic.daemon.plist` with the two paths
filled in:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<plist version="1.0">
<dict>
  <key>Label</key><string>com.kossvat.mnemonic.daemon</string>
  <key>ProgramArguments</key>
  <array><string>BIN</string><string>start</string></array>
  <key>WorkingDirectory</key><string>WORKSPACE</string>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
</dict>
</plist>
```

Stop a daemon started by hand (`mnemonic stop`), then load the agent:
`launchctl bootstrap gui/$(id -u) ~/Library/LaunchAgents/com.kossvat.mnemonic.daemon.plist`.

## 6. Bring in history

Two different things can be read from the transcripts that are already on
disk. Both are optional and both wait for the human's OK.

**Time by project.** A dry run prints the hours by project the rules give for
the last 14 days and writes nothing:

```bash
mnemonic attribute replay --dry-run --days 14
```

Show it to the human: it is the first honest look at whether the layout
works. A large share of time without a project means sessions were started
outside the projects (step 8). Then, on their OK:

```bash
mnemonic attribute backfill
```

It reads the transcripts of the agents mnemonic watches (Claude Code and
Codex), can be interrupted and run again, and a second run reads only what
was added since. It can run while the daemon runs. It gives the agents' time
by project. The human's own time is estimated from what mnemonic records
while it runs, so it begins with the install.

**Decisions and corrections.** From now on they are captured as sessions
happen. The ones in older transcripts are counted by `mnemonic ingest-history`
and, on the human's OK, brought in by `mnemonic ingest-history --apply`.
`--since YYYY-MM-DD` limits it to recent history.

## 7. Connect the agents

The memory tools appear only in agent sessions started after this step.

**Claude Code.** Register the memory server for every project of this user:

```bash
claude mcp add --scope user mnemonic -- "$BIN" mcp
```

Then add a hook that starts the daemon and loads context at the start of a
session. It goes into the user's `~/.claude/settings.json`, so that it runs in
every project. Merge it into the `SessionStart` list that may already be
there; do not replace other hooks. `timeout` is in seconds.

```json
{
  "hooks": {
    "SessionStart": [
      {
        "hooks": [
          {
            "type": "command",
            "command": "sh -c '(cd \"WORKSPACE\" && \"BIN\" start -d) 2>/dev/null; \"BIN\" context 2>/dev/null || true'",
            "timeout": 60
          }
        ]
      }
    ]
  }
}
```

Write the two absolute paths in place of `WORKSPACE` and `BIN`, inside the
quotes that are there: they keep a path with a space in one piece.

**Codex.** Codex has no such hook: it gets the memory through the tools only.
In `~/.codex/config.toml`:

```toml
[mcp_servers.mnemonic]
command = "/absolute/path/to/mnemonic"
args = ["mcp"]
```

Codex transcripts are watched by default (`[watchers] codex_enabled`).

**Offer, do not add silently:**

- The block for the human's instruction file that asks the agent to save
  decisions itself. Its text is in the README under "Use with Claude Code".
  For all projects it goes into `~/.claude/CLAUDE.md`; Codex reads
  `AGENTS.md`.
- More trigger phrases, if the human works in a language other than English
  or Russian (README, "Non-English capture").

## 8. Tell the human how to start sessions

This is the part that decides whether the numbers are right.

- **Start each agent inside the project's folder**, or deeper. A session
  started at the workspace root, or in the home folder, begins without a
  project and stays so until a rule of step 1 gives it one. Its decisions and
  corrections are remembered all the same; its time has no project until
  then.
- **One project per session** gives the cleanest time. A session that moves
  to another project is followed by the rules of step 1. The time before the
  record that shows the move stays with the earlier project.
- **Worktrees** keep the segment of step 1 in their path.
- **Sessions run by a program** are shown as automated when the transcript
  says so (a Claude Code SDK entrypoint, a Codex exec or subagent run). What
  they write is not captured as the human's own decisions.

If the human keeps an instruction file for their agents, offer one line for
it: start sessions inside the project folder, one project per session.

## 9. Plans (optional)

A project card can show where the plan stands. mnemonic only reads files that
are already there. It never runs a tool and never writes to a project. There
are three kinds of plan file:

- **A task registry** (provider `registry`): `docs/engineering/tasks.json` or
  `tasks.json` at the project root. A task has `id`, `title` and `status`.
  The statuses are `backlog`, `todo`, `planned`, `ready`, `in_progress`,
  `review`, `blocked` and `done`; any other status puts the unit in error. A
  task belongs to the unit its `unit` or `release` field names, or to the
  release that lists its id under `releases`. Tasks that name none form one
  unit called `tasks`.

  ```json
  {
    "releases": { "1": ["T1", "T2"] },
    "tasks": [
      { "id": "T1", "title": "Rotate refresh tokens", "status": "done" },
      { "id": "T2", "title": "Rate limit the login", "status": "in_progress" },
      { "id": "T3", "title": "Write the runbook", "status": "todo", "unit": "docs" }
    ]
  }
  ```

  A plain list of tasks, without `releases`, is read the same way.
- **Checkbox files** (provider `checkbox`):
  `openspec/changes/<change>/tasks.md`, one unit per change, tasks as `- [ ]`
  and `- [x]` lines. Only `x` marks a task done. A change with a
  `proposal.md` and no tasks is shown as proposed. A change under
  `openspec/changes/archive/YYYY-MM-DD-<change>/` is a closed unit.
- **A pipeline table** (provider `funnel`): `PIPELINE.md` at the project
  root, a Markdown table with a `status` column. It shows counts by status,
  not a percentage.

The first of these that applies decides which one a project uses: what the
configuration names for the project (below); a task file that the project's
`openspec/config.yaml` declares with a line `task_authority: <relative path>`;
a task registry; a pipeline table; checkbox files. A project with none shows
its sessions and hours and "No plan recorded". That is a correct state, not an
error.

**What the card shows.** Progress is the tasks with the status `done` out of
all tasks of the unit. The task in work is the one marked `in_progress`, else
the one whose id stands as a whole word in the branch name of a live session
(the longest such id), else the first task that is not done. The stage of a
unit:

- blocked: a task has the status `blocked`, the last three gate runs failed,
  or the file could not be read;
- proposed: the unit has no tasks yet, such as a change with only a proposal;
- done: every task is done and the gate passed, or the project has no gate;
- review: every task is done and the gate has not passed;
- building: some task is done, or a session is live on the unit;
- planned: otherwise.

A session is live on a unit when the project has one open unit, or when the
branch of the session names the unit or one of its tasks. A unit whose plan
file has not changed for 14 days and that has no such session is also marked
stale.

Check what mnemonic reads for one project, without a daemon or a store. The
command applies the default choice and ignores the configuration, so use it
before you add any override:

```bash
mnemonic plan-counts --root /absolute/path/to/workspace/projects/api
```

Compare the counts with the plan file. If the provider is guessed wrong, name
it in the configuration, under the key of the project, and restart the daemon
(step 5):

```toml
[projects.plans.api]
provider = "registry"           # registry, checkbox or funnel
authority = "docs/tasks.json"   # relative to the project
# receipt = "build/gate.json"   # one receipt file instead of target/checks
# gate_required = true
```

After an override, check the result where the card reads it: the reply of
`/api/projects/api` (called like the request of step 11) holds the plan of
that project.

Which checkout a plan is read from: a registry always from the project's own
folder. Checkbox files, while sessions are live, from the checkout of each
live session beside the main one, and the card shows the checkout where most
tasks are done. Only a checkout of the same repository counts: a worktree
does, an unrelated clone under a like-named folder does not. While no session
is live, from the project's folder, or from `checkout = "/absolute/path"`
when that is set.

**Gate receipts.** When a project records the result of its checks, the card
shows it. mnemonic reads `target/checks/<unit>/<run>/verification.json` and
takes the newest, where `<run>` is the Unix time of the run. A receipt is JSON
with `change` (the unit) and `passed` (true or false).

A receipt that passed counts as passed only while it names what it tested and
the project's last commit still holds exactly that (changes not yet committed
are not seen). `candidate_subtree` is the git tree of
the project's folder that was tested: `git rev-parse 'HEAD^{tree}'` when the
project is the root of its repository, `git rev-parse HEAD:path/to/project`
when it is a folder inside one. It must equal the same tree at the checkout's
head; otherwise the gate is shown as stale. A single receipt file named with
`receipt` may give `commit` instead, and is fresh while that commit is the
head.

A project has a gate when a `receipt` is configured or when it holds one of
`target/checks`, `bin/verify.sh` or `private/verify_change.py`;
`gate_required` says so either way. Without a gate, a unit with every task
done is done. A passed gate is never inferred from a full bar: without a
receipt the card says the gate is not recorded.

## 10. The menu-bar widget (macOS)

It needs macOS 14 or later and Swift 5.9 or later. Details are in
[clients/macos/README.md](../clients/macos/README.md).

```bash
cd CHECKOUT/clients/macos
swift build -c release --product MnemonicBar
.build/release/MnemonicBar &
```

Its title shows the count of live projects and, once the daemon has counted
them, the count of memories. The first page lists the projects in work. If it
says the API is off, the configuration does not enable `[ui]` (step 4). If it
says the daemon is unreachable, the daemon is not running or was not restarted
after step 4. The human adds the binary to Login Items if they want it at
login.

The companion app with the Projects board is built by
`CHECKOUT/clients/macos/scripts/build-app.sh`. Its project screens are provisional;
build it only if the human asks.

## 11. Acceptance test

Items 4 to 8 need agent sessions started after step 7. Ask the human to open
them. Report each result.

1. `mnemonic doctor` shows the daemon running, the socket accepting
   connections and the loopback API responding. A warning that the current
   folder holds no git repository is fine.
2. The log line of step 5 counts the folders the human confirmed plus their
   parts.
3. The live snapshot answers. The token is read from its file and is never
   printed:

   ```bash
   curl -s -H "X-Mnemonic-Token: $(cat ~/.mnemonic/auth.token)" \
     http://localhost:3737/api/live
   ```

   The reply is JSON with `projects`, `unknown` and `today`, and `memories`
   once they are counted. Each entry of `projects` has `key`, `name`, `state`,
   its `sessions` and, when there is one, its `plan`.
4. A Claude Code session inside one project folder, one message sent. Within
   about 20 seconds that project is in `projects` of the reply above,
   `running` or, when the turn has already ended, `waiting`, and the widget
   shows it.
5. The same with a Codex session in another project folder.
6. A session started in the home folder. It is counted under `unknown` (a
   session without a project) until a rule of step 1 gives it one.
7. Automatic capture: in a session inside a project, the human types a short
   correction that begins with words the detector knows, for example
   `stop, use the other configuration file`. Not a question, not a long
   message. After about 20 seconds
   `mnemonic query "other configuration file"` finds it.
8. The tools: in one agent call `memory_save` with a short test decision; in
   another call `memory_search` for it.
9. `mnemonic attribute replay --dry-run --days 7` prints hours by project.
10. For each project with a plan file, the card shows the units and the
    counts of that file. Without an override they are the ones
    `mnemonic plan-counts --root` printed; with an override, compare with the
    file the override names.

## 12. Keeping it right

- **A new project**: create its folder in the folder of projects. Nothing else
  is needed.
- **A renamed folder**: add `old-name = "new-name"` under `[projects.former]`,
  rename the project's `[projects.plans.<key>]` table if it has one, and
  restart the daemon (step 5). The history of the project follows it. Parts inside it
  are not carried over: under the new name they start again.
- **Two folders that are one project**: merge one into the other with
  `POST /api/projects/{key}/merge` and the body `{"into": "<other key>"}`.
  No history moves, and `POST /api/projects/{key}/unmerge` undoes it.
- **A finished project**: `POST /api/projects/{key}/archive` takes it off the
  board and keeps its history; `/unarchive` brings it back.

  Both routes are on the loopback API and take the token header of step 11.
- **Wrong time on a day**: a correction can be made on the Timeline of the app
  and undone there.

## Troubleshooting

| What you see | Meaning | Action |
|---|---|---|
| A project is missing from the board | It is not a visible child of the folder of projects, it was refused, or it is archived or merged | Check step 2, the log line of step 5 and the counts under the board |
| Sessions without a project | They were started outside every project and no tool call named one | Step 8 |
| Most time is without a project | The sessions of that period were started outside the projects, or `folder` is wrong | Check `[projects]`, run the dry run of step 6 |
| A worktree's sessions are not attributed | Its path does not hold the segment of step 1 | The human moves it: `git worktree move <worktree> <new-path>` |
| The widget says the API is off | The configuration does not enable `[ui]` | Step 4, then restart the daemon (step 5) |
| The widget says the daemon is unreachable | The daemon is not running, or was not restarted after step 4 | Restart the daemon (step 5), then `mnemonic doctor` |
| A live session does not appear | The agent's transcript folder did not exist when the daemon started | Restart the daemon (step 5) |
| Hours are empty before today | History was not read, or the hours are the human's own | `mnemonic attribute backfill` brings the agents' time. The human's own time begins with the install |
| No commits are remembered | The folder the daemon was started in holds no `.git`, or `[watchers] git_enabled` is false | Step 5 |
| A plan unit is blocked | A task has the status `blocked`, or the last three gate runs failed | Read the plan file and the receipts |
| A plan unit is in error | A task has a status outside the list, or the file cannot be read | `mnemonic plan-counts --root <project folder>`, step 9 |
| The configuration cannot be read | A section appears twice, or a path is not quoted | Step 4 |
| A command refuses the store as newer than itself | An older binary met a store a newer one upgraded | Update every binary, restart the agent windows that were open |
