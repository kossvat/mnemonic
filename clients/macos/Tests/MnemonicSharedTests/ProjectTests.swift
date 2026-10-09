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
        XCTAssertNil(unit.error)
        XCTAssertEqual(detail.sessions.map(\.agent), ["claude"])
        XCTAssertEqual(detail.sessions[0].state, .running)
        XCTAssertEqual(detail.sessions[0].startedAt, 1_790_762_400)
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
        XCTAssertEqual(detail.topTerms, [])
        try roundTrip(detail)
    }

    func testDetailKeepsMissingNullAndPopulatedTopTermsCompatible() throws {
        var value = try object("project-detail")
        value.removeValue(forKey: "top_terms")
        let missing = try decode(ProjectDetail.self, value)
        XCTAssertNil(missing.topTerms)
        try roundTrip(missing)

        value["top_terms"] = NSNull()
        let unknown = try decode(ProjectDetail.self, value)
        XCTAssertNil(unknown.topTerms)
        try roundTrip(unknown)

        value["top_terms"] = [
            ["name": "index", "memory_count": 7],
            ["name": "sync", "memory_count": 3],
        ]
        let populated = try decode(ProjectDetail.self, value)
        let terms = try XCTUnwrap(populated.topTerms)
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
