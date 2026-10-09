import SwiftUI
import AppKit
import MnemonicShared

@main
struct MnemonicBarApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) var appDelegate

    var body: some Scene {
        // Empty — we only use the menu bar, no window
        Settings { EmptyView() }
    }
}

@MainActor
class AppDelegate: NSObject, NSApplicationDelegate, NSPopoverDelegate {
    private var statusItem: NSStatusItem!
    private var popover: NSPopover!
    private let service = MnemonicService()
    private lazy var live = WidgetLiveController(profile: .current)
    private var fixturePopover = false
    private var fixtureAnchor: NSWindow?

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Headless design-review mode: render every page to PNG and exit
        // before the status item exists (doesn't disturb a running widget).
        if PreviewRender.runIfRequested() {
            exit(0)
        }
        if CommandLine.arguments.contains("--fixture-popover") {
            guard MnemonicProfile.current.isolated else { exit(2) }
            fixturePopover = true
        }

        // Hide dock icon
        NSApp.setActivationPolicy(.accessory)

        // Earlier builds kept the responses of the local API on disk.
        LegacyCache.removeForThisClient()

        // Create status bar item
        statusItem = NSStatusBar.system.statusItem(withLength: NSStatusItem.variableLength)

        if let button = statusItem.button {
            show(StatusTitle(), on: button)
            button.action = #selector(togglePopover)
            button.target = self
        }

        // Create popover — auto-sizes to the SwiftUI content so the
        // Compact/Expanded presets change height without manual math.
        popover = NSPopover()
        popover.behavior = fixturePopover ? .applicationDefined : .transient
        popover.delegate = self
        let host = NSHostingController(rootView: PagedContainerView(service: service, live: live))
        host.sizingOptions = [.preferredContentSize]
        popover.contentViewController = host

        // The title follows the data: the service tells it when what it
        // says differs, so no timer redraws the item.
        live.onTitle = { [weak self] title in
            guard let self, let button = self.statusItem.button else { return }
            self.show(title, on: button)
        }
        service.onDaemonAction = { [weak self] in
            Task { @MainActor in await self?.live.coordinator.request() }
        }

        Task { await live.start() }
        if fixturePopover {
            showFixturePopover()
        }
    }

    /// An automated run cannot assume a usable slot in a crowded menu bar.
    /// A small visible anchor hosts the same real popover and is measured
    /// with it. Only the delegate's actual did-show event writes "open".
    private func showFixturePopover() {
        guard let screen = NSScreen.main else { return }
        let bounds = screen.visibleFrame
        let window = NSWindow(
            contentRect: NSRect(x: bounds.maxX - 380, y: bounds.maxY - 36, width: 32, height: 24),
            styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        window.level = .floating
        let anchor = NSView(frame: NSRect(x: 0, y: 0, width: 32, height: 24))
        window.contentView = anchor
        fixtureAnchor = window
        popover.animates = false
        popover.contentSize = NSSize(width: 336, height: 680)
        window.orderFrontRegardless()
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            self.popover.show(relativeTo: anchor.bounds, of: anchor, preferredEdge: .minY)
        }
    }

    func popoverWillShow(_ notification: Notification) {
        service.opened()
    }

    func popoverDidShow(_ notification: Notification) {
        fixtureState("open")
        Task { [weak self] in
            guard let self, self.popover.isShown else { return }
            await self.live.setVisible(true)
        }
    }

    func popoverDidClose(_ notification: Notification) {
        service.closed()
        live.hide()
        fixtureState("closed")
    }

    private func fixtureState(_ state: String) {
        guard fixturePopover else { return }
        let file = MnemonicProfile.current.home.appendingPathComponent("fixture-popover.state")
        try? (state + "\n").write(to: file, atomically: true, encoding: .utf8)
    }

    private func show(_ title: StatusTitle, on button: NSStatusBarButton) {
        let attachment = NSTextAttachment()
        if let image = NSImage(systemSymbolName: "brain.head.profile", accessibilityDescription: "Mnemonic") {
            let config = NSImage.SymbolConfiguration(pointSize: 13, weight: .medium)
            attachment.image = image.withSymbolConfiguration(config)
        }

        let attrString = NSMutableAttributedString(attachment: attachment)

        // Only the live-project count appears beside the status icon.
        if let label = title.label {
            attrString.append(NSAttributedString(
                string: " \(label)",
                attributes: [
                    .font: NSFont.monospacedDigitSystemFont(ofSize: 11, weight: .medium),
                    .baselineOffset: 1,
                ]
            ))
        }

        // Small red dot only when the daemon is stopped/not responding.
        if title.alert {
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
                // popoverWillShow tells the service, which publishes what
                // it kept and refreshes: no separate refresh needed here.
                popover.show(relativeTo: button.bounds, of: button, preferredEdge: .minY)

                // Activate app to get focus
                NSApp.activate(ignoringOtherApps: true)
            }
        }
    }
}
