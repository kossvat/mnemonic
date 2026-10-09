import AppKit
import MnemonicShared
import SwiftUI

/// Only the explicitly supplied synthetic fixture is read for these renders.
enum TimelineFixturePreview {
    @MainActor
    static func run(arguments: [String]) throws {
        guard let index = arguments.firstIndex(of: "--render-timeline-fixtures"),
              arguments.count > index + 2 else { throw AppFixturePreview.Failure.arguments }
        guard MnemonicProfile.current.isolated else { throw AppFixturePreview.Failure.profile }
        _ = NSApplication.shared
        let directory = URL(fileURLWithPath: arguments[index + 1])
        let output = URL(fileURLWithPath: arguments[index + 2])
        let full = try JSONDecoder().decode(ProjectTimeline.self,
            from: Data(contentsOf: directory.appendingPathComponent("project-timeline.json")))
        let empty = ProjectTimeline(day: full.day, from: full.from, to: full.to,
                                    projects: full.projects, segments: [], owner: [], ownerAvailable: true)
        let activity = ProjectTimeline(day: full.day, from: full.from, to: full.to,
                                       projects: full.projects, segments: [], owner: full.owner, ownerAvailable: true)
        var calendar = Calendar(identifier: .gregorian)
        guard let utc = TimeZone(secondsFromGMT: 0) else { throw AppFixturePreview.Failure.fixture }
        calendar.timeZone = utc
        calendar.locale = Locale(identifier: "en_US_POSIX")
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        for (name, snapshot, height) in [("full", full, CGFloat(1_400)),
                                         ("empty", empty, 800), ("activity-only", activity, 1_100)] {
            for (suffix, scheme) in [("light", ColorScheme.light), ("dark", .dark)] {
                let page = TimelinePage(
                    day: snapshot.day, snapshot: snapshot, loading: false, failure: nil,
                    mutationInFlight: false, message: nil, canUndo: false,
                    now: Date(timeIntervalSince1970: snapshot.to), calendar: calendar,
                    onDay: { _ in }, onToday: {}, onRefresh: {}, onApply: { _, _ in false }, onUndo: { false })
                try AppFixturePreview.write(page, route: .timeline(snapshot.day), scheme: scheme, height: height,
                                            to: output.appendingPathComponent("timeline-\(name)-\(suffix).png"))
            }
        }
    }
}
