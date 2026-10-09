import Combine
import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicBar

final class WidgetRecordsTests: XCTestCase {
    func testDelayedRecordsFailureCannotSpendANewOpeningsFallbackOrPublish() {
        var waiting: [() -> Void] = []
        var delivery: [() -> Void] = []
        var commands = 0
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { work in waiting.append(work) },
            main: { work in delivery.append(work) },
            http: { _ in nil },
            command: { _, _ in commands += 1; return "{\"sessions\":[]}" }))
        service.selectPage(4)
        service.opened()
        var oldReplies = 0
        service.loadRecords(command: true) { _ in oldReplies += 1 }
        service.closed()
        service.opened()
        var newReplies = 0
        service.loadRecords(command: true) { _ in newReplies += 1 }

        // The old HTTP request fails after the next opening began.
        waiting.removeFirst()()
        delivery.removeFirst()()
        XCTAssertEqual(commands, 0)
        XCTAssertEqual(oldReplies, 0)
        waiting.removeFirst()()
        delivery.removeFirst()()
        XCTAssertEqual(commands, 1)
        XCTAssertEqual(newReplies, 1)

        service.loadRecords(command: false) { _ in newReplies += 1 }
        waiting.removeFirst()()
        service.closed()
        delivery.removeFirst()()
        XCTAssertEqual(newReplies, 1, "a closed Records view received a state update")
    }

    func testAnExplicitRecordsRefreshRetriesAfterAutomaticFallbackWasSpent() {
        var budgets: [CommandBudget] = []
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { work in work() }, main: { work in work() },
            http: { _ in nil },
            command: { _, budget in
                budgets.append(budget)
                return "{\"sessions\":[]}"
            }))
        service.selectPage(4)
        service.opened()
        service.loadRecords(command: true) { object in XCTAssertNotNil(object) }
        XCTAssertEqual(budgets.count, 1)
        service.loadRecords(command: true) { object in XCTAssertNil(object) }
        XCTAssertEqual(budgets.count, 1, "another appearance reused automatic fallback")

        var explicitReplies = 0
        let listening = service.refreshed.sink { command in
            service.loadRecords(command: command, automatic: false) { object in
                if command {
                    XCTAssertNotNil(object)
                    explicitReplies += 1
                } else {
                    XCTAssertNil(object)
                }
            }
        }
        defer { listening.cancel() }
        service.refresh()
        XCTAssertEqual(explicitReplies, 1)
        XCTAssertEqual(budgets.count, 2)
        XCTAssertFalse(budgets[0] === budgets[1])
        XCTAssertLessThanOrEqual(budgets[1].remaining, MnemonicService.commandLimit)
        service.refresh(command: false)
        XCTAssertEqual(budgets.count, 2, "an HTTP-only refresh started a command")
    }
}
