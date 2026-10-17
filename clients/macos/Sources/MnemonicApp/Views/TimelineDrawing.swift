import Foundation
import MnemonicShared

/// Paint and picking share a span but never substitute their bounds for evidence.
struct TimelineDrawing {
    enum AgentMark: String { case claude, codex, unknown }
    enum Scale: String, CaseIterable { case focus = "Focus", day = "Full day" }

    struct Tick: Identifiable {
        let at: Double
        let position: Double
        let label: String
        let major: Bool
        var id: Double { at }
    }

    struct Block: Identifiable {
        let segment: TimelineSegment
        let position: Double
        let length: Double
        let track: Int
        let geometry: Geometry
        let caption: Caption?
        var id: String { segment.id }
        var point: Bool { segment.from == segment.to }
        var mark: AgentMark { TimelineDrawing.mark(segment.agent) }
        var label: String { TimelineDrawing.agent(segment.agent) }
    }

    struct Lane: Identifiable {
        let project: String?
        let name: String
        let agents: String
        let count: Int
        let blocks: [Block]
        var id: String { project.map { key in "project:" + key } ?? "unassigned" }
        var tracks: Int { max(1, (blocks.map(\.track).max() ?? 0) + 1) }
        var emptyMessage: String? {
            count == 0 ? "No sessions that day" : blocks.isEmpty ? "No sessions in this focus" : nil
        }
    }

    struct OwnerBlock: Identifiable {
        let range: TimelineOwnerRange
        let position: Double
        let length: Double
        let geometry: Geometry
        var id: String { range.id }
    }

    let from: Double
    let to: Double
    let width: Double
    let scale: Scale
    let ticks: [Tick]
    let lanes: [Lane]
    let owner: [OwnerBlock]
    let ownerAvailable: Bool
    var duration: Double { to - from }
    var valid: Bool { Span(from: from, to: to).valid && width > 0 }
    var height: Double { 32 + Self.ownerHeight + lanes.reduce(0) { total, lane in total + Self.laneHeight(lane) } }

    init(_ snapshot: ProjectTimeline, span: Span? = nil, width: Double = 580,
         registry: ProjectBoard? = nil, scale: Scale = .day, calendar: Calendar = .current) {
        let span = span ?? Span(from: snapshot.from, to: snapshot.to)
        from = span.from
        to = span.to
        self.width = width.isFinite ? max(0, width) : 0
        self.scale = scale
        ownerAvailable = snapshot.ownerAvailable
        guard span.valid, self.width > 0 else {
            ticks = []; lanes = []; owner = []
            return
        }
        let names = Dictionary(snapshot.projects.map { project in (project.key, project.name) },
                               uniquingKeysWith: { first, _ in first })
        let segments = Self.daySegments(snapshot)
        let keys = Set(segments.map(\.project)).union(Self.activeEmptyLaneKeys(snapshot, registry: registry)).sorted { left, right in
            let leftName = left.map { key in names[key] ?? key } ?? "Unassigned"
            let rightName = right.map { key in names[key] ?? key } ?? "Unassigned"
            return leftName == rightName ? (left ?? "") < (right ?? "") : leftName < rightName
        }
        lanes = keys.map { key in
            let values = segments.filter { segment in segment.project == key }
            return Lane(project: key, name: key.map { value in names[value] ?? value } ?? "Unassigned",
                        agents: Self.agents(values), count: values.count,
                        blocks: Self.blocks(values, span: span, width: width))
        }
        owner = snapshot.owner.compactMap { range in
            guard snapshot.ownerAvailable, range.to > range.from,
                  let geometry = Self.geometry(from: range.from, to: range.to, span: span, width: width,
                                               rowHeight: Self.ownerHeight, paintHeight: 12) else { return nil }
            let start = max(span.from, range.from), end = min(span.to, range.to)
            return OwnerBlock(range: range, position: (start - span.from) / span.duration,
                              length: (end - start) / span.duration, geometry: geometry)
        }
        ticks = Self.ticks(span: span, calendar: calendar)
    }

    private static func blocks(_ segments: [TimelineSegment], span: Span, width: Double) -> [Block] {
        let visible = segments.compactMap { segment -> (TimelineSegment, Geometry)? in
            geometry(from: segment.from, to: segment.to, span: span, width: width).map { value in (segment, value) }
        }.sorted { left, right in
            if left.1.paint.x != right.1.paint.x { return left.1.paint.x < right.1.paint.x }
            if left.0.from != right.0.from { return left.0.from < right.0.from }
            if left.0.to != right.0.to { return left.0.to < right.0.to }
            return left.0.id < right.0.id
        }
        var endings: [Double] = []
        return visible.map { segment, geometry in
            let track = endings.firstIndex { end in end <= geometry.paint.x } ?? endings.count
            if track == endings.count { endings.append(geometry.paint.end) }
            else { endings[track] = geometry.paint.end }
            let start = max(span.from, segment.from), end = min(span.to, segment.to)
            return Block(segment: segment, position: (start - span.from) / span.duration,
                         length: max(0, end - start) / span.duration, track: track, geometry: geometry,
                         caption: caption(segment, geometry: geometry, width: width))
        }
    }

    private static func ticks(span: Span, calendar: Calendar) -> [Tick] {
        var instants: [Double] = []
        let focus = span.duration <= 7_200
        var at = span.from
        while at < span.to, instants.count < 50 { instants.append(at); at += focus ? 1_800 : 3_600 }
        instants.append(span.to)
        let labels = instants.map { instant in
            FixedDateFormat.string(Date(timeIntervalSince1970: instant), format: "HH:mm", calendar: calendar)
        }
        return instants.enumerated().map { index, instant in
            var label = labels[index]
            // Repeated local hours carry their UTC offsets.
            if index > 0 && index < instants.count - 1 && labels.filter({ value in value == label }).count > 1 {
                let offset = calendar.timeZone.secondsFromGMT(for: Date(timeIntervalSince1970: instant))
                label += String(format: " UTC%+03d:%02d", offset / 3_600, abs(offset / 60) % 60)
            }
            return Tick(at: instant, position: (instant - span.from) / span.duration, label: label,
                        major: focus || index % 6 == 0 || index == instants.count - 1)
        }
    }

    static func daySegments(_ snapshot: ProjectTimeline) -> [TimelineSegment] {
        snapshot.segments.filter { segment in
            geometry(from: segment.from, to: segment.to, span: Span(from: snapshot.from, to: snapshot.to), width: 1) != nil
        }
    }

    static func segmentCount(_ count: Int) -> String { "\(count) recorded \(count == 1 ? "segment" : "segments")" }

    static func agents(_ segments: [TimelineSegment]) -> String {
        Set(segments.map(\.agent)).sorted { left, right in
            func rank(_ value: String) -> Int { mark(value) == .claude ? 0 : mark(value) == .codex ? 1 : 2 }
            return rank(left) == rank(right) ? left < right : rank(left) < rank(right)
        }.map(agent).joined(separator: " · ")
    }

    static func mark(_ agent: String) -> AgentMark {
        switch agent.lowercased() {
        case "claude": return .claude
        case "codex": return .codex
        default: return .unknown
        }
    }

    static func agent(_ agent: String) -> String {
        switch mark(agent) {
        case .claude: return "Claude Code"
        case .codex: return "Codex"
        case .unknown: return agent.isEmpty ? "Unknown agent" : "Unknown agent: \(agent)"
        }
    }

    static func time(_ instant: Double, calendar: Calendar) -> String {
        guard instant.isFinite else { return "Unknown time" }
        let format = instant.rounded(.towardZero) == instant ? "HH:mm:ss z" : "HH:mm:ss.SSS z"
        return FixedDateFormat.string(Date(timeIntervalSince1970: instant), format: format, calendar: calendar)
    }
}

enum TimelineSelection: Identifiable, Equatable {
    case agent(day: String, TimelineSegment)
    case owner(day: String, TimelineOwnerRange)

    var id: String {
        switch self {
        case .agent(let day, let segment): return day + ":agent:" + segment.id
        case .owner(let day, let range): return day + ":owner:" + range.id
        }
    }
    var day: String {
        switch self { case .agent(let day, _), .owner(let day, _): return day }
    }
    var from: Double {
        switch self { case .agent(_, let segment): return segment.from; case .owner(_, let range): return range.from }
    }
    var to: Double {
        switch self { case .agent(_, let segment): return segment.to; case .owner(_, let range): return range.to }
    }
    var stream: String? {
        if case .agent(_, let segment) = self { return segment.stream }
        return nil
    }
    var project: String? {
        switch self {
        case .agent(_, let segment): return segment.project
        case .owner(_, let range): return range.assignments.count == 1 ? range.assignments.first?.project : nil
        }
    }
    var exactRange: Bool {
        from.isFinite && to.isFinite && to > from && from.rounded(.towardZero) == from && to.rounded(.towardZero) == to
    }
    var point: Bool { from == to }

    func current(day: String, snapshot: ProjectTimeline?) -> Bool {
        guard day == self.day, snapshot?.day == day else { return false }
        switch self {
        case .agent(_, let segment): return snapshot?.segments.contains(segment) == true
        case .owner(_, let range): return snapshot?.owner.contains(range) == true
        }
    }

    func correction(project: String?, transcript: Bool, day: String,
                    snapshot: ProjectTimeline?) -> AttributionCorrection? {
        guard current(day: day, snapshot: snapshot) else { return nil }
        if transcript {
            guard let stream else { return nil }
            let correction = AttributionCorrection(stream: stream, project: project)
            return correction.isValid ? correction : nil
        }
        guard exactRange else { return nil }
        let correction = AttributionCorrection(stream: stream, from: Date(timeIntervalSince1970: from),
                                              to: Date(timeIntervalSince1970: to), project: project)
        return correction.isValid ? correction : nil
    }
}
