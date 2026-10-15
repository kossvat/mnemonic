import Foundation
import MnemonicShared
import XCTest

/// These fixture bytes are also compared with the daemon's project routes.
final class ProjectTests: XCTestCase {
    private func fixture(_ name: String) throws -> Data {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent()
            .deletingLastPathComponent()
            .appendingPathComponent("Fixtures/\(name).json")
        return try Data(contentsOf: url)
    }

    private func object(_ name: String) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: fixture(name)) as? [String: Any])
    }

    private func decode<T: Decodable>(_ type: T.Type, _ object: [String: Any]) throws -> T {
        try JSONDecoder().decode(type, from: JSONSerialization.data(withJSONObject: object))
    }

    private func roundTrip<T: Codable & Equatable>(_ value: T) throws {
        let encoded = try JSONEncoder().encode(value)
        XCTAssertEqual(try JSONDecoder().decode(T.self, from: encoded), value)
    }

    func testBoardDecodesTheDaemonFixture() throws {
        let board = try JSONDecoder().decode(ProjectBoard.self, from: fixture("project-board"))
        let project = try XCTUnwrap(board.projects.first)
        XCTAssertEqual(project.id, "demoapp")
        XCTAssertEqual(project.name, "demoapp")
        XCTAssertEqual(project.state, .active)
        XCTAssertNil(project.mergedInto)
        XCTAssertEqual(project.lastActiveAt, 1_790_762_460)
        XCTAssertEqual(project.week.ownerSeconds, 1500.0)
        XCTAssertEqual(project.week.agentSeconds, 3600)
        XCTAssertTrue(project.week.ownerEstimate)
        XCTAssertEqual(project.plan?.done, 1)
        XCTAssertEqual(project.plan?.total, 2)
        XCTAssertEqual(project.plan?.stage, "building")
        XCTAssertEqual(project.plan?.task, "Review the result")
        XCTAssertNil(project.planMessage)
        XCTAssertEqual(project.gateAt, .null)
        XCTAssertEqual(project.planProvider, .value(.checkbox))
        XCTAssertEqual(project.planCounts, .value(["done": 1, "planned": 1]))
        XCTAssertEqual(project.planWeeklyCount, .value(0))
        XCTAssertEqual(project.openUnits, .value(1))
        XCTAssertEqual(project.waitingOnOwner, .value([]))
        XCTAssertEqual(project.lastClosed, .null)
        XCTAssertEqual(board.week.value?.ownerSeconds, 1500.0)
        XCTAssertEqual(board.week.value?.agentSeconds, 3600)
        XCTAssertEqual(board.week.value?.ownerEstimate, true)
        try roundTrip(board)
    }

    func testDetailDecodesPlanSessionsTimeAndHistory() throws {
        let detail = try JSONDecoder().decode(ProjectDetail.self, from: fixture("project-detail"))
        let unit = try XCTUnwrap(detail.plan.units.first)
        XCTAssertEqual(detail.id, "demoapp")
        XCTAssertEqual(detail.state, .active)
        XCTAssertNil(detail.plan.message)
        XCTAssertEqual(unit.provider, .checkbox)
        XCTAssertEqual(unit.stage, .building)
        XCTAssertFalse(unit.stale)
        XCTAssertEqual(unit.completionFraction, 0.5)
        XCTAssertEqual(unit.currentTask?.id, "T02")
        XCTAssertEqual(unit.currentTask?.source, "first_open")
        XCTAssertEqual(unit.counts, ["done": 1, "planned": 1])
        XCTAssertEqual(unit.tasks.map(\.id), ["T01", "T02"])
        XCTAssertNil(unit.tasks[0].owner)
        XCTAssertEqual(unit.waitingOnOwner, [])
        XCTAssertEqual(unit.weeklyCount, 0)
        XCTAssertEqual(unit.checkouts, 0)
        XCTAssertEqual(unit.asOf, 1_790_762_460)
        XCTAssertEqual(unit.gate.status, .none)
        XCTAssertNil(unit.gate.at)
        XCTAssertEqual(unit.gate.failedRuns, 0)
        XCTAssertEqual(unit.gate.result, .null)
        XCTAssertEqual(unit.closed, .value(false))
        XCTAssertEqual(unit.closedAt, .null)
        XCTAssertNil(unit.error)
        XCTAssertEqual(detail.sessions.map(\.agent), ["claude"])
        XCTAssertEqual(detail.sessions[0].state, .running)
        XCTAssertEqual(detail.sessions[0].startedAt, 1_790_762_400)
        XCTAssertEqual(detail.sessions[0].lastRecordAt, .value(1_790_762_460))
        let gateDetail = try XCTUnwrap(detail.gateDetail?["plan"])
        XCTAssertNil(gateDetail.dirty)
        XCTAssertNil(gateDetail.commitsSince)
        XCTAssertEqual(detail.time.count, 14)
        XCTAssertEqual(detail.time[12].ownerSeconds, 1200.0)
        XCTAssertEqual(detail.time[12].agentSeconds, 3600)
        XCTAssertEqual(detail.time[12].confidence, "high")
        XCTAssertTrue(detail.ownerEstimate)
        XCTAssertEqual(detail.history.map(\.id), ["decision-a"])
        XCTAssertEqual(detail.history[0].title, "Use the local cache")
        XCTAssertEqual(detail.history[0].at, 1_790_762_460)
        XCTAssertEqual(detail.history[0].kind, .value("decision"))
        XCTAssertEqual(detail.topTerms, [])
        try roundTrip(detail)
    }

    func testDetailKeepsMissingNullEmptyAndPopulatedTopTermsCompatible() throws {
        var value = try object("project-detail")
        value.removeValue(forKey: "top_terms")
        let missing = try decode(ProjectDetail.self, value)
        XCTAssertNil(missing.topTerms)
        XCTAssertEqual(missing.topTermsPresence, .missing)
        try roundTrip(missing)

        value["top_terms"] = NSNull()
        let absent = try decode(ProjectDetail.self, value)
        XCTAssertNil(absent.topTerms)
        XCTAssertEqual(absent.topTermsPresence, .null)
        try roundTrip(absent)

        value["top_terms"] = [] as [Any]
        let empty = try decode(ProjectDetail.self, value)
        XCTAssertEqual(empty.topTerms, [])
        XCTAssertEqual(empty.topTermsPresence, .value([]))
        try roundTrip(empty)

        value["top_terms"] = [
            ["name": "index", "memory_count": 7],
            ["name": "sync", "memory_count": 3],
        ]
        let populated = try decode(ProjectDetail.self, value)
        let terms = try XCTUnwrap(populated.topTerms)
        XCTAssertEqual(populated.topTermsPresence, .value(terms))
        XCTAssertEqual(terms.map(\.name), ["index", "sync"])
        XCTAssertEqual(terms.map(\.memoryCount), [7, 3])
        try roundTrip(populated)
    }

    func testBoardPreservesUnknownTimeMergedKeysAndMissingOptionalFields() throws {
        let original = try object("project-board")
        var rows = try XCTUnwrap(original["projects"] as? [[String: Any]])
        var row = rows[0]
        row["merged_into"] = "canonical-project"
        row["state"] = "archived"
        row["last_active_at"] = NSNull()
        row["plan"] = NSNull()
        row["plan_message"] = "no plan file"
        row["week"] = ["owner_s": NSNull(), "agent_s": 0, "owner_estimate": true]
        rows[0] = row
        let board = try decode(ProjectBoard.self, ["projects": rows])
        XCTAssertEqual(board.projects[0].mergedInto, "canonical-project")
        XCTAssertEqual(board.projects[0].state, .archived)
        XCTAssertNil(board.projects[0].lastActiveAt)
        XCTAssertNil(board.projects[0].week.ownerSeconds)
        XCTAssertNil(board.projects[0].plan)
        XCTAssertEqual(board.projects[0].planMessage, "no plan file")
        try roundTrip(board)

        for key in ["merged_into", "last_active_at", "plan", "plan_message"] {
            row.removeValue(forKey: key)
        }
        row["week"] = ["owner_s": 1500.5, "agent_s": 3600, "owner_estimate": true]
        let missing = try decode(ProjectBoard.self, ["projects": [row]])
        XCTAssertNil(missing.projects[0].mergedInto)
        XCTAssertNil(missing.projects[0].planMessage)
        XCTAssertEqual(missing.projects[0].week.ownerSeconds, 1500.5)
        try roundTrip(missing)
    }

    func testFunnelKeepsStatusCountsAndNeverComputesAPercentage() throws {
        var detail = try object("project-detail")
        var plan = try XCTUnwrap(detail["plan"] as? [String: Any])
        let units = try XCTUnwrap(plan["units"] as? [[String: Any]])
        var unit = units[0]
        unit["provider"] = "funnel"
        unit["done"] = 0
        unit["total"] = 0
        unit["stage"] = "planned"
        unit["current_task"] = NSNull()
        unit["tasks"] = [] as [Any]
        unit["counts"] = ["contacted": 2, "waiting_on_owner": 1]
        unit["waiting_on_owner"] = ["Confirm the draft"]
        unit["weekly_count"] = 3
        plan["units"] = [unit]
        detail["plan"] = plan
        detail["gate_detail"] = NSNull()
        detail["time"] = [["day": "2026-09-30", "owner_s": NSNull(),
                            "agent_s": 0, "confidence": NSNull()]]
        let funnel = try decode(ProjectDetail.self, detail)
        let funnelUnit = try XCTUnwrap(funnel.plan.units.first)
        XCTAssertEqual(funnelUnit.provider, .funnel)
        XCTAssertNil(funnelUnit.completionFraction)
        XCTAssertNil(funnelUnit.currentTask)
        XCTAssertEqual(funnelUnit.counts, ["contacted": 2, "waiting_on_owner": 1])
        XCTAssertEqual(funnelUnit.waitingOnOwner, ["Confirm the draft"])
        XCTAssertEqual(funnelUnit.weeklyCount, 3)
        XCTAssertNil(funnel.gateDetail)
        XCTAssertNil(funnel.time[0].ownerSeconds)
        XCTAssertNil(funnel.time[0].confidence)
        try roundTrip(funnel)

        unit["done"] = 1
        unit["total"] = 2
        let counted = try decode(ProjectPlanUnit.self, unit)
        XCTAssertNil(counted.completionFraction)
    }

    /// A daemon of an earlier build omits the additions of design 9.1: each
    /// decodes as missing, not as a recorded null, and is not written back.
    func testAdditionsFromAnEarlierDaemonAreMissingAndStayOmitted() throws {
        var board = try object("project-board")
        var rows = try XCTUnwrap(board["projects"] as? [[String: Any]])
        for key in ["gate_at", "plan_provider", "plan_counts", "plan_weekly_count",
                    "open_units", "waiting_on_owner", "last_closed"] {
            rows[0].removeValue(forKey: key)
        }
        board["projects"] = rows
        board.removeValue(forKey: "week")
        let earlierBoard = try decode(ProjectBoard.self, board)
        XCTAssertEqual(earlierBoard.week, .missing)
        try roundTrip(earlierBoard)
        let row = try XCTUnwrap(earlierBoard.projects.first)
        XCTAssertEqual(row.gateAt, .missing)
        XCTAssertEqual(row.planProvider, .missing)
        XCTAssertEqual(row.planCounts, .missing)
        XCTAssertEqual(row.planWeeklyCount, .missing)
        XCTAssertEqual(row.openUnits, .missing)
        XCTAssertEqual(row.waitingOnOwner, .missing)
        XCTAssertEqual(row.lastClosed, .missing)
        let rowKeys = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(row)) as? [String: Any])
        XCTAssertEqual(Set(rowKeys.keys), Set(rows[0].keys).subtracting(["merged_into", "plan_message"]))

        var detail = try object("project-detail")
        var plan = try XCTUnwrap(detail["plan"] as? [String: Any])
        var units = try XCTUnwrap(plan["units"] as? [[String: Any]])
        units[0].removeValue(forKey: "closed")
        units[0].removeValue(forKey: "closed_at")
        var gate = try XCTUnwrap(units[0]["gate"] as? [String: Any])
        gate.removeValue(forKey: "result")
        units[0]["gate"] = gate
        plan["units"] = units
        detail["plan"] = plan
        var sessions = try XCTUnwrap(detail["sessions"] as? [[String: Any]])
        sessions[0].removeValue(forKey: "last_record_at")
        detail["sessions"] = sessions
        var history = try XCTUnwrap(detail["history"] as? [[String: Any]])
        history[0].removeValue(forKey: "kind")
        detail["history"] = history
        let earlier = try decode(ProjectDetail.self, detail)
        let unit = try XCTUnwrap(earlier.plan.units.first)
        XCTAssertEqual(unit.closed, .missing)
        XCTAssertEqual(unit.closedAt, .missing)
        XCTAssertEqual(unit.gate.result, .missing)
        XCTAssertEqual(earlier.sessions[0].lastRecordAt, .missing)
        XCTAssertEqual(earlier.history[0].kind, .missing)
        try roundTrip(earlier)
        let written = String(decoding: try JSONEncoder().encode(earlier), as: UTF8.self)
        for key in ["\"closed\"", "closed_at", "\"result\"", "last_record_at", "\"kind\""] {
            XCTAssertFalse(written.contains(key), key)
        }
    }

    func testAdditionsCarryTheValuesTheDaemonHolds() throws {
        var board = try object("project-board")
        var rows = try XCTUnwrap(board["projects"] as? [[String: Any]])
        rows[0]["gate_at"] = 1_700_000_000
        rows[0]["plan_provider"] = "funnel"
        rows[0]["plan_counts"] = ["contacted": 4, "replied": 1]
        rows[0]["plan_weekly_count"] = 3
        rows[0]["open_units"] = 2
        rows[0]["waiting_on_owner"] = ["Confirm the copy"]
        rows[0]["last_closed"] = ["title": "Shipped", "closed_at": 1_767_312_000]
        board["projects"] = rows
        board["week"] = ["owner_s": NSNull(), "agent_s": 6000, "owner_estimate": true]
        let served = try decode(ProjectBoard.self, board)
        XCTAssertEqual(served.week.value?.ownerSeconds, nil)
        XCTAssertEqual(served.week.value?.agentSeconds, 6000)
        try roundTrip(served)
        let row = try XCTUnwrap(served.projects.first)
        XCTAssertEqual(row.openUnits, .value(2))
        XCTAssertEqual(row.waitingOnOwner, .value(["Confirm the copy"]))
        XCTAssertEqual(row.lastClosed.value?.title, "Shipped")
        XCTAssertEqual(row.lastClosed.value?.closedAt, .value(1_767_312_000))
        let undated = try decode(ProjectLastClosed.self, ["title": "Done in place", "closed_at": NSNull()])
        XCTAssertEqual(undated.closedAt, .null)
        try roundTrip(undated)
        let written = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(undated)) as? [String: Any])
        XCTAssertTrue(written["closed_at"] is NSNull)
        let earlier = try decode(ProjectLastClosed.self, ["title": "Done in place"])
        XCTAssertEqual(earlier.closedAt, .missing)
        XCTAssertFalse(String(decoding: try JSONEncoder().encode(earlier), as: UTF8.self).contains("closed_at"))
        XCTAssertEqual(row.gateAt, .value(1_700_000_000))
        XCTAssertEqual(row.planProvider, .value(.funnel))
        XCTAssertEqual(row.planCounts, .value(["contacted": 4, "replied": 1]))
        XCTAssertEqual(row.planWeeklyCount, .value(3))
        try roundTrip(row)

        let plan = try XCTUnwrap(try object("project-detail")["plan"] as? [String: Any])
        var unit = try XCTUnwrap((plan["units"] as? [[String: Any]])?.first)
        unit["closed"] = true
        unit["closed_at"] = 1_009_929_600
        unit["gate"] = ["status": "stale", "at": 1_700_000_000, "failed_runs": 0, "result": "passed"]
        let closed = try decode(ProjectPlanUnit.self, unit)
        XCTAssertEqual(closed.closed, .value(true))
        XCTAssertEqual(closed.closedAt, .value(1_009_929_600))
        XCTAssertEqual(closed.gate.status, .stale)
        XCTAssertEqual(closed.gate.result, .value(.passed))
        try roundTrip(closed)
        unit["gate"] = ["status": "failed", "at": NSNull(), "failed_runs": 1, "result": "failed"]
        XCTAssertEqual(try decode(ProjectPlanUnit.self, unit).gate.result, .value(.failed))
    }

    func testGateDetailsKeepFalseAndZeroDistinctFromUnknown() throws {
        let measured = try decode(ProjectGateDetail.self, ["dirty": false, "commits_since": 0])
        XCTAssertEqual(measured.dirty, false)
        XCTAssertEqual(measured.commitsSince, 0)
        try roundTrip(measured)
        let unknown = try decode(ProjectGateDetail.self, [:])
        XCTAssertNil(unknown.dirty)
        XCTAssertNil(unknown.commitsSince)
        try roundTrip(unknown)
    }
}
