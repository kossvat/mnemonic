import AppKit
import Foundation
import MnemonicShared
import SwiftUI

enum FixtureStateRender {
    @MainActor
    static func run(arguments: [String]) throws {
        guard MnemonicProfile.current.isolated else { throw AppFixturePreview.Failure.profile }
        guard let index = arguments.firstIndex(of: "--render-fixture-states"), arguments.count > index + 1,
              !arguments[index + 1].hasPrefix("--") else { throw AppFixturePreview.Failure.arguments }
        var states = try FixtureStates.all()
        if let only = arguments.firstIndex(of: "--only") {
            guard arguments.count > only + 1,
                  let state = states.first(where: { state in state.id == arguments[only + 1] }) else {
                throw AppFixturePreview.Failure.arguments
            }
            states = [state]
        }
        try draw(states, to: URL(fileURLWithPath: arguments[index + 1], isDirectory: true))
    }

    @MainActor
    static func draw(_ states: [FixtureState], to output: URL) throws {
        guard Set(states.map(\.id)).count == states.count,
              states.allSatisfy({ state in
                  !state.id.isEmpty && !state.id.contains("/") && FixtureStateSummary.summary(of: state) == state.expected
              }) else {
            throw AppFixturePreview.Failure.fixture
        }
        let files = Set(states.flatMap { state in ["\(state.id)-light.png", "\(state.id)-dark.png"] })
        let manager = FileManager.default
        if manager.fileExists(atPath: output.path) {
            let existing = Set(try manager.contentsOfDirectory(atPath: output.path))
            guard existing.isSubset(of: files) else { throw AppFixturePreview.Failure.arguments }
        }
        try manager.createDirectory(at: output, withIntermediateDirectories: true)
        _ = NSApplication.shared
        for state in states {
            for (name, scheme) in [("light", ColorScheme.light), ("dark", .dark)] {
                try autoreleasepool {
                    let previousZone = NSTimeZone.default
                    NSTimeZone.default = state.timeZone
                    defer { NSTimeZone.default = previousZone }
                    let page = Self.page(state)
                        .environment(\.calendar, state.calendar)
                        .environment(\.timeZone, state.timeZone)
                        .environment(\.locale, Locale(identifier: "en_US_POSIX"))
                    try AppFixturePreview.write(page, route: route(state), scheme: scheme,
                        height: state.height ?? 800, width: 1_240, scale: 2,
                        daemonFailure: state.failure,
                        to: output.appendingPathComponent("\(state.id)-\(name).png"))
                }
            }
        }
    }

    private static func route(_ state: FixtureState) -> AppRoute {
        switch state.screen {
        case .frame, .projects: return .projects
        case .project: return .project(state.detail?.key ?? FixtureWire.key(0))
        case .timeline: return .timeline(state.day)
        }
    }

    @MainActor
    @ViewBuilder private static func page(_ state: FixtureState) -> some View {
        switch state.screen {
        case .frame, .projects:
            ProjectsView(board: state.board, live: state.live, failure: state.failure, loading: state.loading,
                         now: state.now, onOpen: { _ in })
        case .project:
            ProjectPage(key: state.detail?.key ?? FixtureWire.key(0), detail: state.detail,
                        failure: state.failure, loading: state.loading, now: state.now, onBack: {})
        case .timeline:
            TimelinePage(day: state.day, snapshot: state.timeline, loading: state.loading, failure: state.failure,
                         mutationInFlight: false, message: nil, canUndo: false, now: state.now, calendar: state.calendar,
                         live: state.live, registry: state.board, initialScale: FixtureTimelinePresentation.scale(state),
                         initialFocus: FixtureTimelinePresentation.initialFocus(state), laneWidth: FixtureTimelinePresentation.width(state),
                         onDay: { _ in }, onToday: {}, onRefresh: {}, onApply: { _, _ in false }, onUndo: { false })
        }
    }
}
