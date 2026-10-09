import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicBar

final class WidgetServiceRecoveryTests: XCTestCase {
    private func previousRecall() throws -> WidgetData {
        let fixture = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Fixtures/recall-day.json")
        let summary = try XCTUnwrap(RecallDaySummary.decode(Data(contentsOf: fixture)))
        var last = WidgetData()
        last.source = .api
        last.isRunning = true
        last.state = .idle
        last.workedTodaySeconds = 120
        last.recallToday = RecallDayState(summary)
        return last
    }

    func testOnlyDaemonActionsNotifyOnMainDeliveryIncludingAfterClose() {
        var waiting: [() -> Void] = []
        var deliveries: [() -> Void] = []
        var completed: [[String]] = []
        var notified: [[String]] = []
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { work in waiting.append(work) },
            main: { work in deliveries.append(work) },
            http: { _ in XCTFail("a closed action fetched page data"); return nil },
            command: { args, budget in
                completed.append(args)
                XCTAssertLessThanOrEqual(budget.remaining, MnemonicService.actionLimit)
                return ""
            }))
        service.onDaemonAction = { notified.append(completed.last!) }
        service.opened()
        service.startDaemon()
        XCTAssertTrue(completed.isEmpty)
        waiting.removeFirst()()
        XCTAssertEqual(completed, [["start", "-d"]])
        XCTAssertTrue(notified.isEmpty, "the action notified from its background worker")
        service.closed()
        deliveries.removeFirst()()
        XCTAssertEqual(notified, [["start", "-d"]])

        service.stopDaemon()
        waiting.removeFirst()()
        XCTAssertEqual(notified.count, 1)
        deliveries.removeFirst()()
        XCTAssertEqual(notified, [["start", "-d"], ["stop"]])
        XCTAssertEqual(service.assignments, 0)

        service.addMemory(title: "Fixture note", type: "note")
        service.generateContext()
        while !waiting.isEmpty { waiting.removeFirst()() }
        while !deliveries.isEmpty { deliveries.removeFirst()() }
        XCTAssertEqual(completed.count, 4)
        XCTAssertEqual(notified.count, 2, "a non-daemon action invalidated live status")
    }

    func testStatusRunsBeforeOtherFallbackReadsShareTheDeadline() throws {
        var calls: [[String]] = []
        var budgets: [CommandBudget] = []
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { work in work() }, main: { work in work() },
            http: { _ in nil },
            command: { args, budget in
                calls.append(args)
                budgets.append(budget)
                if args == ["status"] {
                    // A late check represents the empty result after other
                    // data commands have consumed the refresh deadline.
                    return calls.count == 1 ? "mnemonic is running (PID 42)" : ""
                }
                return "{}"
            }))
        let fetched = try XCTUnwrap(service.fetchData(
            page: 2, commands: WidgetCommandAllowance(.explicit), last: WidgetData()))
        XCTAssertEqual(calls, [
            ["status"], ["activity", "summary"], ["activity", "projects", "--json"],
        ])
        XCTAssertTrue(fetched.isRunning)
        XCTAssertEqual(fetched.state, .empty)
        XCTAssertEqual(Set(budgets.map { budget in ObjectIdentifier(budget) }).count, 1)
        XCTAssertLessThanOrEqual(budgets.first!.remaining, MnemonicService.commandLimit)
    }

    func testAnUnknownStatusDoesNotTurnARunningOrHungDaemonIntoStopped() throws {
        for (status, wasHung) in [("", false), ("status timed out", false), ("", true)] {
            var last = try previousRecall()
            if wasHung {
                last.isRunning = false
                last.state = .broken
            }
            let service = MnemonicService(environment: WidgetEnvironment(
                background: { work in work() }, main: { work in work() },
                http: { _ in nil },
                command: { args, _ in args == ["status"] ? status : "{}" }))
            let fetched = try XCTUnwrap(service.fetchData(
                page: 2, commands: WidgetCommandAllowance(.explicit), last: last))
            XCTAssertEqual(fetched.isRunning, last.isRunning)
            XCTAssertEqual(fetched.state, wasHung ? .broken : .idle)
        }
    }

    func testDefinitiveStoppedStaleAndHungStatusReplaceThePreviousState() throws {
        let statuses: [(String, WorkState)] = [
            ("mnemonic is not running", .stopped),
            ("mnemonic has a stale PID file (PID 42 is dead)", .stopped),
            ("mnemonic is HUNG (PID 42 alive but API socket unresponsive)", .broken),
        ]
        for (status, expected) in statuses {
            let service = MnemonicService(environment: WidgetEnvironment(
                background: { work in work() }, main: { work in work() },
                http: { _ in nil },
                command: { args, _ in args == ["status"] ? status : "{}" }))
            let fetched = try XCTUnwrap(service.fetchData(
                page: 2, commands: WidgetCommandAllowance(.explicit), last: try previousRecall()))
            XCTAssertFalse(fetched.isRunning)
            XCTAssertEqual(fetched.state, expected)
        }
    }

    func testFailedOpeningAndExplicitRecallLoadsPublishUnavailable() throws {
        var calls: [[String]] = []
        var budgets: [CommandBudget] = []
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { work in work() }, main: { work in work() },
            http: { _ in nil },
            command: { args, budget in
                calls.append(args)
                budgets.append(budget)
                return ""
            }))
        let yesterday = try previousRecall()
        service.present(yesterday)
        service.selectPage(1)
        service.opened()
        XCTAssertEqual(service.data.recallToday, .unavailable)
        XCTAssertTrue(calls.contains(["recall", "--json"]))
        XCTAssertEqual(Set(budgets.map { budget in ObjectIdentifier(budget) }).count, 1)

        service.present(yesterday)
        service.refresh()
        XCTAssertEqual(service.data.recallToday, .unavailable)
        XCTAssertEqual(Set(budgets.map { budget in ObjectIdentifier(budget) }).count, 2)
    }

    func testHTTPOnlyAndSpentFallbackFailuresClearOldRecallWithoutCommands() throws {
        let fallback = WidgetAutomaticFallback()
        let opening = fallback.begin()
        XCTAssertNotNil(fallback.claim(opening: opening))
        let modes: [WidgetCommands] = [.none, .automatic(fallback, opening)]
        for mode in modes {
            var calls = 0
            let service = MnemonicService(environment: WidgetEnvironment(
                background: { work in work() }, main: { work in work() },
                http: { _ in nil },
                command: { _, _ in calls += 1; return "" }))
            let last = try previousRecall()
            let fetched = try XCTUnwrap(service.fetchData(
                page: 1, commands: WidgetCommandAllowance(mode), last: last))
            XCTAssertEqual(fetched.recallToday, .unavailable)
            XCTAssertEqual(calls, 0)
            XCTAssertEqual(fetched.source, last.source)
            XCTAssertEqual(fetched.isRunning, last.isRunning)
        }
    }
}
