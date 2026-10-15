import Foundation
import MnemonicShared

enum FixtureTimelinePresentation {
    static func scale(_ state: FixtureState) -> TimelineDrawing.Scale {
        if ["timeline-day", "timeline-short", "timeline-23h", "timeline-25h"].contains(state.id) { return .day }
        guard let snapshot = state.timeline, let focus = focus(state) else { return .focus }
        return TimelineDrawing.displayScale(.focus, focus: focus, snapshot: snapshot)
    }

    static func width(_ state: FixtureState) -> Double { state.id == "timeline-short" ? 1_440 : 580 }

    static func focus(_ state: FixtureState) -> TimelineDrawing.Span? {
        guard let snapshot = state.timeline else { return nil }
        return initialFocus(state) ?? TimelineDrawing.defaultFocus(snapshot, now: state.now, calendar: state.calendar)
    }

    static func initialFocus(_ state: FixtureState) -> TimelineDrawing.Span? {
        guard let snapshot = state.timeline else { return nil }
        let day = TimelineDrawing.Span(from: snapshot.from, to: snapshot.to)
        // These states show a strip click without changing any of the served input.
        if state.id == "timeline-edge", let first = snapshot.segments.first {
            return TimelineDrawing.focus(endingAt: first.from + (first.to - first.from) / 2 + 7_200, day: day)
        }
        if state.id == "timeline-crowded", let first = snapshot.segments.map(\.from).min() {
            return TimelineDrawing.focus(endingAt: first + 7_200, day: day)
        }
        return nil
    }

    static func summary(_ state: FixtureState) -> String {
        if let failure = state.failure {
            let notice = ProjectPresentation.failure(failure)
            return "failure=\(notice.title): \(notice.detail) span=not-shown totals=not-shown"
        }
        guard let snapshot = state.timeline, snapshot.day == state.day, let focus = focus(state) else {
            return state.loading ? "loading=Loading timeline span=not-shown totals=not-shown" : "not-loaded"
        }
        let scale = scale(state)
        let drawing = TimelineDrawing(snapshot, span: scale == .focus ? focus : nil,
                                      width: width(state), registry: state.board, scale: scale, calendar: state.calendar)
        func minutes(_ instant: Double) -> String { decimal((instant - snapshot.from) / 60) }
        let lanes = drawing.presentedLanes().map { presented in
            let lane = presented.lane, blocks = presented.rows.flatMap { row in row }
            let labels = blocks.compactMap { block in block.caption.map { caption in "\(block.id):\(caption.text)" } }
            return "\(lane.project ?? "unassigned"):\(lane.name) rows=\(presented.rows.count) agents=\(lane.agents) "
                + "count=\(TimelineDrawing.segmentCount(lane.count)) marks=\(blocks.count) labels=\(list(labels))"
                + (lane.emptyMessage.map { text in " message=\(text)" } ?? "")
                + (presented.toggleLabel.map { text in " folded=\(text) hidden=\(presented.hiddenCount)" } ?? "")
        }
        let owner = TimelineDrawing.ownerNotice(drawing, snapshot: snapshot) ?? "\(drawing.owner.count) marks"
        let totals = TimelineDrawing.totals(snapshot)
        let projects = totals.projects.map { project in
            "\(project.key ?? "unassigned")=You \(TimelineDrawing.ownerTime(project.owner)), Agents \(ProjectPresentation.duration(project.agents))"
        }
        let records = snapshot.segments.map { segment in
            "\(segment.id)@\(minutes(segment.from))..\(minutes(segment.to))"
        } + snapshot.owner.map { range in
            let shares = range.assignments.map { share in "\(share.project ?? "unassigned")=\(decimal(share.share))" }
            return "\(range.id)@\(minutes(range.from))..\(minutes(range.to))\(list(shares))"
        }
        let intervals = TimelineDrawing.recordedIntervals(snapshot)
        let folded = intervals.selections.isEmpty ? "" : "intervals=\(intervals.title) \(intervals.initiallyExpanded ? "open" : "folded") "
        return "scale=\(scale.rawValue) day=\(decimal((snapshot.to - snapshot.from) / 60))m "
            + "span=\(minutes(drawing.from))..\(minutes(drawing.to))m width=\(decimal(drawing.width)) "
            + "lanes=\(list(lanes)) owner-first=\(owner) "
            + "totals=[You \(TimelineDrawing.ownerTime(totals.owner)) | Agents \(ProjectPresentation.duration(totals.agents))] "
            + "projects=\(list(projects)) live=\(TimelineDrawing.liveCount(state.live) ?? "not-served") "
            + "empty=\(TimelineDrawing.emptyMessage(snapshot) ?? "none") " + folded + "records=\(list(records))"
    }

    private static func list(_ values: [String]) -> String { "[" + values.joined(separator: " | ") + "]" }
    private static func decimal(_ value: Double) -> String {
        String(format: "%.2f", locale: Locale(identifier: "en_US_POSIX"), value)
    }
}
