# Mnemonic macOS clients

`MnemonicBar` is the native menu-bar widget; `MnemonicApp` is the full window.
They share the selected local profile and response models. The app has its own
light and dark palette; the widget keeps its shared theme.

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

## Frame and navigation in MnemonicApp

The sidebar groups the screens that open: NOW holds Projects and Timeline;
MEMORY holds Memories, Search, Entities and Memory Map; EXPLORE holds Project
Graph, Overview and Manage. Settings remains a separate window. The footer
names the daemon state. Review, Agents, Pulse, System, a review count and an
idle budget have no entry or value yet. Owner time is amber and agent time is
blue in both appearances.

## Projects in MnemonicApp

The board opens with the date, today's time and the last seven days when
served, with separate owner and agent values. The board week comes from the
server's daily totals, with parallel agent work counted once, rather than a
sum of project rows. Owner time is labelled as estimated.

Active cards show agent kinds and counts, the session count and longest
session, time today and over seven days, the unit in focus, its stage, gate
and run date, and current task. A task plan with a positive total has a bar,
percent and done/total count. A funnel has status counts and a weekly count;
a zero task total has no progress bar or percent. A card ends with one fact:
waiting reasons first, otherwise the last closed unit, otherwise the open
unit count. A cut snapshot is named and suppresses counts by agent kind and
the longest session. Sessions without a known project are named separately.

Other active projects are ordered by last activity. Activity older than 30
days starts folded; live projects and rows with an unknown activity date stay
visible. Archived, ignored and merged rows have separate counts outside the
board. Long names and tasks are cut to fit fixed card sizes. Opening a card
or row opens that project.

![Projects board](../../docs/screenshots/app-projects.png)

## The page of a project in MnemonicApp

The header shows the name, state, today's owner and agent time and live
session count. The first registry unit with a positive total supplies the
header's registry task count. The unit in focus is the first served unit with
a current task, otherwise the first unit, matching the board's plan line.

Now, Plan, Gate, Time and History remain the five data requirements. The page
arranges them as a focus panel, units beside tasks and gate, live sessions,
and Recent work. Open units use compact done/total counts and progress bars;
the last closed unit appears separately. Older responses without closure
fields list Units without an open count. Finished tasks start folded; the
current task, next task and queued count stay visible. Funnel units show
status counts without task percentages. Plan details hold the stage, provider,
read time and checkout alternatives; reader errors remain visible.

Gate status, run date, the result of the receipt and checkout measurements
are separate facts. A stale gate stays stale even when that receipt passed.
Sessions show the agent, state, elapsed time and age of the last record when
served; their drivers are under Session details. Recent work keeps every
returned day in three columns, newest first, with owner and agent time and
returned decision or correction titles. History is limited to ten entries
before filtering, so a day without an entry makes no claim about its history.
Top terms remain beside the units. Project graph and Memory actions in the
header remain outside this stage.

Missing fields from an earlier daemon omit their blocks. Recorded null or
empty values explain their absence, loading shows no invented number, and
failed reads keep their own explanation. Zero remains zero where applicable.
Canonical detail responses are accepted for merged routes; live failure hides
old sessions while held plan, time and history remain available.

![Project page](../../docs/screenshots/app-project.png)

## Timeline in MnemonicApp

Timeline carries its selected date in the route. A strip of the whole local
day sits above a two hour focus, with a whole day view available too. Today
opens on the two hours ending at the clock. A past day follows the latest
served agent or available owner interval end, or shows the whole day if there
is neither. Clicking the strip moves the focus within the real day bounds,
including days of 23 or 25 hours.

The owner lane comes first, even without a transcript, with estimated project
shares on their actual simultaneous interval. Project lanes distinguish Claude
and Codex by shape and label as well as tone. Painted widths follow time;
blocks crossing the focus edge are clipped and marked as continuing. Labels
shorten to fit inside their blocks. A project in focus starts with at most
three packed rows and opens the rest through a count of hidden segments.
An empty lane appears only for an active, unmerged registry project that the
day answer also lists. A missing activity store stays unavailable, and zero
activity remains zero.

Day totals count parallel agent work once and show owner time and time by
project. The held live snapshot supplies the live session count. Seven day
totals are shown on Projects; Timeline currently shows day totals only.
Recorded intervals start folded behind their count and retain the exact
bounds served by the daemon.

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

![Timeline](../../docs/screenshots/app-timeline.png)

The demo commands and the source of every documentation picture are listed in
[the screenshot guide](../../docs/screenshots/demo/README.md). Regenerating and
visually checking these pictures is part of the lead's stage verification.

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

Synthetic previews use explicit fixture JSON before creating a live app model.
Native budget tooling uses `--fixture-window --fixture-graph` only with an
isolated profile. Its marker
requires a visible Graph window and completed painting of the current nonempty
snapshot containing both synthetic projects and their plan units. The frame
budget remains BLOCKED: the release renderer measured a maximum of 8.073 ms
against the 8 ms limit, with P95 7.551 ms. The look of the graph and its
native budget are not final.

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
receipt; run the script again to regenerate it. The bundle is staged under
`build/Mnemonic.noindex`, which Spotlight does not index, so search finds
only a copy installed with `--install`. Its icon is made from
`assets/app-icon.png` at the crate root. `--icon FILE` selects another source,
relative to the invoking directory when the path is relative. Missing icon
input, `sips` or `iconutil` stops the script before compilation. The icon has
16, 32, 128, 256 and 512 point representations at both scales. A failed
conversion removes the unfinished bundle. The script refuses links in its
build and staging paths and removes the bundle it used to leave directly in `build`.
The final bundle is staged inside `build/Mnemonic.noindex`; `--install`
also copies it to `~/Applications`.

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

## MnemonicApp render options

Every option below exits after rendering, before creating the live app model.
Select a dedicated absolute profile with `--home PROFILE` or `MNEMONIC_HOME`.
An empty profile directory is enough; no daemon or profile initialization is
needed. DIRECTORY is a folder of synthetic inputs; OUTPUT is an output folder.

| Option and arguments | Input and output |
|---|---|
| `--render-fixture-states OUTPUT [--only ID]` | Built in states from `Sources/MnemonicApp/Fixtures`; writes `ID-light.png` and `ID-dark.png`. The optional selector renders one exact state ID. |
| `--render-project-fixtures DIRECTORY OUTPUT` | Reads `project-board.json`, `project-detail.json` and `live.json`; writes `projects`, `projects-list`, `project`, `projects-empty` and `project-unavailable`, each with `-light.png` and `-dark.png`. |
| `--render-timeline-fixtures DIRECTORY OUTPUT` | Reads `project-timeline.json`; writes `timeline-full`, `timeline-empty` and `timeline-activity-only`, each in both appearances. |
| `--render-graph-fixtures DIRECTORY OUTPUT` | Reads `graph-full.json`, `graph-expanded.json`, `graph-stale-only.json`, `graph-empty.json`, `graph-project.json`, `graph-evidence.json`, `graph-outside.json`, `graph-history.json` and `memory-map.json`; writes `graph-full`, `graph-empty`, `graph-error`, `graph-stale-only`, `graph-expanded` and `memory-map`, each in both appearances. These inputs are in `Tests/Fixtures/Graph`. |

The named states cover the frame, board, project and Timeline, including
loading, each connection failure, absent and unserved fields, long and crowded
content, gate results, corrections, and days of 23 and 25 hours. The list,
expected presentation values and input digests live beside the fixture
vocabulary. The state renderer uses a fixed clock, Gregorian calendar,
en_US_POSIX locale and UTC, except for the two clock change states. It draws
at 1240 points wide and scale 2, normally 800 points high, with taller states
declared in the list. OUTPUT must be empty or contain only the selected
states' expected filenames. It writes pictures only.

## MnemonicBar render options

| Option and arguments | Input and output |
|---|---|
| `--render-live-fixture FILE DIRECTORY` | Reads a synthetic live snapshot with an isolated profile; writes `widget-live`, `widget-empty`, `widget-api-off`, `widget-unreachable`, `widget-unauthorised`, `widget-earlier-daemon` and `widget-undecodable`, each with `-light.png` and `-dark.png`. |
| `--render-previews [DIRECTORY]` | Reads the selected live profile and daemon for the secondary widget pages. The directory defaults to `/tmp/mnemonic-previews`. This is not a demo fixture renderer. |

## Fixture windows and readiness

`MnemonicApp --fixture-window` opens the native Projects window for the
isolated budget scenario. It writes `fixture-window.state` under that profile.
Ready requires a visible Projects route, successful board and live responses,
and both synthetic cards, `fixture-00` and `fixture-01`, with their 1/2 plans.
Both cards must have completed painting for the board and snapshot held now.
Decoded responses alone do not establish readiness. Hiding the window or
replacing its snapshot revokes readiness until a fresh completed paint. Equal
marker states are not rewritten, so the sampler can detect a hide and reopen.
The paint probe adds no timer or persistent observer.

`--fixture-window --fixture-graph` selects the graph budget window instead.
`MnemonicBar --fixture-popover` holds the widget's synthetic budget popover
and writes its visibility marker. These window modes require an isolated
profile and remain open; they are not render options. A picture alone cannot
pass the native budget. MN-42-11 still needs the owner at the unlocked Mac.

## Auto-start on Login

1. Build release: `swift build -c release`
2. Copy it where you keep your own apps: `mkdir -p ~/Applications && cp .build/release/MnemonicBar ~/Applications/MnemonicBar`
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
- `LiveFixturePreview.swift` renders all connection states from synthetic
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
- `AppFixturePreview.swift` renders supplied synthetic data without a
  production model or transport. `--fixture-window` supplies native visibility
  evidence for the app budget; fixture PNGs alone do not establish a budget pass.
