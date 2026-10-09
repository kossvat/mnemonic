import Foundation
import XCTest
@testable import MnemonicBar

private final class MonthWorld {
    var waiting: [() -> Void] = []
    var deliveries: [() -> Void] = []
    var days: [WorkDay] = []
    var asks = 0

    func environment() -> WidgetEnvironment {
        WidgetEnvironment(
            background: { [unowned self] work in self.waiting.append(work) },
            main: { [unowned self] work in self.deliveries.append(work) },
            fetchPage: { _, _, _ in nil },
            fetchMonth: { [unowned self] command in
                XCTAssertTrue(command)
                self.asks += 1
                return self.days
            })
    }

    func fetch() {
        let work = waiting
        waiting = []
        work.forEach { run in run() }
    }

    func deliver() {
        let work = deliveries
        deliveries = []
        work.forEach { run in run() }
    }

    func answer() {
        fetch()
        deliver()
    }
}

final class WidgetMonthRetryTests: XCTestCase {
    private func recoveredDay() -> WorkDay {
        WorkDay(date: "2026-03-08", seconds: 120, isToday: false,
                dowLetter: "S", dayNum: 8, dow: 0, label: "Sunday")
    }

    func testWeekToMonthRetriesAfterFailureWithinTheSameVisit() {
        let world = MonthWorld()
        let service = MnemonicService(environment: world.environment())
        service.selectPage(1)
        service.opened()
        world.answer()
        let visit = service.pageVisit

        service.loadMonthIfNeeded()
        service.loadMonthIfNeeded()
        XCTAssertEqual(world.waiting.count, 1)
        world.fetch()
        service.loadMonthIfNeeded()
        XCTAssertTrue(world.waiting.isEmpty, "an undelivered result was duplicated")
        world.deliver()
        XCTAssertTrue(service.monthDays.isEmpty)
        XCTAssertEqual(world.asks, 1)
        XCTAssertTrue(world.waiting.isEmpty, "a failed load retried without a selection")

        // The backend recovers before the user selects Week, then Month.
        world.days = [recoveredDay()]
        service.loadMonthIfNeeded()
        service.loadMonthIfNeeded()
        XCTAssertEqual(world.waiting.count, 1)
        world.answer()
        XCTAssertEqual(service.pageVisit, visit)
        XCTAssertEqual(service.monthDays, world.days)
        XCTAssertEqual(world.asks, 2)
        service.loadMonthIfNeeded()
        XCTAssertTrue(world.waiting.isEmpty, "a successful visit was fetched again")
    }

    func testAnOldFailedReplyPreservesThePendingNewVisitAndCoalescesItsLoads() {
        let world = MonthWorld()
        let service = MnemonicService(environment: world.environment())
        service.selectPage(1)
        service.opened()
        world.answer()
        service.loadMonthIfNeeded()
        world.fetch()
        service.closed()
        service.opened()
        service.loadMonthIfNeeded()
        service.loadMonthIfNeeded()
        XCTAssertEqual(world.waiting.count, 1, "the old month load is still in flight")
        let assigned = service.assignments

        world.deliver()
        XCTAssertEqual(service.assignments, assigned, "an old visit published a reply")
        XCTAssertEqual(world.waiting.count, 2, "the newer visit lost its month request")
        world.days = [recoveredDay()]
        world.answer()
        XCTAssertEqual(world.asks, 2)
        XCTAssertEqual(service.monthDays, world.days)
        XCTAssertTrue(world.waiting.isEmpty)
        service.loadMonthIfNeeded()
        XCTAssertTrue(world.waiting.isEmpty)
    }
}
