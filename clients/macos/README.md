# Mnemonic Menu Bar Widget

Native macOS menu bar app for monitoring and interacting with the mnemonic daemon.

## Features

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

## Auto-start on Login

1. Build release: `swift build -c release`
2. Copy to Applications: `cp -r .build/release/MnemonicBar /Applications/MnemonicBar`
3. System Settings > General > Login Items > add MnemonicBar

## Architecture

- `App.swift` — NSStatusItem + NSPopover setup, menu bar icon with memory count
- `MnemonicService.swift` — calls `mnemonic` CLI via Process(), parses JSON stats
- `MenuBarView.swift` — SwiftUI view with all UI components
