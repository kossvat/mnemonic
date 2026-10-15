import AppKit
import MnemonicShared
import SwiftUI

/// Explicit rendering of supplied synthetic data; no production model or
/// transport is created, and no preferences or current API are requested.
enum AppFixturePreview {
    enum Failure: Error { case arguments, profile, fixture, image }

    @MainActor
    static func run(arguments: [String]) throws {
        guard let index = arguments.firstIndex(of: "--render-project-fixtures"),
              arguments.count > index + 2 else { throw Failure.arguments }
        guard MnemonicProfile.current.isolated else { throw Failure.profile }
        _ = NSApplication.shared
        let fixtures = URL(fileURLWithPath: arguments[index + 1])
        let output = URL(fileURLWithPath: arguments[index + 2])
        let board: ProjectBoard = try read("project-board.json", at: fixtures)
        let detail: ProjectDetail = try read("project-detail.json", at: fixtures)
        let live: LiveSnapshot = try read("live.json", at: fixtures)
        let emptyBoard = try JSONDecoder().decode(ProjectBoard.self, from: Data("{\"projects\":[]}".utf8))
        let now = Date(timeIntervalSince1970: 1_790_766_000)
        let listBoard = try projectsList(from: board, now: now)
        let emptyLive = try changing(live) { snapshot in
            snapshot["projects"] = []
            snapshot["more_projects"] = 0
            snapshot["unknown"] = ["sessions": [], "more_sessions": 0] as [String: Any]
            snapshot["today"] = ["owner_s": 0, "agent_s": 0]
        }
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        for (suffix, scheme) in [("light", ColorScheme.light), ("dark", .dark)] {
            let boardView = ProjectsView(board: board, live: live, failure: nil, loading: false, now: now, onOpen: { _ in })
            try write(boardView, route: .projects, scheme: scheme, height: 900,
                      to: output.appendingPathComponent("projects-\(suffix).png"))
            let list = ProjectsView(board: listBoard, live: live, failure: nil, loading: false, now: now, onOpen: { _ in })
            try write(list, route: .projects, scheme: scheme, height: 1_100,
                      to: output.appendingPathComponent("projects-list-\(suffix).png"))
            let project = ProjectPage(key: detail.key, detail: detail, failure: nil, loading: false, now: now, onBack: {})
            try write(project, route: .project(detail.key), scheme: scheme, height: 1_700,
                      to: output.appendingPathComponent("project-\(suffix).png"))
            let empty = ProjectsView(board: emptyBoard, live: emptyLive, failure: nil, loading: false, now: now, onOpen: { _ in })
            try write(empty, route: .projects, scheme: scheme, height: 900,
                      to: output.appendingPathComponent("projects-empty-\(suffix).png"))
            let missing = ProjectPage(key: "demoapp", detail: nil, failure: .unreachable, loading: false, now: now, onBack: {})
            try write(missing, route: .project("demoapp"), scheme: scheme, height: 1_500,
                      daemonFailure: .unreachable,
                      to: output.appendingPathComponent("project-unavailable-\(suffix).png"))
        }
    }

    private static func read<Value: Decodable>(_ name: String, at directory: URL) throws -> Value {
        try JSONDecoder().decode(Value.self, from: Data(contentsOf: directory.appendingPathComponent(name)))
    }

    /// Preserve the supplied fixture's decoded shape while deriving only
    /// synthetic rows. The fixed preview date keeps their groups stable.
    private static func projectsList(from board: ProjectBoard, now: Date) throws -> ProjectBoard {
        try changing(board) { object in
            guard var rows = object["projects"] as? [[String: Any]], let template = rows.first else { return }
            let examples: [(String, String, Int, Double?, Int, String?)] = [
                ("sample-docs", "Sample docs", 1, 1_800, 900, "Review the guide"),
                ("sample-library", "Sample library", 6, 4_500, 7_200, "Library cleanup"),
                ("sample-tools", "Sample tools", 18, nil, 0, nil),
                ("sample-archive", "Sample archive", 46, 0, 0, nil),
                ("sample-prototype", "Sample prototype", 75, 0, 0, nil),
            ]
            for (key, name, days, owner, agent, unit) in examples {
                var row = template
                row["key"] = key
                row["name"] = name
                row["state"] = "active"
                row["merged_into"] = NSNull()
                row["last_active_at"] = Int(now.timeIntervalSince1970) - days * 86_400
                row["week"] = ["owner_s": owner.map { seconds in seconds as Any } ?? NSNull(),
                               "agent_s": agent, "owner_estimate": owner != nil]
                if let unit {
                    row["plan"] = ["unit": unit, "done": 3, "total": 8, "stage": "planned",
                                   "task": "Check the next step", "gate": "none"]
                } else {
                    row["plan"] = NSNull()
                }
                row["plan_message"] = NSNull()
                rows.append(row)
            }
            object["projects"] = rows
        }
    }

    private static func changing<Value: Codable>(
        _ value: Value, _ update: (inout [String: Any]) -> Void
    ) throws -> Value {
        let data = try JSONEncoder().encode(value)
        guard var object = try JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            throw Failure.fixture
        }
        update(&object)
        return try JSONDecoder().decode(Value.self, from: JSONSerialization.data(withJSONObject: object))
    }

    @MainActor
    static func write<V: View>(
        _ view: V, route: AppRoute, scheme: ColorScheme, height: CGFloat,
        width: CGFloat = 1_180, scale: CGFloat? = nil,
        daemonFailure: LocalAPIFailure? = nil, to url: URL
    ) throws {
        let frame = HStack(spacing: 0) {
            Sidebar(selection: .constant(route), daemonFailure: daemonFailure).frame(width: 208)
            view.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .environment(\.colorScheme, scheme)
        .background(AppPalette.window)
        let host = NSHostingView(rootView: frame)
        host.appearance = NSAppearance(named: scheme == .dark ? .darkAqua : .aqua)
        host.frame = NSRect(x: 0, y: 0, width: width, height: height)
        host.layoutSubtreeIfNeeded()
        let representation: NSBitmapImageRep?
        if let scale {
            representation = NSBitmapImageRep(bitmapDataPlanes: nil, pixelsWide: Int(width * scale),
                pixelsHigh: Int(height * scale), bitsPerSample: 8, samplesPerPixel: 4, hasAlpha: true,
                isPlanar: false, colorSpaceName: .deviceRGB, bytesPerRow: 0, bitsPerPixel: 0)
            representation?.size = host.bounds.size
        } else {
            representation = host.bitmapImageRepForCachingDisplay(in: host.bounds)
        }
        guard let bitmap = representation else { throw Failure.image }
        host.cacheDisplay(in: host.bounds, to: bitmap)
        guard let png = bitmap.representation(using: .png, properties: [:]) else { throw Failure.image }
        try png.write(to: url)
    }
}
