# Demo data for the screenshots

Invented data only. Existing JSON keys and project identifiers are retained.
Display names, plan titles, task text and history titles use the vocabulary in
`clients/macos/Sources/MnemonicApp/Fixtures/FixtureVocabulary.swift`. No value
comes from the mockup, an owner profile or a running daemon.

`live.json` supplies the widget and the board's live cards. `project-board.json`
adds gate dates, plan providers and counts, waiting reasons, closure summaries
and board week totals. `project-detail.json` shows session record times, a stale
gate with a passed receipt, open and closed units, and a decision and correction.
`project-timeline.json` supplies one UTC day with overlapping Claude and Codex
segments, owner shares and unassigned owner activity. Its day values illustrate
the Timeline independently of the live snapshot's totals.

## Draw the five client pictures

The lead runs these commands from the crate root after building the current
clients in release. All three renderers below use synthetic data only and exit
without creating a live model, starting a daemon or making a request. A fresh
empty profile is sufficient; do not initialize or populate it from another store.

```bash
DEMO_RUN="$(mktemp -d /tmp/mnemonic-demo.XXXXXX)"
DEMO_PROFILE="${DEMO_RUN}/profile"
mkdir -p "${DEMO_PROFILE}"
TZ=UTC MNEMONIC_HOME="${DEMO_PROFILE}" clients/macos/.build/release/MnemonicBar \
  --render-live-fixture docs/screenshots/demo/live.json "${DEMO_RUN}/widget"
TZ=UTC MNEMONIC_HOME="${DEMO_PROFILE}" clients/macos/.build/release/MnemonicApp \
  --render-project-fixtures docs/screenshots/demo "${DEMO_RUN}/app"
TZ=UTC MNEMONIC_HOME="${DEMO_PROFILE}" clients/macos/.build/release/MnemonicApp \
  --render-timeline-fixtures docs/screenshots/demo "${DEMO_RUN}/timeline"
```

Each renderer writes both appearances and its additional empty or failure
states. The project renderer also creates a longer board with its own synthetic
sample rows; the documentation uses `projects-dark.png`, which uses only the
supplied rows. Timeline reads only its day file, so its demo picture has no live
session count or empty registry lanes. It still draws the focus, owner lane,
recorded agent lanes and day totals. The current Timeline view does not draw
board week totals even when a registry response is held.

| Renderer output under DEMO_RUN | Picture in docs/screenshots | Referenced by |
|---|---|---|
| `widget/widget-live-dark.png` | `widget-live.png` | Root README |
| `widget/widget-live-light.png` | `widget-live-light.png` | Root README |
| `app/projects-dark.png` | `app-projects.png` | Root README and macOS README |
| `app/project-dark.png` | `app-project.png` | macOS README |
| `timeline/timeline-full-dark.png` | `app-timeline.png` | macOS README |

After visual inspection, copy the files without resizing or metadata rewriting
so each documentation picture is byte for byte the renderer output:

```bash
cp "${DEMO_RUN}/widget/widget-live-dark.png" docs/screenshots/widget-live.png
cp "${DEMO_RUN}/widget/widget-live-light.png" docs/screenshots/widget-live-light.png
cp "${DEMO_RUN}/app/projects-dark.png" docs/screenshots/app-projects.png
cp "${DEMO_RUN}/app/project-dark.png" docs/screenshots/app-project.png
cp "${DEMO_RUN}/timeline/timeline-full-dark.png" docs/screenshots/app-timeline.png
```

The lead records the source commit and executable digests beside the render
evidence. These instructions do not claim the pictures have been regenerated.
The project renderer uses the host calendar and default bitmap scale; compare
bytes on the same Mac with the same environment. The named state renderer has
its own fixed calendar and scale but does not read this demo folder.

## Other pictures already referenced by the root README

These are part of the picture inventory, but none is drawn from this folder by
an app fixture renderer. No replacement is added by this documentation task.

| Existing picture in docs/screenshots | Source or command limit |
|---|---|
| `widget-hero.png` | The older `widget-hero.html` composition contains its own data. Its header describes a browser capture command; it does not consume the demo JSON or draw the current native widget. |
| `01-status.png` | Terminal capture of `mnemonic status`. No renderer or synthetic terminal input for it exists in this demo folder. |
| `02-eval-default.png` | Terminal capture of `mnemonic eval`. No renderer or synthetic evaluation input for it exists in this demo folder. |
| `03-eval-nograph.png` | Terminal capture of `mnemonic eval --no-graph-hop`. No renderer or synthetic evaluation input for it exists in this demo folder. |

The widget's live preview option reads a selected profile and daemon. It cannot
reproduce these pictures from the demo folder and is not part of this procedure.
Graph fixtures live in `clients/macos/Tests/Fixtures/Graph`; this folder has no
graph input set and the documents add no graph picture.
