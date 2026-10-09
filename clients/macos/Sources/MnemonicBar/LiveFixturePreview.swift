import AppKit
import MnemonicShared
import SwiftUI

/// Explicit offline rendering. It never constructs a production API client.
enum LiveFixturePreview {
    enum Failure: Error { case arguments, profile, fixture, image }

    @MainActor
    static func run(arguments: [String]) throws {
        guard let index = arguments.firstIndex(of: "--render-live-fixture"),
              arguments.count > index + 2 else { throw Failure.arguments }
        guard MnemonicProfile.current.isolated else { throw Failure.profile }
        let fixture = URL(fileURLWithPath: arguments[index + 1])
        let directory = URL(fileURLWithPath: arguments[index + 2])
        let bytes = try Data(contentsOf: fixture)
        let snapshot = try JSONDecoder().decode(LiveSnapshot.self, from: bytes)
        guard var empty = try JSONSerialization.jsonObject(with: bytes) as? [String: Any]
        else { throw Failure.fixture }
        empty["projects"] = []
        empty["more_projects"] = 0
        empty["unknown"] = ["sessions": [], "more_sessions": 0] as [String: Any]
        empty["today"] = ["owner_s": 0, "agent_s": 0]
        let quiet = try JSONDecoder().decode(
            LiveSnapshot.self, from: JSONSerialization.data(withJSONObject: empty))
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let variants: [(String, LiveAnswer)] = [
            ("live", .snapshot(snapshot)), ("empty", .snapshot(quiet)),
            ("api-off", .failed(.off)), ("unreachable", .failed(.unreachable)),
            ("unauthorised", .failed(.unauthorised)), ("earlier-daemon", .failed(.earlierDaemon)),
            ("undecodable", .failed(.undecodable)),
        ]
        let instant = Date(timeIntervalSince1970: 1_790_766_000)
        for (name, answer) in variants {
            for (suffix, scheme) in [("dark", ColorScheme.dark), ("light", .light)] {
                let live = WidgetLiveController(clock: SystemLiveClock()) { .failed(.off) }
                live.present(.snapshot(snapshot), at: instant)
                live.present(answer, at: instant)
                let service = MnemonicService(environment: WidgetEnvironment(
                    fetch: { _, _ in nil }, every: { _, _ in {} },
                    background: { _ in }, main: { action in action() }))
                let view = PagedContainerView(service: service, live: live, previewPage: 0)
                    .environment(\.colorScheme, scheme)
                try write(view, scheme: scheme,
                          to: directory.appendingPathComponent("widget-\(name)-\(suffix).png"))
            }
        }
    }

    @MainActor
    private static func write<V: View>(_ view: V, scheme: ColorScheme, to url: URL) throws {
        let host = NSHostingView(rootView: view)
        host.appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
        host.frame = NSRect(x: 0, y: 0, width: 336, height: 680)
        host.layoutSubtreeIfNeeded()
        guard let bitmap = host.bitmapImageRepForCachingDisplay(in: host.bounds)
        else { throw Failure.image }
        host.cacheDisplay(in: host.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:])
        else { throw Failure.image }
        try png.write(to: url)
    }
}
