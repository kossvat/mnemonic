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
        // Hide dock icon
        NSApp.setActivationPolicy(.accessory)

        // Create status bar item
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)

        if let button = statusItem.button {
            updateButton(button)
            button.action = #selector(togglePopover)
            button.target = self
        }

        // Create popover
        popover = NSPopover()
        popover.contentSize = NSSize(width: 340, height: 500)
        popover.behavior = .transient
        popover.delegate = self
        popover.contentViewController = NSHostingController(
            rootView: MenuBarView(service: service)
        )

        // Start polling
        service.startPolling(interval: 10)

        // Update button text when stats change
        Timer.scheduledTimer(withTimeInterval: 10, repeats: true) { [weak self] _ in
            guard let self = self, let button = self.statusItem.button else { return }
            self.updateButton(button)
        }
    }

    private func updateButton(_ button: NSStatusBarButton) {
        let attachment = NSTextAttachment()
        if let image = NSImage(systemSymbolName: "brain.head.profile", accessibilityDescription: "Mnemonic") {
            let config = NSImage.SymbolConfiguration(pointSize: 13, weight: .medium)
            attachment.image = image.withSymbolConfiguration(config)
        }

        let attrString = NSMutableAttributedString(attachment: attachment)

        if service.stats.total > 0 {
            let countStr = NSAttributedString(
                string: " \(service.stats.total)",
                attributes: [
                    .font: NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .medium),
                    .baselineOffset: 1
                ]
            )
            attrString.append(countStr)
        }

        // Orange dot when daemon silent 2+ hours
        if let hours = service.stats.silentHours, hours >= 2.0 {
            let dot = NSAttributedString(
                string: " ●",
                attributes: [
                    .font: NSFont.systemFont(ofSize: 6),
                    .foregroundColor: NSColor.orange,
                    .baselineOffset: 4
                ]
            )
            attrString.append(dot)
        }

        button.attributedTitle = attrString
    }

    @objc private func togglePopover() {
        if let button = statusItem.button {
            if popover.isShown {
                popover.performClose(nil)
            } else {
                service.refresh()
                popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)

                // Activate app to get focus
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }
}
