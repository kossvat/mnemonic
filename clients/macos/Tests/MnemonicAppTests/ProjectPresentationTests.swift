import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicApp

final class ProjectPresentationTests: XCTestCase {
    private func fixture(_ name: String) throws -> [String: Any] {
        let path = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/\(name).json")
        return try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: path)) as? [String: Any])
    }

    private func decode<T: Decodable>(_ type: T.Type, _ value: [String: Any]) throws -> T {
        try JSONDecoder().decode(type, from: JSONSerialization.data(withJSONObject: value))
    }

    private func row(_ key: String, at: Int?, state: String = "active", merged: String? = nil) throws -> [String: Any] {
        var result = try XCTUnwrap(fixture("project-board")["projects"] as? [[String: Any]])[0]
        result["key"] = key
        result["name"] = key
        result["last_active_at"] = at.map { value in value as Any } ?? NSNull()
        result["state"] = state
        result["merged_into"] = merged.map { value in value as Any } ?? NSNull()
        return result
    }

    private func live(_ keys: [String]) throws -> LiveSnapshot {
        var result = try fixture("live")
        let original = try XCTUnwrap(result["projects"] as? [[String: Any]])[0]
        result["projects"] = keys.map { key in
            var project = original
            project["key"] = key
            project["name"] = key
            return project
        }
        return try decode(LiveSnapshot.self, result)
    }

    private func unit() throws -> [String: Any] {
        let plan = try XCTUnwrap(fixture("project-detail")["plan"] as? [String: Any])
        return try XCTUnwrap(plan["units"] as? [[String: Any]])[0]
    }

    func testLiveProjectsStayOutOfTheStaleFoldAndOtherRowsFollowActivity() throws {
        let now = 1_800_000_000
        let day = 86_400
        let rows = try [
            row("older", at: now - 120), row("live-old", at: now - 90 * day),
            row("stale", at: now - 31 * day), row("recent", at: now - 60),
            row("unknown", at: nil), row("boundary", at: now - 30 * day),
        ]
        let board = try decode(ProjectBoard.self, ["projects": rows])
        let groups = ProjectPresentation.groups(
            board: board, live: try live(["live-old"]), now: Date(timeIntervalSince1970: Double(now)))
        XCTAssertEqual(groups.live.map(\.id), ["live-old"])
        XCTAssertEqual(groups.recent.map(\.key), ["recent", "older", "boundary", "unknown"])
        XCTAssertEqual(groups.stale.map(\.key), ["stale"])
        XCTAssertEqual(groups.live.first?.row?.week.ownerSeconds, 1_500)
    }

    func testExcludedRowsAreCountedOnceAndCannotBecomeLiveCards() throws {
        let rows = try [
            row("archive", at: 1, state: "archived"), row("ignore", at: 1, state: "ignored"),
            row("merge", at: 1, state: "archived", merged: "canonical"),
        ]
        let board = try decode(ProjectBoard.self, ["projects": rows])
        let groups = ProjectPresentation.groups(
            board: board, live: try live(["archive", "ignore", "merge"]), now: Date(timeIntervalSince1970: 2))
        XCTAssertTrue(groups.live.isEmpty)
        XCTAssertTrue(groups.recent.isEmpty)
        XCTAssertTrue(groups.stale.isEmpty)
        XCTAssertEqual(groups.excluded, 3)
        XCTAssertEqual(groups.excludedLabel, "1 archived · 1 ignored · 1 merged")
    }

    func testLiveDataCanArriveBeforeTheBoardWithoutInventingWeeklyTime() throws {
        let groups = ProjectPresentation.groups(board: nil, live: try live(["arriving"]), now: Date())
        let card = try XCTUnwrap(groups.live.first)
        XCTAssertEqual(card.id, "arriving")
        XCTAssertNil(card.row)
        XCTAssertEqual(ProjectPresentation.ownerTime(card.row?.week.ownerSeconds, estimated: true), "Unknown")
        let empty = ProjectPresentation.groups(board: nil, live: nil, now: Date())
        XCTAssertTrue(empty.live.isEmpty)
        XCTAssertTrue(empty.recent.isEmpty)
        XCTAssertTrue(empty.stale.isEmpty)
        XCTAssertEqual(empty.excluded, 0)
    }

    func testUnknownOwnerTimeIsDifferentFromZeroAndDoesNotReceiveAnEstimateMark() {
        XCTAssertEqual(ProjectPresentation.ownerTime(nil, estimated: true), "Unknown")
        XCTAssertEqual(ProjectPresentation.ownerTime(0, estimated: true), "~0m")
        XCTAssertEqual(ProjectPresentation.ownerTime(3_900, estimated: true), "~1h 5m")
        XCTAssertEqual(ProjectPresentation.ownerTime(3_600, estimated: false), "1h")
        XCTAssertEqual(ProjectPresentation.duration(.infinity), "Unknown")
        XCTAssertEqual(ProjectPresentation.activity(nil, now: Date()), "No activity recorded")
    }

    func testTaskCompletionDoesNotPromoteTheRecordedReviewStage() throws {
        var object = try unit()
        object["done"] = 2
        object["total"] = 2
        object["stage"] = "review"
        let model = try decode(ProjectPlanUnit.self, object)
        XCTAssertEqual(ProjectPresentation.completion(model), 1)
        XCTAssertEqual(ProjectPresentation.progressLabel(model), "2 of 2 tasks done")
        XCTAssertEqual(model.stage, .review)
        object["total"] = 0
        let empty = try decode(ProjectPlanUnit.self, object)
        XCTAssertNil(ProjectPresentation.completion(empty))
        XCTAssertEqual(ProjectPresentation.progressLabel(empty), "No task count recorded")
    }

    func testAStatusTableNeverShowsTaskCompletion() throws {
        var object = try unit()
        object["provider"] = "funnel"
        object["done"] = 3
        object["total"] = 4
        object["weekly_count"] = 7
        let model = try decode(ProjectPlanUnit.self, object)
        XCTAssertNil(ProjectPresentation.completion(model))
        XCTAssertEqual(ProjectPresentation.progressLabel(model), "7 this week")
    }

    func testGateReceiptAndUnknownCheckoutRemainSeparateMeasurements() throws {
        let unknown = try decode(ProjectGateDetail.self, ["dirty": NSNull(), "commits_since": NSNull()])
        let clean = try decode(ProjectGateDetail.self, ["dirty": false, "commits_since": 0])
        XCTAssertEqual(ProjectPresentation.gate(.none), "No gate receipt")
        XCTAssertEqual(ProjectPresentation.gate(.stale), "Out of date")
        XCTAssertEqual(ProjectPresentation.checkout(unknown), "Working tree unknown")
        XCTAssertEqual(ProjectPresentation.commits(unknown), "Commits since gate unknown")
        XCTAssertEqual(ProjectPresentation.checkout(clean), "No uncommitted changes")
        XCTAssertEqual(ProjectPresentation.commits(clean), "0 commits since gate")
    }

    func testTimeShowsTheLatestFourteenDaysAndKeepsPartialOwnerAttributionUnknown() throws {
        let rows = try (1...16).map { number in
            try decode(ProjectDay.self, [
                "day": String(format: "2026-03-%02d", number),
                "owner_s": number == 16 ? NSNull() : 60 as Any,
                "agent_s": 120,
            ])
        }
        let days = ProjectPresentation.days(Array(rows.reversed()))
        XCTAssertEqual(days.count, 14)
        XCTAssertEqual(days.first?.day, "2026-03-03")
        XCTAssertEqual(days.last?.day, "2026-03-16")
        XCTAssertNil(ProjectPresentation.ownerTotal(days))
        XCTAssertEqual(ProjectPresentation.ownerTotal(Array(days.dropLast())), 13 * 60)
    }

    func testFailureStatesHaveDistinctSpecificMessages() {
        let failures: [LocalAPIFailure] = [.off, .unreachable, .unauthorised, .earlierDaemon, .undecodable]
        let titles = failures.map { failure in ProjectPresentation.failure(failure).title }
        XCTAssertEqual(Set(titles).count, failures.count)
        XCTAssertTrue(failures.allSatisfy { failure in !ProjectPresentation.failure(failure).detail.isEmpty })
    }
}
