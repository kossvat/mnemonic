# Demo data for the screenshots

Invented data only: no project, plan or number here comes from a real machine.

`widget-live.png`, `widget-live-light.png` and `app-projects.png` in the folder
above are rendered from these files by the clients themselves, offline, with an
isolated profile (`PROFILE` is an absolute path to an empty folder of its own,
outside the default store and the default configuration folder):

```bash
mnemonic --home "$PROFILE" init
cd clients/macos && swift build -c release
MNEMONIC_HOME="$PROFILE" .build/release/MnemonicBar \
  --render-live-fixture ../../docs/screenshots/demo/live.json OUT/widget
MNEMONIC_HOME="$PROFILE" .build/release/MnemonicApp \
  --render-project-fixtures ../../docs/screenshots/demo OUT/app
```

The renderers draw every state in both appearances. The screenshots are
`widget-live-dark.png`, `widget-live-light.png` and `projects-list-dark.png`,
saved with their pixels only.
