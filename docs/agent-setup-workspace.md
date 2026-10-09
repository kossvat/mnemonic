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

This runbook uses the default profile: the configuration in
`~/.config/mnemonic/config.toml`, the store and the log in `~/.mnemonic`. If
`MNEMONIC_HOME` is set, or `--home` is passed, every path below is that
profile's instead. Check before you edit anything.

## Rules

1. Never move, rename or delete the human's folders yourself. Propose the
   layout, show what would move, and let them do it or approve each move.
2. Ask before installing Rust or other system packages, and before editing an
   agent's global configuration.
3. `workspace` is an absolute path. `folder` is given relative to the
   workspace, and a plan's `authority` relative to its project.
4. On a machine that already runs mnemonic, take a backup before anything
   else: `mnemonic backup <dir>`. A newer binary upgrades the store on first
   use and an older binary then refuses it.
5. Report what you find. Do not edit the configuration to make a message go
   away.

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

**Every child folder counts.** A folder of downloads, an archive or a scratch
folder that sits beside the projects becomes a project too. Keep only projects
in that folder.

**The key of a project** is its folder name in lower case, with every run of
characters that are not letters or digits turned into one dash and no dash at
either end. `My App` and `my-app` both give `my-app`. Two folders that give
the same key cannot both be projects: the second one is refused.

**A project inside a project.** A folder directly inside a project that
carries one of these markers is a part of it and is tracked under its parent:
`.git`, `openspec`, `Cargo.toml`, `package.json`, `Package.swift`,
`pyproject.toml`, `go.mod`. Only that one level is looked at:
`web-client/packages/ui` is not a part.

**A path names a project by the folder that follows the name of the folder of
projects**, wherever the checkout lies. With the layout above, any path that
contains `projects/api/` names the project `api`: in the workspace, in a git
worktree, in a review copy. Two things follow:

- Give the folder of projects a name that does not occur by accident in other
  paths. `projects`, `code` or `work` are fine. `src` or `lib` are not.
- A worktree or a second checkout is attributed only when its path keeps that
  segment. `/any/where/feature-x/projects/api` works.
  `/any/where/api-feature-x` does not.

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

A project is named by a path that holds the `folder/project` segment. A
relative file name names nothing.

Inside a stretch of work, the time up to the next record belongs to the
project the session was on. A pause as long as the idle time belongs to no
one. Time that no rule could give to a project is shown as time without a
project. It is never spread over the others.

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
  and to bring the others in. Do not build it out of symbolic links.
- **Is everything in that folder a project?** List the children that are not.
  The human moves them out, or accepts them as projects. A project can be
  archived later; it cannot be hidden by configuration.
- **Are the names right?** The name of the folder is the name the human will
  see everywhere. Renaming a folder later is fine: the former name is given to
  the configuration (step 4) and history follows.
- **Do two folders give the same key?** `My App` and `my-app` do. One of them
  must be renamed.
- **One repository or many?** Both work. A monorepo whose projects sit in one
  folder: the workspace is the repository root and `folder` is that folder. A
  plain folder of separate repositories: the workspace is that folder and
  `folder` is left out.
- **Where do worktrees go?** See the rule about paths above. Tell the human if
  their worktrees would not be attributed and propose a location that keeps
  the segment.

Write down two values and have the human confirm them:

- `WORKSPACE`: the absolute path of the workspace.
- `FOLDER`: the folder of projects inside it, or nothing when the workspace
  itself holds the projects.

## 3. Build and install

Preconditions:

```bash
cargo --version && cc --version
```

- No `cargo`: ask the human, then install Rust with rustup.
- macOS, `cc` exits with code 69: the Xcode license is pending. The human runs
  `sudo xcodebuild -license`. You cannot.
- Network access is needed once: the first start downloads the embedding
  model, about 1 GB on disk.

From the directory that contains this repository's `Cargo.toml`:

```bash
cargo install --path . --locked
mnemonic --version
```

The binary lands in `~/.cargo/bin/mnemonic`. The build can take 10 to 20
minutes.

## 4. Configure

Create the configuration if there is none:

```bash
mnemonic init
```

Open `~/.config/mnemonic/config.toml`. Set `enabled = true` under `[ui]`, and
add that section if the file has none: the menu-bar widget and the app read
the loopback API, which is off by default. Then add the projects:

```toml
[projects]
workspace = "/absolute/path/to/workspace"
folder = "projects"        # leave out when the workspace itself holds them

[projects.aliases]         # optional second names: alias = "folder"
# web = "web-client"

[projects.former]          # optional: a name a folder had before
# old-name = "new-name"
```

Without the `[projects]` section no folder is discovered. Memory works, and
time is attributed the earlier way, by the memories linked to each project.
There are no live projects.

## 5. Start the daemon and check what it found

```bash
mnemonic start -d
mnemonic status
grep "Projects:" ~/.mnemonic/daemon.log | tail -1
```

The log line reads `Projects: P in the workspace (M folders mapped, R refused,
A aliases unset)`. P counts the projects found, parts included. M also counts
the former names of the configuration. Compare P with the folders the human
confirmed in step 2 plus their parts.

- `refused` above zero: a folder was not taken. Two folders give the same key,
  a name gives an empty key or is not valid text, or a name looks like a
  secret and the store will not keep it. Find which and tell the human.
- `aliases unset` above zero: an alias or a former name of the configuration
  could not be set. It points to a folder that is not a project, it collides
  with another name, or the folder a former name speaks of still exists.

The list of folders is read at start and again within ten minutes of a folder
being added or removed. Restart the daemon to see a new project at once.

No job is installed to start the daemon at login. With Claude Code, the hook
of step 7 starts it with the first session after a reboot. With Codex alone
nothing does: the human runs `mnemonic start -d`, or sets up a login job.

## 6. Bring in history

The agents' transcripts that are already on disk can be read once, so that
time by project does not begin empty:

```bash
mnemonic attribute replay --dry-run --days 14
```

This prints the hours by project the rules give for the last 14 days and
writes nothing. Show it to the human: it is the first honest look at whether
the layout works. A large share of time without a project means sessions were
started outside the projects (step 8). Then, on their OK:

```bash
mnemonic attribute backfill
```

It reads the transcripts of the agents mnemonic watches (Claude Code and
Codex), can be interrupted and run again, and a second run reads only what
was added since. It can run while the daemon runs.

History gives the agents' time by project. The human's own time is estimated
from what mnemonic records while it runs, so it begins with the install.

## 7. Connect the agents

For Claude Code, two things: a hook that starts the daemon and loads context
at the start of a session, and the MCP server that gives the agent the memory
tools. For Codex, the MCP server. The exact snippets are in the README, under
"Use with Claude Code" and "Use with Codex". Offer the optional block for the
human's `CLAUDE.md` described there; do not add it silently.

Codex transcripts are watched by default (`watchers.codex_enabled`).

## 8. Tell the human how to start sessions

This is the part that decides whether the numbers are right.

- **Start each agent inside the project's folder**, or deeper. A session
  started at the workspace root, or in the home folder, begins without a
  project and stays so until a rule of step 1 gives it one.
- **One project per session** gives the cleanest time. A session that moves
  to another project is followed by the rules of step 1. The time before the
  record that shows the move stays with the earlier project.
- **Worktrees** keep the `folder/project` segment in their path.
- **Sessions run by a program** are shown as automated when the transcript
  says so (a Claude Code SDK entrypoint, a Codex exec or subagent run). What
  they write is not captured as the human's own decisions.

If the human keeps an instruction file for their agents, offer one line for
it: start sessions inside the project folder, one project per session.

## 9. Plans (optional)

A project card can show where the plan stands. mnemonic only reads files that
are already there. It never runs a tool and never writes to a project. There
are three kinds of plan file:

- **A task registry**: `docs/engineering/tasks.json` or `tasks.json` at the
  project root. Either a list of tasks, or an object with `tasks` and an
  optional `releases` map from a release to the ids of its tasks. A task has
  `id`, `title` and `status`, and may name its `unit` or `release`. The
  statuses are `backlog`, `todo`, `planned`, `ready`, `in_progress`, `review`,
  `blocked` and `done`. Any other status puts the unit in error.
- **Checkbox files**: `openspec/changes/<change>/tasks.md`, one unit per
  change, tasks as `- [ ]` and `- [x]` lines. Only `x` marks a task done. A
  change with a `proposal.md` and no tasks is shown as proposed.
- **A pipeline table**: `PIPELINE.md` at the project root, a Markdown table
  with a `status` column. It shows counts by status, not a percentage.

The first of these that applies decides which one a project uses: what the
configuration names for the project (below); a task file that the project's
`openspec/config.yaml` declares with a line `task_authority: <relative path>`;
a task registry; a pipeline table; checkbox files. A project with none shows
its sessions and hours and "No plan recorded". That is a correct state, not an
error.

The stage of a unit is one of proposed, planned, building, review, blocked and
done. A unit whose plan file has not changed for 14 days and that has no live
session is also marked stale.

Check what mnemonic reads for one project, without a daemon or a store. The
command applies the default choice and ignores the configuration, so use it
before you add any override:

```bash
mnemonic plan-counts --root /absolute/path/to/workspace/projects/api
```

Compare the counts with the plan file. If the provider is guessed wrong, name
it in the configuration, under the key of the project:

```toml
[projects.plans.api]
provider = "registry"           # registry, checkbox or funnel
authority = "docs/tasks.json"   # relative to the project
# receipt = "build/gate.json"   # one receipt file instead of target/checks
# gate_required = true
```

A registry is always read from the project's own folder. For checkbox files,
`checkout = "/absolute/path"` names the checkout they are read from while no
session is live.

**Gate receipts.** When a project records the result of its checks, the card
shows it. mnemonic reads `target/checks/<unit>/<run>/verification.json` and
takes the newest, where `<run>` is the Unix time of the run. A receipt is JSON
with `change` (the unit) and `passed` (true or false).

A receipt that passed counts as passed only while it names what it tested and
the project still holds exactly that: `candidate_subtree`, the git tree of the
project's folder that was tested, must equal the tree at the checkout's head.
Otherwise the gate is shown as stale. A single receipt file named with
`receipt` may give `commit` instead, and is fresh while that commit is the
head. Three failed runs in a row block the unit. A passed gate is never
inferred from a full bar: without a receipt the card says the gate is not
recorded.

## 10. The menu-bar widget (macOS)

```bash
cd clients/macos
swift build -c release --product MnemonicBar
.build/release/MnemonicBar
```

Its title shows the count of live projects and, once the daemon has counted
them, the count of memories. The first page lists the projects in work. If it
says the API is off, the configuration does not enable `[ui]` (step 4). If it
says the daemon is unreachable, the daemon is not running or was not restarted
after step 4. The human adds the binary to Login Items if they want it at
login.

## 11. Acceptance test

Report each result to the human.

1. `mnemonic doctor` shows the daemon running, the socket accepting
   connections and the loopback API responding. A warning that the current
   folder holds no git repository is fine.
2. The log line of step 5 counts the folders the human confirmed.
3. The live snapshot answers. The token is read from its file and is never
   printed:

   ```bash
   curl -s -H "X-Mnemonic-Token: $(cat ~/.mnemonic/auth.token)" \
     http://localhost:3737/api/live
   ```

   The reply is JSON with `projects`, `unknown` and `today`, and `memories`
   once they are counted. The port and the token file are the defaults;
   `[ui]` can change both.
4. Start an agent session inside one project folder and send one message.
   Within about 20 seconds that project is in `projects` of the reply above,
   `running` or, when the turn has already ended, `waiting`, and the widget
   shows it.
5. Start a second session in the home folder. It is counted under `unknown`
   (a session without a project) until a rule of step 1 gives it one.
6. `mnemonic attribute replay --dry-run --days 7` prints hours by project.
7. In one agent call `memory_save` with a short test decision; in another
   call `memory_search` for it.

## 12. Keeping it right

- **A new project**: create its folder in the folder of projects. Nothing else
  is needed.
- **A renamed folder**: add `old-name = "new-name"` under `[projects.former]`,
  rename the project's `[projects.plans.<key>]` table if it has one, and
  restart the daemon. The history of the project follows it. Parts inside it
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
| A project is missing from the board | It is not a visible child of the folder of projects, it was refused, or it is archived, ignored or merged | Check step 2, the log line of step 5 and the counts under the board |
| Sessions without a project | They were started outside every project and no tool call named one | Step 8 |
| Most time is without a project | The sessions of that period were started outside the projects, or `folder` is wrong | Check `[projects]`, run the dry run of step 6 |
| A worktree's sessions are not attributed | Its path does not hold the `folder/project` segment | Move the worktree, see step 1 |
| The widget says the API is off | The configuration does not enable `[ui]` | Step 4 |
| The widget says the daemon is unreachable | The daemon is not running, or was not restarted after step 4 | `mnemonic start -d`, then `mnemonic doctor` |
| Hours are empty before today | History was not read, or the hours are the human's own | `mnemonic attribute backfill` brings the agents' time. The human's own time begins with the install |
| A plan unit is blocked | A task has the status `blocked`, or the last three gate runs failed | Read the plan file and the receipts |
| A plan unit is in error | A task has a status outside the list, or the file cannot be read | `mnemonic plan-counts --root <project folder>`, step 9 |
| A command refuses the store as newer than itself | An older binary met a store a newer one upgraded | Update every binary, restart the agent windows that were open |
