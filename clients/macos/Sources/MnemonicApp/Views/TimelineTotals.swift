import Foundation
import MnemonicShared

extension TimelineDrawing {
    struct ProjectTime: Identifiable {
        let key: String?
        let name: String
        let owner: Double?
        let agents: Double
        var id: String { key.map { value in "project:" + value } ?? "unassigned" }
    }

    struct Totals {
        let owner: Double?
        let agents: Double
        let projects: [ProjectTime]
    }

    static func union(_ intervals: [Span], within span: Span) -> Double {
        guard span.valid else { return 0 }
        let cut: [Span] = intervals.filter(\.valid).map { (range: Span) -> Span in
            Span(from: max(span.from, range.from), to: min(span.to, range.to))
        }
        let values: [Span] = cut.filter(\.valid).sorted { (left: Span, right: Span) -> Bool in
            left.from == right.from ? left.to < right.to : left.from < right.from
        }
        var total = 0.0, end = span.from
        for range in values {
            total += max(0, range.to - max(end, range.from))
            end = max(end, range.to)
        }
        return total
    }

    static func totals(_ snapshot: ProjectTimeline) -> Totals {
        let day = Span(from: snapshot.from, to: snapshot.to)
        let segments = daySegments(snapshot)
        let owners = snapshot.ownerAvailable ? snapshot.owner : []
        let owner = snapshot.ownerAvailable ? union(owners.map { range in Span(from: range.from, to: range.to) }, within: day) : nil
        let agents = union(segments.map { segment in Span(from: segment.from, to: segment.to) }, within: day)
        let registered: [String?] = snapshot.projects.map { project in Optional(project.key) }
        let worked: [String?] = segments.map(\.project)
        let assigned: [String?] = owners.flatMap { range in range.assignments.map(\.project) }
        let keys = Set(registered + worked + assigned)
        var projects: [ProjectTime] = []
        for key in keys {
            var own = 0.0
            for range in owners {
                let length = union([Span(from: range.from, to: range.to)], within: day)
                var share = 0.0
                for assignment in range.assignments where assignment.project == key {
                    share += assignment.share.isFinite ? max(0, min(1, assignment.share)) : 0
                }
                own += length * share
            }
            let spans: [Span] = segments.filter { segment in segment.project == key }
                .map { segment in Span(from: segment.from, to: segment.to) }
            projects.append(ProjectTime(key: key, name: projectName(key, snapshot: snapshot),
                                        owner: snapshot.ownerAvailable ? own : nil,
                                        agents: union(spans, within: day)))
        }
        projects.sort { (left: ProjectTime, right: ProjectTime) -> Bool in
            left.name == right.name ? left.id < right.id : left.name < right.name
        }
        return Totals(owner: owner, agents: agents, projects: projects)
    }

    static func projectName(_ key: String?, snapshot: ProjectTimeline) -> String {
        key.flatMap { key in snapshot.projects.first { project in project.key == key }?.name ?? key } ?? "Unassigned"
    }

    static func ownerTime(_ seconds: Double?) -> String {
        seconds.map { value in ProjectPresentation.duration(value) } ?? "Unavailable"
    }

    static func liveCount(_ live: LiveSnapshot?) -> String? {
        guard let live else { return nil }
        let count = live.projects.reduce(0) { total, project in total + project.sessions.count + project.moreSessions }
            + live.unknown.sessions.count + live.unknown.moreSessions
        let label = "\(count) \(count == 1 ? "session" : "sessions") live"
        return live.moreProjects > 0 ? "At least " + label + " · snapshot incomplete" : label
    }

    static func emptyMessage(_ snapshot: ProjectTimeline) -> String? {
        guard daySegments(snapshot).isEmpty else { return nil }
        return snapshot.ownerAvailable && snapshot.owner.isEmpty
            ? "No activity recorded for this day." : "No agent sessions recorded for this day."
    }

    static func ownerNotice(_ drawing: TimelineDrawing, snapshot: ProjectTimeline) -> String? {
        if !drawing.ownerAvailable { return "Owner activity unavailable" }
        guard drawing.owner.isEmpty else { return nil }
        return snapshot.owner.isEmpty ? "No owner activity recorded" : "No owner activity in this span"
    }
}
