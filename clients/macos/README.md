# Mnemonic macOS clients

`MnemonicBar` is the native menu-bar widget; `MnemonicApp` is the full window.
They share the selected local profile, response models, and theme.

## Features

- Live projects first: agent kinds and counts, separate owner and agent
  time, and plan unit, task and progress.
- The menu-bar title counts live projects. Off, unreachable, unauthorised,
  earlier-daemon and invalid-response states have distinct explanations.
- Real-time daemon status (running/stopped, PID)
- Memory stats: Decisions, Feedback, Notes counts
- Click stat cards to filter by type
- Expandable memory rows with full text and importance
- Quick Save: add memories directly from the menu bar
- Search memories
- Daemon control: Start/Stop from the widget
- Open log file, generate context
- Last activity indicator with alert when daemon is silent 2+ hours
- Copy memory text to clipboard

## Projects in MnemonicApp

The app opens on live project cards with agent kinds and states, separate
owner and agent time over the last 7 days, and the current plan. Other active
projects are ordered by last activity. Activity older than 30 days starts folded; live
projects and rows with an unknown activity date stay visible. Archived,
ignored, and merged rows have separate counts outside the board.

Each project has five fixed sections: **Now, Plan, Gate, Time, and History**.
Plan units keep their recorded stage and task disclosures; status tables show
counts without completion percentages. Gate receipts and checkout measurements
are separate facts. Time shows the latest 14 daily rows with owner and agent
columns; missing attribution stays Unknown. History includes memory titles
and IDs. Empty sections name what is not recorded. Canonical detail responses
are accepted for merged routes, and live failure hides old sessions without
discarding held plan, time, and history.

The project screens are provisional. Owner visual acceptance and the native
open-window budget remain pending; current evidence is in the live-projects
design, section 19.

## Timeline in MnemonicApp

Timeline carries its selected date in the route. Project lanes show recorded
agent segments with different shapes and labels for Claude and Codex. The
owner lane shows activity even when there is no transcript, with estimated
project shares kept on their true simultaneous interval. A missing activity
store stays unavailable; a day without activity or segments stays empty.
The time axis uses the actual local day, including 23- and 25-hour days.

Today uses the existing signal and 30-second visible refresh. Past-day reads
are cached in memory for up to 31 dates; explicit refresh and corrections
invalidate them. Hiding the window stops requests, and a reply from an earlier
date, visit or endpoint cannot replace the current screen. After midnight the
previous day's recurring refresh stops. Today navigation resolves its date
when clicked.

Selecting a block only opens details. Apply submits an attribution correction;
Undo removes the last saved correction. Point segments can be pinned to a
whole transcript only with an explicit action. The existing correction store
has whole-second precision, so a fractional activity boundary is not silently
expanded or rounded into an editable interval.

Synthetic preview rendering uses `--render-timeline-fixtures DIRECTORY OUTPUT`
with an isolated profile and the supplied `project-timeline.json`. It renders
full, empty and activity-only days in light and dark appearances. Visual
acceptance remains with the owner.

## Graphs in MnemonicApp

Project Graph draws the daemon's stable project, part, unit, technology and
person positions. Shape identifies kind; tone identifies recency; node size
uses agent hours. Unknown owner or agent hours remain unavailable. The initial
viewport fits the project core. Core, Fit all and zoom controls change the
viewport without moving stored nodes. Selecting a hub opens its plan and top
terms; an action opens the full project page. Selecting a relationship loads
its actual supporting memories on demand.

Stale registry projects and outside-registry entities have separate lists.
Archives require an explicit action and retain Undo. Archive history supports
restoring entities, including retained marks for entities that no longer exist.
A write whose refresh cannot be confirmed keeps Retry and Undo available.
Evidence retains at most 30 memories; outside and history lists retain at most
100 rows each, show discarded counts and support restarting from the first page.

One native canvas and painter serve Project Graph and Memory Map. Memory Map
keeps its original starting spiral and node radii. Neither graph simulates
motion or schedules a timer. Graph reads occur on opening, an existing daemon
signal or explicit refresh, with stale replies discarded across visibility,
selection, focus and endpoint changes. The transport caches one overview and
up to eight expansions in memory with entity tags.

Synthetic previews use `--render-graph-fixtures DIRECTORY OUTPUT` with explicit
fixture JSON, before creating a live app model. Native budget tooling uses
`--fixture-window --fixture-graph` only with an isolated profile. Its marker
requires a visible Graph window and completed painting of the current nonempty
snapshot containing both synthetic projects and their plan units. The frame
budget remains BLOCKED: the release renderer measured a maximum of 8.073 ms
against the 8 ms limit, with P95 7.551 ms. Owner visual acceptance and the native
graph budget remain pending; evidence is in the live-projects design, section 19.

## Requirements

- macOS 14+ (Sonoma)
- Swift 5.9+
- `mnemonic` CLI installed (`~/.cargo/bin/mnemonic`)

## Build & Run

```bash
cd clients/macos
swift build
.build/debug/MnemonicBar
```

The app appears as a brain icon in the menu bar. Click to open the popover.
Run `.build/debug/MnemonicApp` after the same build for the full app.
For widget-to-app navigation, run `./scripts/build-app.sh` without
`--install`. It creates a release app bundle and a routing-capability receipt
bound to the release executable. The widget prefers this verified local
executable, then a compatible installed bundle. A missing or obsolete build
shows a build/update message. A rebuild invalidates an older executable's
receipt; run the script again to regenerate it.

## Running against an isolated profile

By default the widget and the app talk to the default store. To point them
at an isolated profile, start them with the same selection the daemon takes:

```bash
MNEMONIC_HOME=/absolute/path/to/profile .build/debug/MnemonicBar
```

or with `--home /absolute/path/to/profile`. The client then takes everything
from that folder and nothing from the default store: the port of the local
API (`[ui] port` in the profile's `config.toml`), the token, the log, and its
own preferences. It keeps no response on disk, and it names the profile to
every `mnemonic` command it runs. A selection that cannot be used ends the
client with a message; it never falls back to the default store.

Two more variables are for tests and tooling: `MNEMONIC_ENDPOINT` names the
endpoint outright (only an http address of this machine is accepted), and
`MNEMONIC_BIN` names the `mnemonic` binary to run.

## Auto-start on Login

1. Build release: `swift build -c release`
2. Copy to Applications: `cp -r .build/release/MnemonicBar /Applications/MnemonicBar`
3. System Settings > General > Login Items > add MnemonicBar

## Architecture

- `App.swift` manages the status item, popover visibility and live count.
- `WidgetLiveController.swift` uses the shared live coordinator and signal
  subscriber. It never starts a separate polling loop.
- `LiveProjectsPage.swift` renders the live cards; `PagedWidget.swift` mounts
  only the selected visible page. The five secondary pages stay available.
- The `MnemonicService` files load secondary pages on appearance, with at
  most one automatic command fallback batch per opening and bounded explicit
  actions. Replies from an earlier appearance cannot publish to a later one.
- `LiveFixturePreview.swift` accepts `--render-live-fixture FILE DIRECTORY`
  with an isolated profile and renders all connection states from synthetic
  input. It creates no production API client or command request.
- `--fixture-popover` opens and holds the synthetic budget popover. It also
  requires an isolated profile and writes a visibility marker there, allowing
  the sampler to reject a run that measured a closed window.
- `MnemonicApp/AppProjectController.swift` owns visible board/detail reads
  and rejects late replies from an earlier route or visit. Projects, project
  detail, and Overview share the coordinator's 30-second visible refresh.
  The app disables hidden requests, including safety and label timers.
  Native window visibility includes occlusion, minimization, and app hiding;
  Overview no longer owns a separate repeating timer. Delayed signals,
  responses, and fired timer tasks cannot refresh a later visible visit.
  Appearance reads coalesce. Project signals, visible refreshes and explicit
  refreshes invalidate an in-flight answer and retain one serial replacement;
  legacy mutation callbacks and explicit refreshes do the same.
- `ProjectsView.swift`, `ProjectPage.swift`, and `ProjectPresentation.swift`
  render data without starting requests or timers. New dynamic text is verbatim.
- Shared `LocalAPI` project reads use the profile transport and token, a
  five-second request limit, and one token reload after a 401. Project reads
  do not use the live response tag. Endpoint edits accept only local HTTP
  origins; isolated or explicitly named endpoints stay fixed.
- `AppLink` carries Projects, project, and Search routes for an expected
  profile identity, with no home, endpoint, or token. Launches use `--open-url`;
  a running app can receive a matching distributed notification. AppLinkReadiness
  scopes the startup acknowledgement to the profile and one launch nonce,
  retaining the latest click until the subscriber exists. The widget checks an
  explicit protocol capability before launching a dashboard. On a later click
  after a startup timeout, it drops pending routing and asks for a build or
  update, holding the existing process until exit to avoid duplicate launches.
  The timeout adds no timer or poll. DashboardActivation
  restores the dashboard or opens its scene when only Settings remains.
- `AppFixturePreview.swift` accepts `--render-project-fixtures DIRECTORY OUTPUT`
  with an isolated profile and renders supplied synthetic data without a
  production model or transport. `--fixture-window` supplies native visibility
  evidence for the app budget; fixture PNGs alone do not establish a budget pass.
