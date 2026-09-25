import SwiftUI
import AppKit

@main
struct MnemonicBarApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        // Empty — we only use the menu bar, no window
        Settings { EmptyView() }
    }
}

class AppDelegate: NSObject, NSApplicationDelegate, NSPopoverDelegate {
    private var statusItem: NSStatusItem!
    private var popover: NSPopover!
    private let service = MnemonicService()

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Headless design-review mode: render every page to PNG and exit
        // before the status item exists (doesn't disturb a running widget).
        if PreviewRender.runIfRequested() {
            exit(0)
        }

        // Hide dock icon
        NSApp.setActivationPolicy(.accessory)

        // Create status bar item
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)

        if let button = statusItem.button {
            updateButton(button)
            button.action = #selector(togglePopover)
            button.target = self
        }

        // Create popover — auto-sizes to the SwiftUI content so the
        // Compact/Expanded presets change height without manual math.
        popover = NSPopover()
        popover.behavior = .transient
        popover.delegate = self
        let host = NSHostingController(rootView: PagedContainerView(service: service))
        host.sizingOptions = [.preferredContentSize]
        popover.contentViewController = host

        // Background cadence — the popover delegate switches to 10s while
        // open (popoverWillShow / popoverDidClose below).
        service.setPollingForeground(false)

        // Update button text when stats change
        Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in
            guard let self = self, let button = self.statusItem.button else { return }
            self.updateButton(button)
        }
    }

    func popoverWillShow(_ notification: Notification) {
        service.setPollingForeground(true)
    }

    func popoverDidClose(_ notification: Notification) {
        service.setPollingForeground(false)
        if let button = statusItem.button { updateButton(button) }
    }

    private func updateButton(_ button: NSStatusBarButton) {
        let attachment = NSTextAttachment()
        if let image = NSImage(systemSymbolName: "brain.head.profile", accessibilityDescription: "Mnemonic") {
            let config = NSImage.SymbolConfiguration(pointSize: 13, weight: .medium)
            attachment.image = image.withSymbolConfiguration(config)
        }

        let attrString = NSMutableAttributedString(attachment: attachment)

        // Memory count (keep it — watching it grow is the point)
        // plus today's worked time, both visible without opening the
        // popover: "631 · 1h 13m".
        var parts: [String] = []
        if service.data.memoriesTotal > 0 {
            parts.append("\(service.data.memoriesTotal)")
        }
        let worked = service.data.workedTodaySeconds
        if worked >= 60 {
            parts.append(fmtDur(worked))
        }
        let label: String? = parts.isEmpty ? nil : parts.joined(separator: " · ")
        if let label {
            attrString.append(NSAttributedString(
                string: " \(label)",
                attributes: [
                    .font: NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .medium),
                    .baselineOffset: 1,
                ]
            ))
        }

        // Small red dot only when the daemon is stopped/not responding.
        if service.data.state == .stopped || service.data.state == .broken {
            attrString.append(NSAttributedString(
                string: " ●",
                attributes: [
                    .font: NSFont.systemFont(ofSize: 6),
                    .foregroundColor: NSColor.systemRed,
                    .baselineOffset: 4,
                ]
            ))
        }

        button.attributedTitle = attrString
    }

    @objc private func togglePopover() {
        if let button = statusItem.button {
            if popover.isShown {
                popover.performClose(nil)
            } else {
                // popoverWillShow flips polling to foreground (which also
                // refreshes) — no separate refresh needed here.
                popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)

                // Activate app to get focus
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }
}
