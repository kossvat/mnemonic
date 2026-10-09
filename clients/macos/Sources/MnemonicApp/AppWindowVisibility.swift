import AppKit
import SwiftUI

/// Real window visibility includes occlusion and application hiding, not focus.
struct AppWindowVisibility: NSViewRepresentable {
    @Environment(\.mnemonicDashboardWindowAttached) private var windowAttached
    let changed: @MainActor (ObjectIdentifier, Bool) -> Void

    func makeNSView(context: Context) -> Probe {
        Probe(changed: changed, windowAttached: windowAttached)
    }
    func updateNSView(_ view: Probe, context: Context) {
        view.changed = changed
        view.windowAttached = windowAttached
    }

    @MainActor
    final class Probe: NSView {
        var changed: @MainActor (ObjectIdentifier, Bool) -> Void
        var windowAttached: @MainActor () -> Void
        private weak var observedWindow: NSWindow?
        private var observedID: ObjectIdentifier?
        private var observers: [NSObjectProtocol] = []
        private var lastVisible: Bool?

        init(changed: @escaping @MainActor (ObjectIdentifier, Bool) -> Void,
             windowAttached: @escaping @MainActor () -> Void) {
            self.changed = changed
            self.windowAttached = windowAttached
            super.init(frame: .zero)
        }
        required init?(coder: NSCoder) { fatalError("init(coder:) is unavailable") }

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            stop()
            guard let window else { return }
            observedWindow = window
            observedID = ObjectIdentifier(window)
            window.identifier = DashboardActivation.windowIdentifier
            windowAttached()
            let center = NotificationCenter.default
            let names: [Notification.Name] = [
                NSWindow.didChangeOcclusionStateNotification,
                NSWindow.didMiniaturizeNotification,
                NSWindow.didDeminiaturizeNotification,
                NSWindow.didBecomeKeyNotification,
                NSWindow.didResignKeyNotification
            ]
            for name in names {
                observers.append(center.addObserver(forName: name, object: window, queue: .main) {
                    [weak self] _ in MainActor.assumeIsolated { self?.update() }
                })
            }
            observers.append(center.addObserver(forName: NSWindow.willCloseNotification, object: window, queue: .main) {
                [weak self] _ in MainActor.assumeIsolated {
                    self?.observedWindow?.identifier = nil
                    self?.report(false)
                }
            })
            for name in [NSApplication.didHideNotification, NSApplication.didUnhideNotification,
                         NSApplication.didBecomeActiveNotification] {
                observers.append(center.addObserver(forName: name, object: nil, queue: .main) {
                    [weak self] _ in MainActor.assumeIsolated { self?.update() }
                })
            }
            update()
            DispatchQueue.main.async { [weak self] in self?.update() }
        }

        private func update() {
            guard let window = observedWindow else { report(false); return }
            report(window.isVisible && !window.isMiniaturized && !NSApp.isHidden
                   && window.occlusionState.contains(.visible))
        }

        private func report(_ visible: Bool) {
            guard let id = observedID, lastVisible != visible else { return }
            lastVisible = visible
            changed(id, visible)
        }

        private func stop() {
            report(false)
            observers.forEach { observer in NotificationCenter.default.removeObserver(observer) }
            observers.removeAll()
            observedID = nil
            observedWindow = nil
            lastVisible = nil
        }

        deinit {
            for observer in observers { NotificationCenter.default.removeObserver(observer) }
        }
    }
}

/// Legacy canvases consult this until they move to the static canvas.
private struct WindowVisibleKey: EnvironmentKey { static let defaultValue = false }
extension EnvironmentValues {
    var mnemonicWindowVisible: Bool {
        get { self[WindowVisibleKey.self] }
        set { self[WindowVisibleKey.self] = newValue }
    }
}
