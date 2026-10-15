import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class BoardPresentationTests: XCTestCase {
    private let now = FixtureWire.now

    private func display(board: ProjectBoard? = nil, live: LiveSnapshot? = nil,
                         failure: LocalAPIFailure? = nil, loading: Bool = false) -> ProjectPresentation.Board {
        ProjectPresentation.board(board: board, live: live, failure: failure, loading: loading,
                                  now: now, calendar: FixtureWire.calendar())
    }

    func testHeaderUsesServedTotalsRatherThanAddingProjectTimes() throws {
        var object: FixtureWire.Object = [
            "boot": "fixture", "v": 1, "projects": [FixtureWire.liveProject(0), FixtureWire.liveProject(1)],
            "more_projects": 0, "unknown": ["sessions": [], "more_sessions": 0],
            "today": ["owner_s": 60, "agent_s": 120]
        ]
        let live = try FixtureWire.decode(object, as: LiveSnapshot.self)
        XCTAssertEqual(display(live: live).today, .init(owner: "1m", agents: "2m"))
        object["today"] = ["owner_s": 0, "agent_s": 0]
        let zero = try FixtureWire.decode(object, as: LiveSnapshot.self)
        XCTAssertEqual(display(live: zero).today, .init(owner: "0m", agents: "0m"))
        XCTAssertNil(display(loading: true).today)
        XCTAssertNil(display(loading: true).active)
        XCTAssertEqual(display(live: zero).active?.projects, "2 projects")
        XCTAssertTrue(display().dateNote.hasPrefix("Wednesday, 30 September"))
    }

    func testActiveCountsIncludeUnknownSessionsAndCountEveryServedSession() throws {
        let live = try FixtureWire.live(count: 2, agents: [0: ["claude", "codex", "claude", "codex"]],
                                        unknown: FixtureWire.sessions(["claude"]))
        let result = display(live: live)
        XCTAssertEqual(result.active?.projects, "2 projects")
        XCTAssertEqual(result.active?.sessions, "3 Claude Code · 3 Codex")
        XCTAssertEqual(result.cards.first?.agents?.map(\.label), ["Claude Code 2", "Codex 2"])
        XCTAssertEqual(result.cards.first?.sessions, "4 sessions · longest 1h")
        XCTAssertEqual(result.notices, ["1 session has no recorded project."])
        XCTAssertFalse(result.cut)
        let empty = display(live: try FixtureWire.live())
        XCTAssertEqual(empty.active?.projects, "0 projects")
        XCTAssertEqual(empty.active?.sessions, "0 sessions")
        XCTAssertEqual(empty.liveMessage, "Nothing is in work now.")
    }

    func testAnySnapshotCutSuppressesAllKindsAndLongestButKeepsServedTotals() throws {
        let cuts = try [
            FixtureWire.live(count: 2, moreProjects: 3),
            FixtureWire.live(count: 2, moreSessions: [1: 2]),
            FixtureWire.live(count: 2, moreUnknown: 2)
        ]
        for live in cuts {
            let result = display(live: live)
            XCTAssertTrue(result.cut)
            XCTAssertEqual(result.active?.sessions, "Snapshot incomplete")
            XCTAssertTrue(result.cards.allSatisfy { card in card.agents == nil && card.longest == nil })
            XCTAssertFalse(result.cards.contains { card in card.sessions.contains("longest") })
            XCTAssertEqual(result.today, .init(owner: "20m", agents: "40m"))
            XCTAssertTrue(result.notices.first?.contains("Snapshot incomplete") == true)
        }
        XCTAssertEqual(display(live: cuts[0]).active?.projects, "5 projects")
        XCTAssertEqual(display(live: cuts[1]).cards[1].sessions, "3 sessions")
    }

    func testLongestUsesEarliestStartAtTheGivenClockAndClampsFutureStarts() throws {
        let start = Int(now.timeIntervalSince1970)
        let sessions = try [
            FixtureWire.session("claude", startedAt: start - 90),
            FixtureWire.session("codex", startedAt: start - 7_500),
            FixtureWire.session("claude", startedAt: start + 600)
        ].map { object in try FixtureWire.decode(object, as: LiveSession.self) }
        XCTAssertEqual(ProjectPresentation.longestSession(sessions, now: now, cut: false), "2h 5m")
        XCTAssertEqual(ProjectPresentation.longestSession([sessions[2]], now: now, cut: false), "0m")
        XCTAssertNil(ProjectPresentation.longestSession([], now: now, cut: false))
        XCTAssertNil(ProjectPresentation.longestSession(sessions, now: now, cut: true))
    }

    func testZeroTimeIsShownWhileAZeroTaskTotalHasNoProgressBlock() throws {
        let plan = FixtureWire.planLine(done: 0, total: 0)
        let board = try FixtureWire.board([FixtureWire.row(0, owner: 0, agent: 0, plan: plan)])
        let card = try XCTUnwrap(display(board: board, live: FixtureWire.live(count: 1)).cards.first)
        XCTAssertEqual(card.today, .init(owner: "~10m", agents: "20m"))
        XCTAssertEqual(card.week, .init(owner: "~0m", agents: "0m"))
        XCTAssertNil(card.plan?.progress)
        XCTAssertEqual(card.plan?.unit, "Review the guide")
        let zeroDone = LivePlan(unit: "Review the guide", done: 0, total: 2, stage: "planned", task: nil, gate: nil)
        XCTAssertEqual(ProjectPresentation.boardProgress(zeroDone)?.percent, "0%")
        XCTAssertEqual(ProjectPresentation.boardProgress(zeroDone)?.tasks, "0 of 2 tasks")
        XCTAssertEqual(ProjectPresentation.boardProgress(zeroDone)?.fraction, 0)
        let partial = LivePlan(unit: "Review the guide", done: 1, total: 3, stage: "building", task: nil, gate: nil)
        XCTAssertEqual(ProjectPresentation.boardProgress(partial)?.percent, "33%")
        XCTAssertEqual(ProjectPresentation.boardProgress(partial)?.fraction ?? 0, 1.0 / 3, accuracy: 0.0001)
    }

    func testMissingBoardRowOmitsItsBlocksAndRecordedAbsenceKeepsItsWords() throws {
        let live = try FixtureWire.live(count: 1)
        let arriving = try XCTUnwrap(display(live: live).cards.first)
        XCTAssertNil(arriving.week)
        // Until its row arrives a card draws the plan line of the live snapshot.
        let line = try XCTUnwrap(live.projects.first?.plan)
        XCTAssertEqual(arriving.plan?.unit, line.unit)
        XCTAssertNil(arriving.plan?.message)
        XCTAssertNil(arriving.footer)
        let board = try FixtureWire.board([FixtureWire.row(0, owner: nil, plan: nil)])
        let recorded = try XCTUnwrap(display(board: board, live: live).cards.first)
        XCTAssertEqual(recorded.week?.owner, "Unavailable")
        XCTAssertEqual(recorded.plan?.message, "No plan recorded")
        XCTAssertNil(recorded.plan?.unit)
        let unreadable = try FixtureWire.board([FixtureWire.row(0, plan: nil, message: "Plan read incomplete")])
        XCTAssertEqual(display(board: unreadable, live: live).cards.first?.plan?.message, "Plan read incomplete")
    }

    func testBoardPlanIsAuthoritativeAndPreservesRecordText() throws {
        let line = FixtureWire.planLine(unit: "Review the guide", stage: "blocked", task: "Check the local cache", gate: "stale")
        let board = try FixtureWire.board([FixtureWire.row(0, plan: line)])
        let card = try XCTUnwrap(display(board: board, live: FixtureWire.live(count: 1)).cards.first)
        XCTAssertEqual(card.plan?.task, "Check the local cache")
        XCTAssertEqual(card.plan?.stage, "Blocked")
        XCTAssertEqual(card.plan?.gate, "Gate · Stale")
        XCTAssertEqual(card.plan?.progress?.percent, "50%")
        for (value, words) in [("passed", "Gate · Passed"), ("failed", "Gate · Failed"),
                               ("stale", "Gate · Stale"), ("none", "Gate · Not recorded")] {
            XCTAssertEqual(ProjectPresentation.boardGate(value), words)
        }
        XCTAssertEqual(ProjectPresentation.boardGate(nil), "Gate · Not recorded")
        XCTAssertEqual(ProjectPresentation.boardGate("Review the guide"), "Gate · Review the guide")
    }

    func testRowsSortByLastActivityAndStaleFoldCountsOnlyOldRecordedActivity() throws {
        let board = try FixtureWire.board([
            FixtureWire.row(3, ageDays: nil), FixtureWire.row(2, ageDays: 31),
            FixtureWire.row(1, ageDays: 2), FixtureWire.row(0, ageDays: 2),
            FixtureWire.row(4, ageDays: 30), FixtureWire.row(5, ageDays: 40)
        ])
        let result = display(board: board, live: try FixtureWire.live())
        XCTAssertEqual(result.rows.map(\.id), ["beacon", "harbor", "juniper", "atlas"])
        XCTAssertEqual(result.rows.map(\.activity), ["28 Sep", "28 Sep", "31 Aug", "No activity recorded"])
        XCTAssertEqual(result.stale.map(\.id), ["ledger", "orchard"])
        XCTAssertEqual(result.staleLine, "2 stale projects")
        XCTAssertEqual(result.staleDetail, "Last recorded activity was more than 30 days ago.")
        XCTAssertEqual(result.rows.first?.week, .init(owner: "~25m", agents: "1h"))
        let single = display(board: try FixtureWire.board([FixtureWire.row(0, ageDays: 31)]))
        XCTAssertEqual(single.staleLine, "1 stale project")
        XCTAssertEqual(single.inactiveLine, "0 projects · last activity")
    }

    func testUnknownSessionsStayOutsideCardsAndExcludedRowsStayOutsideBothLists() throws {
        let board = try FixtureWire.board([FixtureWire.row(0, state: "archived"),
            FixtureWire.row(1, state: "ignored"), FixtureWire.row(2, mergedInto: FixtureWire.key(0))])
        let result = display(board: board, live: try FixtureWire.live(unknown: FixtureWire.sessions(["codex"])))
        XCTAssertTrue(result.cards.isEmpty)
        XCTAssertTrue(result.rows.isEmpty)
        XCTAssertTrue(result.stale.isEmpty)
        XCTAssertEqual(result.excluded, "1 archived · 1 ignored · 1 merged")
        XCTAssertEqual(result.active?.sessions, "1 Codex")
        XCTAssertEqual(result.notices, ["1 session has no recorded project."])
        XCTAssertEqual(result.liveMessage, "No live projects are shown on this board.")
        let omitted = display(live: try FixtureWire.live(moreProjects: 1))
        XCTAssertEqual(omitted.liveMessage, "No live projects are included in this snapshot.")
    }

    func testFailuresSuppressHeldLiveSessionsAndKeepBoardRows() throws {
        let board = try FixtureWire.board([FixtureWire.row(0)])
        let live = try FixtureWire.live(count: 1)
        for failure in [LocalAPIFailure.off, .unreachable, .unauthorised, .earlierDaemon, .undecodable] {
            let result = display(board: board, live: live, failure: failure)
            XCTAssertNil(result.today)
            XCTAssertNil(result.active)
            XCTAssertTrue(result.cards.isEmpty)
            XCTAssertEqual(result.rows.map(\.id), ["harbor"])
            XCTAssertEqual(result.failure?.title, ProjectPresentation.failure(failure).title)
        }
    }

    func testBoardWeekDistinguishesMissingNullAndServedTotals() throws {
        let rows = [FixtureWire.row(0), FixtureWire.row(1)]
        XCTAssertNil(display(board: try FixtureWire.board(rows)).week)
        let absent = try FixtureWire.decode(["projects": rows, "week": NSNull()], as: ProjectBoard.self)
        XCTAssertEqual(display(board: absent).week, .init(owner: "unavailable", agents: "unavailable"))
        for (owner, expected) in [(NSNull() as Any, "unavailable"), (0 as Any, "0m"), (120 as Any, "2m")] {
            let board = try FixtureWire.decode([
                "projects": rows, "week": ["owner_s": owner, "agent_s": 180, "owner_estimate": false]
            ], as: ProjectBoard.self)
            XCTAssertEqual(display(board: board).week, .init(owner: expected, agents: "3m"))
            XCTAssertEqual(display(board: board, failure: .unreachable).week?.owner, expected)
        }
        XCTAssertNil(display(loading: true).week)
    }

    func testGateDayDistinguishesMissingNullAndValueInTheScreenCalendar() throws {
        let calendar = FixtureWire.calendar()
        XCTAssertNil(ProjectPresentation.boardGateDate(.missing, calendar: calendar))
        XCTAssertEqual(ProjectPresentation.boardGateDate(.null, calendar: calendar), "No gate run recorded")
        let at = Int(now.timeIntervalSince1970) - 86_400
        XCTAssertEqual(ProjectPresentation.boardGateDate(.value(at), calendar: calendar), "29 Sep")
        var row = FixtureWire.row(0, plan: FixtureWire.planLine(gate: "stale"))
        row["gate_at"] = at
        let board = try FixtureWire.board([row])
        let card = try XCTUnwrap(display(board: board, live: FixtureWire.live(count: 1)).cards.first)
        XCTAssertEqual(card.plan?.gate, "Gate · Stale, 29 Sep")
        row["gate_at"] = NSNull()
        let null = try FixtureWire.board([row])
        XCTAssertEqual(display(board: null, live: try FixtureWire.live(count: 1)).cards.first?.plan?.gate,
                       "Gate · Stale, No gate run recorded")
        let midnight = FixtureWire.calendar().startOfDay(for: now).addingTimeInterval(60)
        let west = FixtureWire.calendar(try XCTUnwrap(TimeZone(secondsFromGMT: -3_600)))
        XCTAssertEqual(ProjectPresentation.boardGateDate(.value(Int(midnight.timeIntervalSince1970)), calendar: west), "29 Sep")
    }

    func testProviderPresenceAndFunnelNeverProduceTaskProgress() throws {
        var row = FixtureWire.row(0)
        func plan() throws -> ProjectPresentation.BoardPlan {
            ProjectPresentation.boardPlan(try FixtureWire.decode(row, as: ProjectBoardRow.self))
        }
        XCTAssertNil(try plan().provider)
        XCTAssertEqual(try plan().progress?.percent, "50%")
        row["plan_provider"] = NSNull()
        XCTAssertEqual(try plan().provider, "Plan provider not recorded")
        XCTAssertNil(try plan().progress)
        for provider in ["checkbox", "registry"] {
            row["plan_provider"] = provider
            XCTAssertEqual(try plan().provider, provider)
            XCTAssertEqual(try plan().progress?.percent, "50%")
            XCTAssertNil(try plan().funnel)
        }
        row["plan_provider"] = "funnel"
        row["plan_counts"] = ["contacted": 4, "waiting_on_owner": 2]
        row["plan_weekly_count"] = 3
        let board = try FixtureWire.board([row])
        let card = try XCTUnwrap(display(board: board, live: FixtureWire.live(count: 1)).cards.first)
        XCTAssertNil(card.plan?.progress)
        XCTAssertEqual(card.plan?.provider, "funnel")
        XCTAssertEqual(card.plan?.funnel?.counts, "contacted 4 · waiting for you 2")
        XCTAssertEqual(card.plan?.funnel?.weeklyCount, "3 this week")
        let quiet = try XCTUnwrap(display(board: board).rows.first)
        XCTAssertEqual(quiet.plan, "Review the guide · Building")
        XCTAssertEqual(quiet.funnel, card.plan?.funnel)
        XCTAssertFalse(quiet.plan.contains("%"))
        XCTAssertFalse(quiet.plan.contains("tasks"))
    }

    func testFunnelCountsAndWeekKeepMissingNullEmptyAndZeroApart() {
        XCTAssertNil(ProjectPresentation.boardFunnelCounts(.missing))
        XCTAssertEqual(ProjectPresentation.boardFunnelCounts(.null), "No status counts recorded")
        XCTAssertEqual(ProjectPresentation.boardFunnelCounts(.value([:])), "No status counts recorded")
        XCTAssertEqual(ProjectPresentation.boardFunnelCounts(.value(["waiting_on_owner": 0, "contacted": 4])),
                       "contacted 4 · waiting for you 0")
        XCTAssertNil(ProjectPresentation.boardFunnelWeek(.missing))
        XCTAssertEqual(ProjectPresentation.boardFunnelWeek(.null), "Weekly count not recorded")
        XCTAssertEqual(ProjectPresentation.boardFunnelWeek(.value(0)), "0 this week")
        XCTAssertEqual(ProjectPresentation.boardFunnelWeek(.value(3)), "3 this week")
    }

    func testFunnelFieldsOnlyBelongToAFunnel() throws {
        for provider in [nil, NSNull() as Any?, "registry" as Any?, "checkbox" as Any?] {
            var row = FixtureWire.row(0)
            row["plan_provider"] = provider
            row["plan_counts"] = ["contacted": 4]
            row["plan_weekly_count"] = 3
            let value = try FixtureWire.decode(row, as: ProjectBoardRow.self)
            XCTAssertNil(ProjectPresentation.boardPlan(value).funnel)
            XCTAssertNil(ProjectPresentation.boardRow(value, calendar: FixtureWire.calendar()).funnel)
        }
        var row = FixtureWire.row(0)
        row["plan_provider"] = "funnel"
        let value = try FixtureWire.decode(row, as: ProjectBoardRow.self)
        let plan = ProjectPresentation.boardPlan(value)
        XCTAssertNotNil(plan.funnel)
        XCTAssertNil(plan.funnel?.counts)
        XCTAssertNil(plan.funnel?.weeklyCount)
        XCTAssertNil(plan.progress)
        row["plan_counts"] = NSNull()
        row["plan_weekly_count"] = NSNull()
        let absent = ProjectPresentation.boardPlan(try FixtureWire.decode(row, as: ProjectBoardRow.self))
        XCTAssertEqual(absent.funnel?.counts, "No status counts recorded")
        XCTAssertEqual(absent.funnel?.weeklyCount, "Weekly count not recorded")
        XCTAssertNil(absent.progress)
    }

    func testFooterPresenceZeroSeveralWaitingItemsAndUndatedClosure() throws {
        XCTAssertNil(ProjectPresentation.boardOpenUnits(.missing))
        XCTAssertEqual(ProjectPresentation.boardOpenUnits(.null), "Open units not recorded")
        XCTAssertEqual(ProjectPresentation.boardOpenUnits(.value(0)), "0 open units")
        XCTAssertEqual(ProjectPresentation.boardOpenUnits(.value(1)), "1 open unit")
        XCTAssertEqual(ProjectPresentation.boardOpenUnits(.value(3)), "3 open units")
        XCTAssertNil(ProjectPresentation.boardWaiting(.missing))
        XCTAssertEqual(ProjectPresentation.boardWaiting(.null), "Nothing recorded as waiting for you")
        XCTAssertEqual(ProjectPresentation.boardWaiting(.value([])), "Nothing recorded as waiting for you")
        let long = Array(repeating: "Check the local cache", count: 8).joined(separator: " ")
        XCTAssertEqual(ProjectPresentation.boardWaiting(.value([long])), "Waiting for you: " + long)
        XCTAssertEqual(ProjectPresentation.boardWaiting(.value(["Review the guide", long])),
                       "2 items wait for you: Review the guide; " + long)
        let calendar = FixtureWire.calendar()
        XCTAssertNil(ProjectPresentation.boardLastClosed(.missing, calendar: calendar))
        XCTAssertEqual(ProjectPresentation.boardLastClosed(.null, calendar: calendar), "No closed unit recorded")
        var closed: FixtureWire.Object = ["title": "Draft the guide", "closed_at": NSNull()]
        let undated = try FixtureWire.decode(closed, as: ProjectLastClosed.self)
        XCTAssertEqual(ProjectPresentation.boardLastClosed(.value(undated), calendar: calendar), "Draft the guide · Closed")
        closed["closed_at"] = Int(now.timeIntervalSince1970) - 86_400
        let dated = try FixtureWire.decode(closed, as: ProjectLastClosed.self)
        XCTAssertEqual(ProjectPresentation.boardLastClosed(.value(dated), calendar: calendar), "Draft the guide · Closed 29 Sep")
        var row = FixtureWire.row(0)
        let legacy = try FixtureWire.board([row])
        XCTAssertNil(display(board: legacy, live: try FixtureWire.live(count: 1)).cards.first?.footer)
        row["open_units"] = 0
        row["waiting_on_owner"] = ["Review the guide", long]
        row["last_closed"] = ["title": "Draft the guide", "closed_at": NSNull()]
        let board = try FixtureWire.board([row])
        let footer = try XCTUnwrap(display(board: board, live: FixtureWire.live(count: 1)).cards.first?.footer)
        XCTAssertEqual(footer.openUnits, "0 open units")
        XCTAssertEqual(footer.waiting, "2 items wait for you: Review the guide; " + long)
        XCTAssertEqual(footer.lastClosed, "Draft the guide · Closed")
        XCTAssertEqual(footer.text, "2 items wait for you: Review the guide; " + long)
        row["open_units"] = NSNull()
        row["waiting_on_owner"] = NSNull()
        row["last_closed"] = NSNull()
        let absent = try FixtureWire.board([row])
        XCTAssertEqual(display(board: absent, live: try FixtureWire.live(count: 1)).cards.first?.footer?.text,
                       "Nothing recorded as waiting for you")
    }

    func testDaemonBoardFixtureDrivesTheNewPresentationFields() throws {
        let path = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/project-board.json")
        let board = try JSONDecoder().decode(ProjectBoard.self, from: Data(contentsOf: path))
        let row = try XCTUnwrap(board.projects.first)
        let plan = ProjectPresentation.boardPlan(row, calendar: FixtureWire.calendar())
        XCTAssertEqual(display(board: board).week, .init(owner: "~25m", agents: "1h"))
        XCTAssertEqual(plan.provider, "checkbox")
        XCTAssertEqual(plan.gate, "Gate · No run recorded")
        XCTAssertEqual(plan.progress?.percent, "50%")
        XCTAssertNil(plan.funnel)
        let footer = try XCTUnwrap(ProjectPresentation.boardFooter(row, calendar: FixtureWire.calendar()))
        XCTAssertEqual(footer.openUnits, "1 open unit")
        XCTAssertEqual(footer.waiting, "Nothing recorded as waiting for you")
        XCTAssertEqual(footer.lastClosed, "No closed unit recorded")
    }
}
