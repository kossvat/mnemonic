import Foundation
import MnemonicShared

/// Route objects are serialized and decoded even when every value is synthetic.
enum FixtureWire {
    typealias Object = [String: Any]
    static let now = Date(timeIntervalSince1970: 1_790_766_000)
    static let utc = TimeZone(secondsFromGMT: 0)!

    static func calendar(_ zone: TimeZone = utc) -> Calendar {
        var value = Calendar(identifier: .gregorian)
        value.locale = Locale(identifier: "en_US_POSIX")
        value.timeZone = zone
        return value
    }

    static func decode<Value: Decodable>(_ object: Object, as type: Value.Type) throws -> Value {
        try JSONDecoder().decode(type, from: JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]))
    }

    private static func nullable<Value>(_ value: Value?) -> Any { value.map { present in present as Any } ?? NSNull() }

    static func name(_ index: Int, long: Bool = false) -> String {
        let words = FixtureVocabulary.projects
        let name = words[index % words.count] + (index < words.count ? "" : " \(index / words.count + 1)")
        return long ? name + " " + Array(repeating: "Beacon Orchard Lantern Meadow", count: 4).joined(separator: " ") : name
    }

    static func key(_ index: Int) -> String { name(index).lowercased().replacingOccurrences(of: " ", with: "-") }

    static func planLine(unit: String = "Review the guide", done: Int = 1, total: Int = 2,
                         stage: String = "building", task: String? = "Check the local cache",
                         gate: String = "passed") -> Object {
        ["unit": unit, "done": done, "total": total, "stage": stage, "task": nullable(task), "gate": gate]
    }

    static func row(_ index: Int, long: Bool = false, ageDays: Int? = 0,
                    state: String = "active", mergedInto: String? = nil,
                    owner: Double? = 1_500, agent: Int = 3_600,
                    plan: Object? = planLine(), message: String? = nil) -> Object {
        ["key": key(index), "name": name(index, long: long), "state": state,
         "merged_into": nullable(mergedInto),
         "last_active_at": nullable(ageDays.map { days in Int(now.timeIntervalSince1970) - days * 86_400 }),
         "week": ["owner_s": nullable(owner), "agent_s": agent, "owner_estimate": owner != nil],
         "plan": nullable(plan), "plan_message": nullable(message)]
    }

    static func board(_ rows: [Object] = []) throws -> ProjectBoard {
        try decode(["projects": rows], as: ProjectBoard.self)
    }

    static func funnelRow(_ index: Int) -> Object {
        var object = row(index, plan: planLine(done: 0, total: 0))
        object["plan_provider"] = "funnel"
        object["plan_counts"] = ["contacted": 4, "waiting_on_owner": 2]
        object["plan_weekly_count"] = 3
        return object
    }

    static func funnelBoard() throws -> ProjectBoard {
        try board([row(0), funnelRow(1), row(2), funnelRow(3)])
    }

    static func enrichedBoard() throws -> ProjectBoard {
        let rows: [Object] = (0..<4).map { index in
            var object = index.isMultiple(of: 2) ? row(index) : funnelRow(index)
            if index.isMultiple(of: 2) {
                object["plan_provider"] = index == 0 ? "checkbox" : "registry"
                object["plan_counts"] = ["done": 1, "planned": 1]
                object["plan_weekly_count"] = 0
            }
            object["gate_at"] = Int(now.timeIntervalSince1970) - 60
            object["open_units"] = index == 2 ? 0 : 2
            object["waiting_on_owner"] = index == 0 ? ["Review the guide", "Verify the result"] : ["Verify the result"]
            object["last_closed"] = ["title": "Draft the guide", "closed_at": Int(now.timeIntervalSince1970) - 86_400]
            return object
        }
        return try decode(["projects": rows,
                           "week": ["owner_s": 4_200, "agent_s": 6_600, "owner_estimate": false]], as: ProjectBoard.self)
    }

    static func session(_ agent: String, state: String = "running", driver: String = "owner",
                        startedAt: Int = Int(now.timeIntervalSince1970) - 3_600) -> Object {
        ["agent": agent, "state": state, "driver": driver, "started_at": startedAt]
    }

    static func sessions(_ agents: [String]) -> [Object] { agents.map { agent in session(agent) } }

    static func liveProject(_ index: Int, long: Bool = false, agents: [String] = ["claude"],
                            state: String = "running", moreSessions: Int = 0,
                            plan: Object? = planLine()) -> Object {
        ["key": key(index), "name": name(index, long: long), "state": state,
         "sessions": agents.enumerated().map { offset, agent in
             session(agent, state: state, driver: offset == 0 ? "owner" : "program")
         }, "more_sessions": moreSessions,
         "today": ["owner_s": 600, "agent_s": 1_200, "confidence": "high"], "plan": nullable(plan)]
    }

    static func live(count: Int = 0, agents: [Int: [String]] = [:],
                     plans: [Int: Object] = [:], withoutPlan: Set<Int> = [],
                     long: Bool = false, moreProjects: Int = 0, moreSessions: [Int: Int] = [:],
                     unknown: [Object] = [], moreUnknown: Int = 0) throws -> LiveSnapshot {
        let projects = (0..<count).map { index in
            liveProject(index, long: long, agents: agents[index] ?? [index.isMultiple(of: 2) ? "claude" : "codex"],
                        moreSessions: moreSessions[index] ?? 0,
                        plan: withoutPlan.contains(index) ? nil : plans[index] ?? planLine())
        }
        return try decode([
            "boot": "0123456789abcdef", "v": 1, "projects": projects, "more_projects": moreProjects,
            "unknown": ["sessions": unknown, "more_sessions": moreUnknown],
            "today": ["owner_s": count * 600, "agent_s": count * 1_200], "memories": 40
        ], as: LiveSnapshot.self)
    }

    static func task(_ number: Int, title: String, status: String = "planned") -> Object {
        ["id": String(format: "T%02d", number), "title": title, "status": status, "owner": NSNull()]
    }

    static func unit(_ number: Int = 1, title: String = "Review the guide", done: Int = 1, total: Int = 2,
                     stage: String = "building", stale: Bool = false, current: Bool = true,
                     tasks: [Object]? = nil, gate: String = "passed", error: String? = nil) -> Object {
        ["id": "unit-\(number)", "title": title, "done": done, "total": total, "stage": stage, "stale": stale,
         "current_task": nullable(current ? ["id": "T02", "title": "Check the local cache", "source": "first_open"] : nil),
         "counts": ["done": done, "planned": max(0, total - done)], "waiting_on_owner": [], "weekly_count": 0,
         "gate": ["status": gate, "at": nullable(gate == "none" ? nil : Int(now.timeIntervalSince1970) - 60),
                  "failed_runs": gate == "failed" ? 1 : 0],
         "provider": "checkbox", "checkouts": 1, "as_of": Int(now.timeIntervalSince1970),
         "tasks": tasks ?? [task(1, title: "Draft the guide", status: "done"), task(2, title: "Check the local cache")],
         "error": nullable(error)]
    }

    static func days(count: Int = 14, owner: Double? = 600, agent: Int = 1_200) -> [Object] {
        (0..<count).map { index in
            let date = now.addingTimeInterval(Double(index - count + 1) * 86_400)
            return ["day": TimelineDay.string(date, calendar: calendar()), "owner_s": nullable(owner),
                    "agent_s": agent, "confidence": nullable(owner == nil ? nil : "high")]
        }
    }

    static func history(count: Int = 1, at: Int = Int(now.timeIntervalSince1970) - 60,
                        spacing: Int = 3_600) -> [Object] {
        (0..<count).map { index in
            ["id": "decision-\(index + 1)", "title": "Keep the local cache \(index + 1)", "at": at - index * spacing]
        }
    }

    static func term(_ name: String, count: Int = 1) -> Object { ["name": name, "memory_count": count] }

    static func detail(units: [Object] = [unit()], message: String? = nil,
                       agents: [String] = ["claude"], time: [Object] = days(),
                       history: [Object] = FixtureWire.history(), terms: [Object]? = [term("cache"), term("guide")],
                       sessionRows: [Object]? = nil, checkoutDetails: [String: Object]? = nil) throws -> ProjectDetail {
        let gates = Dictionary(units.compactMap { unit -> (String, Object)? in
            guard let id = unit["id"] as? String else { return nil }
            return (id, ["dirty": false, "commits_since": 0])
        }, uniquingKeysWith: { first, _ in first })
        var object: Object = [
            "key": key(0), "name": name(0), "state": "active", "plan": ["units": units, "message": nullable(message)],
            "sessions": sessionRows ?? sessions(agents), "gate_detail": checkoutDetails ?? gates,
            "time": time, "owner_estimate": true, "history": history
        ]
        if let terms { object["top_terms"] = terms }
        return try decode(object, as: ProjectDetail.self)
    }

    static func projectEnriched() throws -> ProjectDetail {
        let at = Int(now.timeIntervalSince1970)
        var open = unit(total: 4, tasks: [task(1, title: "Draft the guide", status: "done"),
            task(2, title: "Check the local cache"), task(3, title: "Verify the result"), task(4, title: "Review the guide")])
        open["provider"] = "registry"
        open["closed"] = false
        open["closed_at"] = NSNull()
        open["gate"] = ["status": "stale", "at": at - 86_400, "failed_runs": 0, "result": "passed"]
        open["checkouts"] = 2
        var closed = unit(2, title: "Draft the guide", done: 2, stage: "review", current: false,
            tasks: [task(1, title: "Draft the guide", status: "done"), task(2, title: "Verify the result", status: "done")])
        closed["closed"] = true
        closed["closed_at"] = at - 2 * 86_400
        closed["gate"] = ["status": "passed", "at": at - 2 * 86_400, "failed_runs": 0, "result": "passed"]
        var first = session("claude"), second = session("codex", state: "waiting", driver: "program")
        first["last_record_at"] = at - 60
        second["last_record_at"] = at - 600
        var entries = history(count: 2, spacing: 86_400)
        entries[0]["kind"] = "decision"
        entries[1]["kind"] = "correction"
        return try detail(units: [open, closed], time: days(count: 3), history: entries,
                          sessionRows: [first, second], checkoutDetails: [
                            "unit-1": ["dirty": true, "commits_since": 3],
                            "unit-2": ["dirty": false, "commits_since": 0]])
    }

    static func projectCorrections() throws -> ProjectDetail {
        let entries = history(count: 2, spacing: 86_400).map { entry -> Object in
            var value = entry
            value["kind"] = "correction"
            return value
        }
        return try detail(history: entries)
    }

    struct Segment {
        var project: Int? = 0
        var agent = "claude"
        var from: Double
        var to: Double
    }

    struct Owner {
        var from: Double
        var to: Double
        var projects: [Int?] = [0]
    }

    static func timelineRegistryBoard() throws -> ProjectBoard {
        try board([row(0), row(1), row(2, state: "archived"), row(3, state: "ignored"),
                   row(4, mergedInto: key(0)), row(5)])
    }

    static func timelineRegistryDay() throws -> ProjectTimeline {
        try timeline(projectCount: 5, segments: [.init(project: 1, from: 540, to: 600)])
    }

    static func timeline(day: String = "2026-09-29", zone: TimeZone = utc,
                         projectCount: Int = 1, long: Bool = false, segments: [Segment] = [],
                         owner: [Owner] = [], available: Bool = true) throws -> ProjectTimeline {
        let calendar = Self.calendar(zone)
        guard let date = TimelineDay.date(day, calendar: calendar),
              let interval = calendar.dateInterval(of: .day, for: date) else { throw AppFixturePreview.Failure.fixture }
        let start = interval.start.timeIntervalSince1970
        let blocks: [Object] = segments.enumerated().map { index, segment in
            ["id": "segment-\(index + 1)", "stream": "fixture-\(index + 1)",
             "project": nullable(segment.project.map { index in key(index) }), "agent": segment.agent, "driver": "owner",
             "from": start + segment.from * 60, "to": start + segment.to * 60]
        }
        let ranges: [Object] = owner.enumerated().map { index, range in
            let assignments: [Object] = range.projects.map { project in
                ["project": nullable(project.map { index in key(index) }), "share": 1 / Double(range.projects.count),
                 "confidence": nullable(project == nil ? nil : "high")]
            }
            return ["id": "owner-\(index + 1)", "from": start + range.from * 60,
                    "to": start + range.to * 60, "assignments": assignments]
        }
        return try decode([
            "day": day, "from": start, "to": interval.end.timeIntervalSince1970,
            "projects": (0..<projectCount).map { index in ["key": key(index), "name": name(index, long: long)] },
            "segments": blocks, "owner": ranges, "owner_available": available
        ], as: ProjectTimeline.self)
    }
}
