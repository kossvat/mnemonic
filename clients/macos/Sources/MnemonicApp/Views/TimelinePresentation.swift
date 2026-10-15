import Foundation
import MnemonicShared

extension TimelineDrawing {
    struct PresentedLane: Identifiable {
        let lane: Lane
        let rows: [[Block]]
        let hiddenCount: Int
        let toggleLabel: String?
        var id: String { lane.id }
        var height: Double { max(76, Double(rows.count) * TimelineDrawing.rowHeight + 14) + (toggleLabel == nil ? 0 : 24) }
    }

    struct RecordedIntervals {
        let selections: [TimelineSelection]
        var initiallyExpanded: Bool { false }
        var title: String { "Recorded intervals (\(selections.count))" }
    }

    func presentedLanes(expanded: Set<String> = []) -> [PresentedLane] {
        lanes.map { lane in
            let foldable = scale == .focus && lane.tracks > 3
            let open = expanded.contains(lane.id)
            let tracks = foldable && !open ? 3 : lane.tracks
            let rows = (0..<tracks).map { track in lane.blocks.filter { block in block.track == track } }
            let hidden = lane.blocks.count - rows.reduce(0) { count, row in count + row.count }
            let toggle = foldable ? (open ? "Show fewer segments"
                : "+\(hidden) more \(hidden == 1 ? "segment" : "segments")") : nil
            return PresentedLane(lane: lane, rows: rows, hiddenCount: hidden, toggleLabel: toggle)
        }
    }

    static func presentedHeight(_ lanes: [PresentedLane]) -> Double {
        32 + ownerHeight + lanes.reduce(0) { total, lane in total + lane.height }
    }

    static func recordedIntervals(_ snapshot: ProjectTimeline) -> RecordedIntervals {
        RecordedIntervals(selections: snapshot.segments.map { segment in .agent(day: snapshot.day, segment) }
            + snapshot.owner.map { range in .owner(day: snapshot.day, range) })
    }

    static func displayScale(_ requested: Scale, focus: Span, snapshot: ProjectTimeline) -> Scale {
        requested == .focus && focus != Span(from: snapshot.from, to: snapshot.to) ? .focus : .day
    }

    static func overviewNote(_ scale: Scale) -> String {
        scale == .focus ? "Day view · 2-hour focus" : "Day view · Full day"
    }

    static func activeEmptyLaneKeys(_ snapshot: ProjectTimeline, registry: ProjectBoard?) -> Set<String?> {
        let active = Set((registry?.projects ?? []).filter { project in
            project.state == .active && project.mergedInto == nil
        }.map(\.key))
        return Set(snapshot.projects.compactMap { project in active.contains(project.key) ? project.key : nil }
            .map { key in Optional(key) })
    }
}
