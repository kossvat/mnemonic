import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class ProjectPagePresentationTests: XCTestCase {
    private let now = FixtureWire.now
    private let calendar = FixtureWire.calendar()

    private func unit(_ object: FixtureWire.Object) throws -> ProjectPlanUnit {
        try FixtureWire.decode(object, as: ProjectPlanUnit.self)
    }

    private func page(_ detail: ProjectDetail?, failure: LocalAPIFailure? = nil,
                      liveFailure: LocalAPIFailure? = nil, loading: Bool = false) -> ProjectPresentation.Page {
        ProjectPresentation.page(detail: detail, failure: failure, liveFailure: liveFailure,
                                 loading: loading, now: now, calendar: calendar)
    }

    func testFocusUsesTheBoardsFunctionAndRegistryKeepsItsOwnCount() throws {
        var registry = FixtureWire.unit(1, title: "Draft the guide", done: 3, total: 7, current: false)
        registry["provider"] = "registry"
        let detail = try FixtureWire.detail(units: [registry, FixtureWire.unit(2)])
        let display = page(detail)
        XCTAssertEqual(display.focus?.id, ProjectPresentation.focusUnit(detail.plan.units)?.id)
        XCTAssertEqual(display.focus?.id, "unit-2")
        XCTAssertEqual(display.registry, "registry 3 of 7 tasks")
        let board = try FixtureWire.board([FixtureWire.row(0)])
        XCTAssertEqual(display.focus?.title, ProjectPresentation.boardPlan(board.projects[0]).unit)
        XCTAssertEqual(page(try FixtureWire.detail(units: [registry])).focus?.id, "unit-1")
        XCTAssertNil(page(try FixtureWire.detail(units: [])).focus)
    }

    func testClosureMissingNullAndValuesNeverInferClosureFromStage() throws {
        var missingObject = FixtureWire.unit(stage: "done", current: false)
        missingObject["closed_at"] = Int(now.timeIntervalSince1970)
        let missing = try unit(missingObject)
        var nullObject = FixtureWire.unit(2)
        nullObject["closed"] = NSNull()
        let absent = try unit(nullObject)
        var openObject = FixtureWire.unit(3, stage: "done")
        openObject["closed"] = false
        var closedObject = FixtureWire.unit(4, stage: "review", current: false)
        closedObject["closed"] = true
        let open = try unit(openObject), closed = try unit(closedObject)
        let legacy = ProjectPresentation.pageUnits([missing, open, closed], calendar: calendar)
        XCTAssertEqual(legacy.heading, "Units")
        XCTAssertNil(legacy.openCount)
        XCTAssertNil(legacy.lastClosed)
        XCTAssertEqual(legacy.rows.map(\.id), [missing.id, open.id, closed.id])
        XCTAssertTrue(legacy.rows.allSatisfy { row in row.closure == nil })
        let recorded = ProjectPresentation.pageUnits([open, absent, closed], calendar: calendar)
        XCTAssertEqual(recorded.openCount, 1)
        XCTAssertEqual(recorded.rows.map(\.id), [open.id])
        XCTAssertEqual(recorded.unclassified.first?.closure, "Closure status not recorded")
        XCTAssertEqual(recorded.lastClosed?.id, closed.id)
        XCTAssertEqual(recorded.lastClosed?.stage.label, "Review")
    }

    func testTheTaskInFocusIsInProgressOnlyWhenThePlanSaysSo() {
        let started = ProjectPresentation.PageTask(id: "T02", title: "Check the local cache", status: "in_progress", owner: nil)
        let picked = ProjectPresentation.PageTask(id: "T02", title: "Check the local cache", status: "planned", owner: nil)
        XCTAssertEqual(ProjectPresentation.currentTaskLabel(started), "IN PROGRESS")
        XCTAssertEqual(ProjectPresentation.currentTaskLabel(picked), "CURRENT")
    }

    func testClosureTimeIsOmittedAbsentOrTheRecordedDay() throws {
        var object = FixtureWire.unit(stage: "review", current: false)
        object["closed"] = true
        func closure() throws -> String? {
            ProjectPresentation.pageUnits([try unit(object)], calendar: calendar).lastClosed?.closure
        }
        XCTAssertNil(try closure())
        object["closed_at"] = NSNull()
        XCTAssertEqual(try closure(), "No closure time recorded")
        object["closed_at"] = Int(now.timeIntervalSince1970) - 86_400
        XCTAssertEqual(try closure(), "Closed 29 Sep 2026")
    }

    func testStagesStayRecordedAndBlockedDoesNotInventAStage() throws {
        for (index, stage) in ["proposed", "planned", "building", "review", "done"].enumerated() {
            let display = ProjectPresentation.pageUnit(try unit(FixtureWire.unit(stage: stage)), calendar: calendar)
            XCTAssertEqual(display.stage.steps.map(\.label), ["Proposed", "Planned", "Building", "Review", "Done"])
            XCTAssertEqual(display.stage.steps.first { step in step.current }?.index, index)
            XCTAssertTrue(display.stage.flags.isEmpty)
        }
        let blocked = ProjectPresentation.pageUnit(try unit(FixtureWire.unit(stage: "blocked", stale: true)), calendar: calendar)
        XCTAssertEqual(blocked.stage.flags, ["Blocked", "Stale"])
        XCTAssertFalse(blocked.stage.steps.contains { step in step.current || step.complete })
        let review = ProjectPresentation.pageUnit(try unit(FixtureWire.unit(done: 2, stage: "review")), calendar: calendar)
        XCTAssertEqual(review.stage.label, "Review")
        XCTAssertEqual(review.progress?.percent, "100%")
        XCTAssertEqual(review.progress?.compactTasks, "2/2")
    }

    func testNoTotalAndFunnelHaveNoProgressAndNoInventedTasks() throws {
        let noTotal = ProjectPresentation.pageUnit(try unit(FixtureWire.unit(done: 0, total: 0)), calendar: calendar)
        XCTAssertNil(noTotal.progress)
        var object = FixtureWire.unit(done: 3, total: 5, tasks: [])
        object["provider"] = "funnel"
        object["counts"] = ["contacted": 3, "waiting_on_owner": 2]
        object["weekly_count"] = 4
        let display = ProjectPresentation.pageUnit(try unit(object), calendar: calendar)
        XCTAssertNil(display.progress)
        XCTAssertEqual(display.counts, ["contacted: 3", "waiting_on_owner: 2"])
        XCTAssertEqual(display.weekly, "4 this week")
        XCTAssertNil(display.tasks)
    }

    func testUnitRowsUseCompactCountsAndKeepTheStageForDetails() throws {
        for (done, total) in [(0, 4), (1, 4), (4, 4)] {
            let display = ProjectPresentation.pageUnit(try unit(FixtureWire.unit(done: done, total: total, stage: "blocked")),
                                                       calendar: calendar)
            XCTAssertEqual(display.progress?.compactTasks, "\(done)/\(total)")
            XCTAssertEqual(display.stage.label, "Blocked")
            XCTAssertEqual(display.stage.flags, ["Blocked"])
        }
    }

    func testFinishedTasksFoldAndQueueKeepsServedOrderIncludingNext() throws {
        let object = FixtureWire.unit(tasks: [
            FixtureWire.task(1, title: "Draft the guide", status: "done"),
            FixtureWire.task(2, title: "Check the local cache"),
            FixtureWire.task(4, title: "Verify the result"),
            FixtureWire.task(3, title: "Review the guide")
        ])
        let tasks = try XCTUnwrap(ProjectPresentation.pageTasks(try unit(object)))
        XCTAssertEqual(tasks.finished.map(\.id), ["T01"])
        XCTAssertEqual(tasks.foldedLabel, "1 completed task folded")
        XCTAssertEqual(tasks.current?.id, "T02")
        XCTAssertEqual(tasks.next?.id, "T04")
        XCTAssertEqual(tasks.queued.map(\.id), ["T04", "T03"])
        XCTAssertEqual(tasks.queueLabel, "2 queued")
        XCTAssertEqual(tasks.summary, "1 complete · 3 remaining")
    }

    func testStaleGateNeverPromotesTheResultAndDistinguishesPresence() throws {
        var object: FixtureWire.Object = ["status": "stale", "at": Int(now.timeIntervalSince1970), "failed_runs": 0]
        func gate() throws -> ProjectPresentation.PageGate {
            ProjectPresentation.pageGate(try FixtureWire.decode(object, as: ProjectGate.self), checkout: nil,
                                         calendar: calendar)
        }
        XCTAssertNil(try gate().result)
        XCTAssertNil(try gate().movement)
        object["result"] = NSNull()
        XCTAssertEqual(try gate().result, "Run result not recorded")
        object["result"] = "passed"
        XCTAssertEqual(try gate().status, "Stale")
        XCTAssertEqual(try gate().result, "Last run result: passed")
        XCTAssertEqual(try gate().day, "Last run 30 Sep 2026")
        object["result"] = "failed"
        XCTAssertEqual(try gate().result, "Last run result: failed")
        object["status"] = "none"
        XCTAssertEqual(try gate().status, "No gate recorded")
    }

    func testCheckoutKeepsDirtyUnknownAndZeroSeparate() throws {
        let gate = try unit(FixtureWire.unit()).gate
        let empty = ProjectPresentation.pageGate(gate, checkout: nil, calendar: calendar)
        XCTAssertNil(empty.movement)
        XCTAssertNil(empty.checkout)
        for (dirty, words) in [(true as Any, "Uncommitted changes"), (false as Any, "No uncommitted changes"),
                               (NSNull(), "Working tree unknown")] {
            let checkout = try FixtureWire.decode(["dirty": dirty, "commits_since": 0], as: ProjectGateDetail.self)
            let display = ProjectPresentation.pageGate(gate, checkout: checkout, calendar: calendar)
            XCTAssertEqual(display.checkout, words)
            XCTAssertEqual(display.movement, "0 commits since gate")
        }
    }

    func testPlanDetailsNameOnlyOtherCheckoutsWithDifferentCounts() throws {
        for (count, words) in [(0, nil as String?), (1, "1 other checkout differs"), (2, "2 other checkouts differ")] {
            var object = FixtureWire.unit()
            object["checkouts"] = count
            let detail = try FixtureWire.detail(units: [object])
            let display = page(detail)
            XCTAssertEqual(display.focus?.checkoutDifferences, words)
            XCTAssertEqual(display.units.rows.first?.checkoutDifferences, words)
            XCTAssertEqual(display.gate?.checkout, "No uncommitted changes")
            XCTAssertEqual(display.gate?.movement, "0 commits since gate")
            let summary = FixtureProjectSummary.summary(FixtureState(
                id: "checkout-differences", screen: .project, detail: detail, expected: ""))
            XCTAssertTrue(summary.contains("details-checkouts=\(words ?? "not-shown");"))
            XCTAssertTrue(summary.contains("gate=[Passed; day=Last run 30 Sep 2026; result=not-served; "
                + "movement=0 commits since gate; checkout=No uncommitted changes; failures=none]"))
            for closed in [false as Any, true as Any, NSNull()] {
                object["closed"] = closed
                let units = page(try FixtureWire.detail(units: [object])).units
                let rows = units.rows + units.unclassified + [units.lastClosed].compactMap { row in row }
                XCTAssertEqual(try XCTUnwrap(rows.first).checkoutDifferences, words)
            }
        }
    }

    func testHeaderShowsTodayAndLiveStateWithoutInventingTime() throws {
        let detail = try FixtureWire.detail(agents: ["claude", "codex"])
        XCTAssertEqual(page(detail).today, .init(owner: "~10m", agents: "20m"))
        XCTAssertEqual(page(detail).state, "Live")
        XCTAssertEqual(page(detail).sessionCount, "2 live sessions")
        XCTAssertEqual(page(try FixtureWire.detail(agents: [])).sessionsMessage, "No live sessions")
        XCTAssertEqual(page(try FixtureWire.detail(time: [])).todayMessage, "No time recorded for today")
        XCTAssertNil(page(try FixtureWire.detail(time: [])).today)
    }

    func testFailuresKeepRecordedWorkButNeverHeldSessionsOrTheirLiveBadge() throws {
        let detail = try FixtureWire.detail()
        for failure in [LocalAPIFailure.off, .unreachable, .unauthorised, .earlierDaemon, .undecodable] {
            for display in [page(detail, failure: failure), page(detail, liveFailure: failure)] {
                XCTAssertTrue(display.sessions.isEmpty)
                XCTAssertNil(display.sessionCount)
                XCTAssertEqual(display.state, "Active")
                XCTAssertNotNil(display.focus)
                XCTAssertEqual(display.recent.days.count, 14)
            }
            XCTAssertEqual(page(nil, failure: failure).failure?.title, ProjectPresentation.failure(failure).title)
            XCTAssertEqual(page(detail, liveFailure: failure).liveFailure?.detail, ProjectPresentation.failure(failure).detail)
        }
        let loading = page(nil, loading: true)
        XCTAssertNil(loading.today)
        XCTAssertNil(loading.sessionCount)
        XCTAssertNil(loading.focus)
        XCTAssertEqual(loading.planMessage, "Loading plan")
        XCTAssertEqual(loading.sessionsMessage, "Loading live sessions")
        XCTAssertEqual(loading.gateMessage, "Loading gate")
        XCTAssertEqual(loading.tasksMessage, "Loading tasks")
        XCTAssertEqual(page(nil, failure: .unreachable).gateMessage, "Gate unavailable")
    }

    func testTopTermsOmitTheBlockWhenMissingAndNameRecordedAbsence() throws {
        let missing = try FixtureWire.detail(terms: nil)
        XCTAssertNil(page(missing).terms)
        XCTAssertNil(page(missing).termsMessage)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(missing)) as? FixtureWire.Object)
        object["top_terms"] = NSNull()
        let absent = page(try FixtureWire.decode(object, as: ProjectDetail.self))
        XCTAssertEqual(absent.terms, [])
        XCTAssertEqual(absent.termsMessage, "No top terms recorded")
        XCTAssertEqual(page(try FixtureWire.detail(terms: [])).termsMessage, "No top terms recorded")
        XCTAssertEqual(page(try FixtureWire.detail()).terms, ["cache · 1", "guide · 1"])
    }
}
