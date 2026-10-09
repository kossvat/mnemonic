import AppKit
import SwiftUI

/// Opens the dashboard scene without treating a Settings window as its replacement.
@MainActor
final class DashboardActivation {
    static let sceneID = "mnemonic-dashboard"
    static let windowIdentifier = NSUserInterfaceItemIdentifier(sceneID)

    struct Window {
        let identifier: String?
        let isMiniaturized: Bool
        let deminiaturize: @MainActor () -> Void
        let orderFront: @MainActor () -> Void
    }

    private let windows: @MainActor () -> [Window]
    private let activateApplication: @MainActor () -> Void
    private var openDashboard: (@MainActor () -> Void)?
    private var opening = false

    init(windows: @escaping @MainActor () -> [Window],
         activateApplication: @escaping @MainActor () -> Void) {
        self.windows = windows
        self.activateApplication = activateApplication
    }

    convenience init() {
        self.init(windows: {
            NSApp.windows.map { window in
                Window(identifier: window.identifier?.rawValue,
                       isMiniaturized: window.isMiniaturized,
                       deminiaturize: { window.deminiaturize(nil) },
                       orderFront: { window.makeKeyAndOrderFront(nil) })
            }
        }, activateApplication: { NSApp.activate(ignoringOtherApps: true) })
    }

    func registerOpener(_ action: @escaping @MainActor () -> Void) {
        openDashboard = action
    }

    /// Called after the native window receives its dashboard identifier.
    func windowDidAttach() { opening = false }

    func activate() {
        if let window = windows().first(where: { candidate in
            candidate.identifier == Self.windowIdentifier.rawValue
        }) {
            opening = false
            if window.isMiniaturized { window.deminiaturize() }
            window.orderFront()
        } else if !opening, let openDashboard {
            opening = true
            openDashboard()
        }
        activateApplication()
    }
}

private struct DashboardWindowAttachedKey: EnvironmentKey {
    static let defaultValue: @MainActor () -> Void = {}
}

extension EnvironmentValues {
    var mnemonicDashboardWindowAttached: @MainActor () -> Void {
        get { self[DashboardWindowAttachedKey.self] }
        set { self[DashboardWindowAttachedKey.self] = newValue }
    }
}
