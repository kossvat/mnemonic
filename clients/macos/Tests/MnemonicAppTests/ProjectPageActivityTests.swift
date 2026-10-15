import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class ProjectPageActivityTests: XCTestCase {
    private let now = FixtureWire.now
    private let calendar = FixtureWire.calendar()

    func testSessionLastRecordMissingNullAndValueAtAFixedClock() throws {
        var object = FixtureWire.session("codex")
        func line() throws -> ProjectPresentation.PageSession {
            ProjectPresentation.pageSession(try FixtureWire.decode(object, as: LiveSession.self),
                                            index: 0, now: now, calendar: calendar)
        }
        XCTAssertNil(try line().lastRecord)
        XCTAssertEqual(try line().elapsed, "1h")
        XCTAssertEqual(try line().agent, "Codex")
        XCTAssertEqual(try line().state, "Running")
        object["last_record_at"] = NSNull()
        XCTAssertEqual(try line().lastRecord, "No last record time recorded")
        object["last_record_at"] = Int(now.timeIntervalSince1970)
        XCTAssertEqual(try line().lastRecord, "last record under 1 min ago")
        object["state"] = "waiting"
        XCTAssertEqual(try line().state, "Waiting for you")
        object["state"] = "quiet"
        XCTAssertEqual(try line().state, "Quiet")
        object["started_at"] = Int(now.timeIntervalSince1970) + 60
        XCTAssertEqual(try line().elapsed, "0m")
    }

    func testLastRecordAgeUsesOnlyThePageClockAndClampsFutureRecords() throws {
        let cases: [(Int, String)] = [(-60, "under 1 min"), (0, "under 1 min"), (59, "under 1 min"),
            (60, "1 min"), (120, "2 min"), (3_599, "59 min"), (3_600, "1 hr"),
            (7_200, "2 hr"), (86_400, "1 day"), (172_800, "2 days")]
        for (age, label) in cases {
            var object = FixtureWire.session("claude")
            object["last_record_at"] = Int(now.timeIntervalSince1970) - age
            let session = try FixtureWire.decode(object, as: LiveSession.self)
            let display = ProjectPresentation.pageSession(session, index: 0, now: now, calendar: calendar)
            XCTAssertEqual(display.lastRecord, "last record " + label + " ago")
        }
    }

    func testRecentWorkKeepsEveryDayInDescendingRowsOfThreeAndTwoLineEntries() throws {
        let detail = try FixtureWire.detail(time: FixtureWire.days(count: 14), history: [
            ["id": "decision-1", "title": "Keep the local cache", "at": Int(now.timeIntervalSince1970) - 40 * 86_400,
             "kind": "correction"]
        ])
        let recent = ProjectPresentation.pageRecent(detail, calendar: calendar)
        XCTAssertEqual(recent.columnCount, 3)
        XCTAssertEqual(recent.days.count, 15)
        XCTAssertEqual(recent.days.map(\.id), recent.days.map(\.id).sorted(by: >))
        XCTAssertNil(recent.days.last?.times)
        XCTAssertEqual(recent.days.last?.history.first?.title, "Keep the local cache")
        XCTAssertEqual(recent.historyLineLimit, 2)
    }

    func testHistoryKindMissingNullAndValuePreservesItsTitle() throws {
        var object = FixtureWire.history()[0]
        func entry() throws -> ProjectPresentation.PageHistory {
            ProjectPresentation.pageHistory(try FixtureWire.decode(object, as: ProjectHistoryEntry.self))
        }
        XCTAssertNil(try entry().kind)
        object["kind"] = NSNull()
        XCTAssertEqual(try entry().kind, "Kind not recorded")
        object["kind"] = "decision"
        XCTAssertEqual(try entry().kind, "Decision")
        object["kind"] = "correction"
        XCTAssertEqual(try entry().kind, "Correction")
        XCTAssertEqual(try entry().title, "Keep the local cache 1")
        object["kind"] = "Review the guide"
        XCTAssertEqual(try entry().kind, "Review the guide")
    }

    func testRecentWorkKeepsAllServedTimeAndPartialOwnerAttributionUnavailable() throws {
        let rows: [FixtureWire.Object] = (1...16).map { number in
            ["day": String(format: "2026-03-%02d", number),
             "owner_s": number == 16 ? NSNull() : 60 as Any, "agent_s": 120]
        }
        let detail = try FixtureWire.detail(time: rows, history: [])
        let recent = ProjectPresentation.pageRecent(detail, calendar: calendar)
        XCTAssertEqual(recent.days.count, 16)
        XCTAssertEqual(recent.days.first?.id, "2026-03-16")
        XCTAssertEqual(recent.days.last?.id, "2026-03-01")
        XCTAssertEqual(recent.days.map(\.id), recent.days.map(\.id).sorted(by: >))
        XCTAssertEqual(recent.days.first?.times?.owner, "Unavailable")
        XCTAssertTrue(recent.days.dropFirst().allSatisfy { day in day.times?.owner == "~1m" })
        XCTAssertTrue(recent.days.allSatisfy { day in day.times?.agents == "2m" })
        XCTAssertNil(recent.timeMessage)
    }

    func testHistoryDaysIncludeTheOldestEvenOutsideTheServedTimeDays() throws {
        let at = Int(now.timeIntervalSince1970)
        let history: [FixtureWire.Object] = [
            ["id": "decision-1", "title": "Keep the local cache", "at": at, "kind": "decision"],
            ["id": "decision-2", "title": "Review the guide", "at": at - 40 * 86_400, "kind": "correction"]
        ]
        let detail = try FixtureWire.detail(time: FixtureWire.days(count: 3), history: history)
        let display = ProjectPresentation.pageRecent(detail, calendar: calendar)
        XCTAssertEqual(display.days.count, 4)
        XCTAssertEqual(display.days.first?.history.first?.kind, "Decision")
        XCTAssertEqual(display.days.last?.id, "2026-08-21")
        XCTAssertEqual(display.days.last?.history.first?.title, "Review the guide")
        XCTAssertEqual(display.days.last?.history.first?.kind, "Correction")
        XCTAssertNil(display.days.last?.times)
        XCTAssertTrue(display.days[1].history.isEmpty)
        XCTAssertTrue(display.days[2].history.isEmpty)
        XCTAssertNil(display.historyMessage)
    }

    func testHistoryUsesLocalDaysAndStableOrderIncludingTheOldestReturnedTitle() throws {
        let zone = try XCTUnwrap(TimeZone(identifier: "America/Los_Angeles"))
        let local = FixtureWire.calendar(zone)
        let at = Int(now.timeIntervalSince1970) - 11 * 3_600 + 60
        let detail = try FixtureWire.detail(history: [
            ["id": "decision-2", "title": "Review the guide", "at": at],
            ["id": "decision-1", "title": "Keep the local cache", "at": at],
            ["id": "decision-3", "title": "Verify the result", "at": at - 86_400]
        ])
        let display = ProjectPresentation.pageRecent(detail, calendar: local)
        XCTAssertTrue(display.days.first { day in day.id == "2026-09-30" }?.history.isEmpty == true)
        XCTAssertEqual(display.days.first { day in day.id == "2026-09-29" }?.history.map(\.id),
                       ["decision-1", "decision-2"])
        XCTAssertEqual(display.days.first { day in day.id == "2026-09-28" }?.history.map(\.id), ["decision-3"])
    }

    func testEmptyHistoryLimitsItsClaimAndNullTimeIsDifferentFromZero() throws {
        let absent = ProjectPresentation.pageRecent(
            try FixtureWire.detail(time: FixtureWire.days(count: 1, owner: nil), history: []), calendar: calendar)
        XCTAssertEqual(absent.historyMessage, "The returned history holds no decision or correction.")
        XCTAssertEqual(absent.days.first?.times?.owner, "Unavailable")
        XCTAssertTrue(absent.days.first?.history.isEmpty == true)
        let zero = ProjectPresentation.pageRecent(
            try FixtureWire.detail(time: FixtureWire.days(count: 1, owner: 0, agent: 0)), calendar: calendar)
        XCTAssertEqual(zero.days.first?.times?.owner, "~0m")
        XCTAssertEqual(zero.days.first?.times?.agents, "0m")
    }

    func testCorrectionsOnlyRemainVisibleAndMoreThanTwentyFourSessionsStay() throws {
        let detail = try FixtureWire.projectCorrections()
        let recent = ProjectPresentation.pageRecent(detail, calendar: calendar)
        let entries = recent.days.flatMap(\.history)
        XCTAssertFalse(entries.isEmpty)
        XCTAssertTrue(entries.allSatisfy { entry in entry.kind == "Correction" })
        XCTAssertNil(recent.historyMessage)
        let many = try FixtureWire.detail(agents: Array(repeating: "codex", count: 27))
        let page = ProjectPresentation.page(detail: many, failure: nil, liveFailure: nil,
                                            loading: false, now: now, calendar: calendar)
        XCTAssertEqual(page.sessions.count, 27)
        XCTAssertEqual(page.sessionCount, "27 live sessions")
    }
}
