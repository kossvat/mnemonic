import AppKit
import SwiftUI
import XCTest
import MnemonicShared
@testable import MnemonicBar

final class MountedPagesTests: XCTestCase {
    private static var home: URL!

    /// The views read preferences of the profile. The tests run in an
    /// isolated profile of their own, selected before anything resolves it.
    override class func setUp() {
        home = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-widget-\(UUID().uuidString)")
        try? FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        setenv("MNEMONIC_HOME", home.path, 1)
    }

    override class func tearDown() {
        try? FileManager.default.removeItem(at: home)
    }

    @MainActor
    func testOnePageIsMountedTheOneShown() {
        let service = MnemonicService(environment: WidgetEnvironment(
            fetch: { _, _ in nil },
            every: { _, _ in {} },
            background: { _ in },
            main: { work in work() }))
        _ = NSApplication.shared
        for page in [0, 1, 2] {
            let live = WidgetLiveController(clock: SystemLiveClock()) { .failed(.off) }
            live.present(.failed(.off), at: .distantPast)
            let host = NSHostingView(
                rootView: PagedContainerView(service: service, live: live, previewPage: page))
            host.frame = NSRect(x: 0, y: 0, width: 336, height: 680)
            let window = NSWindow(
                contentRect: host.frame, styleMask: [.borderless],
                backing: .buffered, defer: false)
            // Under ARC a window that releases itself when closed is
            // released twice.
            window.isReleasedWhenClosed = false
            window.contentView = host
            host.layoutSubtreeIfNeeded()
            RunLoop.main.run(until: Date().addingTimeInterval(0.2))
            XCTAssertEqual(PageMounts.shown, [page])
            live.hide()
            RunLoop.main.run(until: Date().addingTimeInterval(0.1))
            XCTAssertEqual(PageMounts.shown, [], "a closed popover kept a page mounted")
            window.contentView = nil
            window.close()
            RunLoop.main.run(until: Date().addingTimeInterval(0.2))
            XCTAssertEqual(PageMounts.shown, [], "page \(page) stayed mounted")
        }
    }
}
