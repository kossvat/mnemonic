import Foundation
import MnemonicShared

/// Positions represent elapsed seconds in the complete local day, including
/// offset changes. Drawing never turns a point or a share into elapsed time.
struct TimelineDrawing {
    enum AgentMark: String { case claude, codex, unknown }

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
        var id: String { segment.id }
        var point: Bool { segment.from == segment.to }
        var mark: AgentMark { TimelineDrawing.mark(segment.agent) }
        var label: String { TimelineDrawing.agent(segment.agent) }
    }

    struct Lane: Identifiable {
        let project: String?
        let name: String
        let blocks: [Block]
        var id: String { project.map { key in "project:" + key } ?? "unassigned" }
        var tracks: Int { max(1, (blocks.map(\.track).max() ?? 0) + 1) }
    }

    struct OwnerBlock: Identifiable {
        let range: TimelineOwnerRange
        let position: Double
        let length: Double
        var id: String { range.id }
    }

    let from: Double
    let to: Double
    let ticks: [Tick]
    let lanes: [Lane]
    let owner: [OwnerBlock]
    let ownerAvailable: Bool
    var duration: Double { to - from }
    var valid: Bool { from.isFinite && to.isFinite && to > from }

    init(_ snapshot: ProjectTimeline, calendar: Calendar = .current) {
        from = snapshot.from
        to = snapshot.to
        ownerAvailable = snapshot.ownerAvailable
        guard snapshot.from.isFinite, snapshot.to.isFinite, snapshot.to > snapshot.from else {
            ticks = []; lanes = []; owner = []
            return
        }
        let span = snapshot.to - snapshot.from
        let names = Dictionary(snapshot.projects.map { project in (project.key, project.name) },
                               uniquingKeysWith: { first, _ in first })
        let segments = snapshot.segments.filter { segment in
            guard segment.from.isFinite, segment.to.isFinite, segment.to >= segment.from else { return false }
            if segment.from == segment.to { return segment.from >= snapshot.from && segment.from <= snapshot.to }
            return segment.to > snapshot.from && segment.from < snapshot.to
        }
        let keys = Set(segments.map(\.project)).sorted { left, right in
            let leftName = left.map { key in names[key] ?? key } ?? "Unassigned"
            let rightName = right.map { key in names[key] ?? key } ?? "Unassigned"
            return leftName == rightName ? (left ?? "") < (right ?? "") : leftName < rightName
        }
        lanes = keys.map { key in
            let ordered = segments.filter { segment in segment.project == key }.sorted { left, right in
                if left.from != right.from { return left.from < right.from }
                if left.to != right.to { return left.to < right.to }
                return left.id < right.id
            }
            var endings: [(Double, Bool)] = []
            let blocks = ordered.map { segment in
                let point = segment.from == segment.to
                let track = endings.firstIndex { end, lastPoint in
                    end < segment.from || (end == segment.from && !point && !lastPoint)
                } ?? endings.count
                if track == endings.count { endings.append((segment.to, point)) }
                else { endings[track] = (segment.to, point) }
                let start = max(snapshot.from, segment.from), end = min(snapshot.to, segment.to)
                return Block(segment: segment, position: (start - snapshot.from) / span,
                             length: max(0, end - start) / span, track: track)
            }
            return Lane(project: key, name: key.map { value in names[value] ?? value } ?? "Unassigned",
                        blocks: blocks)
        }
        owner = snapshot.owner.compactMap { range in
            guard range.from.isFinite, range.to.isFinite, range.to > range.from,
                  range.to > snapshot.from, range.from < snapshot.to else { return nil }
            let start = max(snapshot.from, range.from), end = min(snapshot.to, range.to)
            return OwnerBlock(range: range, position: (start - snapshot.from) / span,
                              length: (end - start) / span)
        }
        let formatter = DateFormatter()
        formatter.calendar = calendar
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = calendar.timeZone
        formatter.dateFormat = "HH:mm"
        var instants: [Double] = []
        var at = snapshot.from
        while at < snapshot.to, instants.count < 50 { instants.append(at); at += 3_600 }
        instants.append(snapshot.to)
        let labels = instants.map { instant in formatter.string(from: Date(timeIntervalSince1970: instant)) }
        ticks = instants.enumerated().map { index, instant in
            var label = labels[index]
            // The repeated fall-back hour must be distinguishable from itself.
            if index > 0 && index < instants.count - 1 && labels.filter({ value in value == label }).count > 1 {
                let offset = calendar.timeZone.secondsFromGMT(for: Date(timeIntervalSince1970: instant))
                label += String(format: " UTC%+03d:%02d", offset / 3_600, abs(offset / 60) % 60)
            }
            return Tick(at: instant, position: (instant - snapshot.from) / span, label: label,
                        major: index % 3 == 0 || index == instants.count - 1)
        }
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
        case .claude: return "Claude"
        case .codex: return "Codex"
        case .unknown: return agent.isEmpty ? "Unknown agent" : "Unknown agent: \(agent)"
        }
    }

    static func time(_ instant: Double, calendar: Calendar) -> String {
        guard instant.isFinite else { return "Unknown time" }
        let formatter = DateFormatter()
        formatter.calendar = calendar
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = calendar.timeZone
        formatter.dateFormat = instant.rounded(.towardZero) == instant ? "HH:mm:ss z" : "HH:mm:ss.SSS z"
        return formatter.string(from: Date(timeIntervalSince1970: instant))
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
