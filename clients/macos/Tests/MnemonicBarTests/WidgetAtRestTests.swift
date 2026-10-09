import AppKit
import SwiftUI
import XCTest
import MnemonicShared
@testable import MnemonicBar

/// Hold background work and main-queue delivery independently, including
/// answers that were already in flight when a page disappeared.
private final class World {
    struct Ask: Equatable {
        let page: Int
        let command: Bool
    }

    var timerStarts = 0
    var waiting: [() -> Void] = []
    var deliveries: [() -> Void] = []
    var asked: [Ask] = []
    var monthAsked: [Bool] = []
    var next: WidgetData?
    var month: [WorkDay] = []

    func environment() -> WidgetEnvironment {
        WidgetEnvironment(
            every: { [unowned self] _, _ in
                self.timerStarts += 1
                return {}
            },
            background: { [unowned self] work in self.waiting.append(work) },
            main: { [unowned self] work in self.deliveries.append(work) },
            fetchPage: { [unowned self] page, command, _ in
                self.asked.append(Ask(page: page, command: command))
                return self.next
            },
            fetchMonth: { [unowned self] command in
                self.monthAsked.append(command)
                return self.month
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

private func data(memories: Int = 12, latest: String = "first") -> WidgetData {
    var data = WidgetData()
    data.source = .api
    data.isRunning = true
    data.hasData = true
    data.state = .idle
    data.memoriesTotal = memories
    data.workedTodaySeconds = 120
    data.latest = LatestMemory(
        id: latest, type: "note", title: latest, content: "", agoMinutes: 1)
    return data
}

private func day(_ date: String = "2026-03-08") -> WorkDay {
    WorkDay(date: date, seconds: 120, isToday: false,
            dowLetter: "S", dayNum: 8, dow: 0, label: "Sunday")
}

final class WidgetAtRestTests: XCTestCase {
    private var world: World!
    private var service: MnemonicService!

    override func setUp() {
        world = World()
        world.next = data()
        service = MnemonicService(environment: world.environment())
    }

    func testStartupClosedPagesAndLiveOpeningStartNoLegacyWorkOrTimer() {
        service.start()
        service.selectPage(1)
        service.refresh()
        XCTAssertTrue(world.waiting.isEmpty)
        service.selectPage(0)
        service.opened()
        service.opened()
        service.refresh()
        for page in [3, 4, 0] { service.selectPage(page) }
        XCTAssertTrue(world.waiting.isEmpty)
        XCTAssertEqual(world.timerStarts, 0)
        XCTAssertEqual(service.assignments, 0)
        service.closed()
        service.refresh()
        XCTAssertTrue(world.waiting.isEmpty)
    }

    func testOpeningAndSelectingTheSamePageAskOnce() {
        service.selectPage(1)
        service.opened()
        service.selectPage(1)
        service.opened()
        XCTAssertEqual(world.waiting.count, 1)
        world.answer()
        XCTAssertEqual(world.asked, [.init(page: 1, command: true)])
        XCTAssertEqual(service.data, world.next)
        XCTAssertEqual(service.assignments, 1)
        service.selectPage(1)
        XCTAssertTrue(world.waiting.isEmpty)
        service.refresh(command: false)
        world.answer()
        XCTAssertEqual(service.assignments, 1, "an equal answer was published again")
    }

    func testEachSecondaryAppearanceAsksAndPageOwnedLoadsStaySeparate() {
        service.opened()
        for page in [1, 2, 3, 4, 5, 0, 2] {
            service.selectPage(page)
            world.answer()
        }
        XCTAssertEqual(world.asked.map(\.page), [1, 2, 5, 2])
        XCTAssertEqual(world.asked.map(\.command), [true, false, false, false])
        XCTAssertEqual(world.timerStarts, 0)
    }

    func testALateAnswerIsKeptWithoutPublishingAfterClose() {
        service.selectPage(1)
        service.opened()
        world.fetch()
        service.closed()
        world.deliver()
        XCTAssertEqual(service.backing, world.next)
        XCTAssertEqual(service.data, WidgetData())
        XCTAssertEqual(service.assignments, 0)
        service.opened()
        XCTAssertEqual(service.data, world.next)
        XCTAssertEqual(service.assignments, 1)
        world.answer()
        XCTAssertEqual(service.assignments, 1)
    }

    func testPageChangesCoalesceToTheLatestVisiblePage() {
        service.opened()
        service.selectPage(1)
        service.selectPage(2)
        service.selectPage(5)
        XCTAssertEqual(world.waiting.count, 1)
        world.answer()
        XCTAssertEqual(service.backing, world.next)
        XCTAssertEqual(service.assignments, 0, "an old page's answer was published")
        XCTAssertEqual(world.waiting.count, 1)
        world.next = data(memories: 20)
        world.answer()
        XCTAssertEqual(world.asked.map(\.page), [1, 5])
        XCTAssertEqual(service.data.memoriesTotal, 20)
        XCTAssertEqual(service.assignments, 1)
    }

    func testReturningToLiveOrClosingDropsAnOwedLegacyPage() {
        service.opened()
        service.selectPage(1)
        service.selectPage(2)
        service.selectPage(0)
        world.answer()
        XCTAssertTrue(world.waiting.isEmpty)
        XCTAssertEqual(service.assignments, 0)
        service.selectPage(5)
        let assigned = service.assignments
        service.selectPage(2)
        service.closed()
        world.answer()
        XCTAssertTrue(world.waiting.isEmpty)
        XCTAssertEqual(world.asked.map(\.page), [1, 5])
        XCTAssertEqual(service.assignments, assigned)
    }

    func testAnOldOpeningCannotConsumeTheNewOpeningsFallback() {
        service.selectPage(1)
        service.opened()
        service.closed()
        service.opened()
        // The previous opening's worker has not reached its source yet.
        world.answer()
        XCTAssertEqual(world.asked, [.init(page: 1, command: false)])
        XCTAssertEqual(service.assignments, 0)
        world.answer()
        XCTAssertEqual(world.asked, [
            .init(page: 1, command: false), .init(page: 1, command: true),
        ])
        XCTAssertEqual(service.data, world.next)
    }

    func testExplicitRefreshesCoalesceAndKeepTheirBoundedFallbackPermission() {
        service.selectPage(1)
        service.opened()
        service.refresh()
        service.refresh()
        XCTAssertEqual(world.waiting.count, 1)
        XCTAssertEqual(service.dropped, 2)
        world.answer()
        XCTAssertEqual(world.waiting.count, 1)
        world.answer()
        XCTAssertEqual(world.asked, [
            .init(page: 1, command: true), .init(page: 1, command: true),
        ])
        XCTAssertTrue(world.waiting.isEmpty)
    }

    func testLegacyRepliesNeverTellTheStatusTitle() {
        var titles = 0
        service.onTitle = { _ in titles += 1 }
        service.selectPage(1)
        service.opened()
        world.answer()
        world.next = data(memories: 99)
        service.refresh()
        world.answer()
        XCTAssertEqual(titles, 0)
    }

    func testRefreshNotificationsStayOnTheVisibleVisit() {
        var told: [Bool] = []
        let listening = service.refreshed.sink { command in told.append(command) }
        defer { listening.cancel() }
        service.selectPage(1)
        service.opened()
        world.answer()
        service.refresh()
        world.answer()
        XCTAssertEqual(told, [false, true])
        service.refresh()
        world.fetch()
        service.closed()
        world.deliver()
        XCTAssertEqual(told, [false, true])
    }

    func testLateMonthRepliesRemainBackingOnlyUntilWorkIsVisibleAgain() {
        service.selectPage(1)
        service.opened()
        world.answer()
        world.month = [day()]
        service.loadMonthIfNeeded()
        service.loadMonthIfNeeded()
        XCTAssertEqual(world.waiting.count, 1)
        world.fetch()
        let assigned = service.assignments
        service.closed()
        world.deliver()
        XCTAssertTrue(service.monthDays.isEmpty)
        XCTAssertEqual(service.assignments, assigned)
        service.loadMonthIfNeeded()
        XCTAssertTrue(world.waiting.isEmpty)
        service.opened()
        XCTAssertEqual(service.monthDays, world.month)
        XCTAssertEqual(service.assignments, assigned + 1)
        XCTAssertEqual(world.monthAsked, [true])
    }

    func testANewMonthAppearanceIsNotLostBehindAnOldRequest() {
        service.selectPage(1)
        service.opened()
        world.answer()
        service.loadMonthIfNeeded()
        world.fetch()
        service.closed()
        service.opened()
        service.loadMonthIfNeeded()
        world.deliver()
        // One new page request and one month request for this appearance.
        XCTAssertEqual(world.waiting.count, 2)
        world.month = [day()]
        world.answer()
        XCTAssertEqual(world.monthAsked.count, 2)
        XCTAssertEqual(service.monthDays, world.month)
    }
}

final class WidgetPageFetchTests: XCTestCase {
    private func body(_ value: [String: Any]) -> Data {
        try! JSONSerialization.data(withJSONObject: value)
    }

    func testEachPageRequestsOnlyItsRoutesAndKeepsOtherPageData() throws {
        let fixture = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Fixtures/recall-day.json")
        let recall = try Data(contentsOf: fixture)
        var paths: [String] = []
        var commands = 0
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { work in work() }, main: { work in work() },
            http: { path in
                paths.append(path)
                if path == "/api/recall" { return (200, recall) }
                let value: [String: Any]
                switch path {
                case "/api/status": value = ["total_memories": 12]
                case "/api/memories?limit=12": value = ["results": []]
                case "/api/activity/projects": value = ["projects": []]
                default: value = ["worked_today": ["seconds": 120], "days": []]
                }
                return (200, self.body(value))
            },
            command: { _, _ in commands += 1; return "" }))
        service.opened()
        service.selectPage(1)
        XCTAssertEqual(paths, [
            "/api/activity/summary", "/api/status", "/api/memories?limit=12", "/api/recall",
        ])
        let recallState = service.data.recallToday
        paths = []
        service.selectPage(2)
        XCTAssertEqual(paths, ["/api/activity/summary", "/api/activity/projects"])
        XCTAssertEqual(service.data.recallToday, recallState)
        XCTAssertEqual(service.data.memoriesTotal, 12)
        paths = []
        service.selectPage(5)
        XCTAssertEqual(paths, ["/api/activity/summary", "/api/memories?limit=12"])
        XCTAssertEqual(commands, 0)
    }

    func testHealthyHTTPDoesNotSpendTheAutomaticFallbackAndPageLoadsShareIt() {
        var calls: [[String]] = []
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { work in work() }, main: { work in work() },
            http: { _ in (200, self.body(["projects": [], "days": []])) },
            command: { args, _ in calls.append(args); return "{}" }))
        service.selectPage(2)
        service.opened()
        XCTAssertTrue(calls.isEmpty)
        XCTAssertNotNil(service.cliObject(["first"], commands: service.automaticCommands()))
        XCTAssertNil(service.cliObject(["second"], commands: service.automaticCommands()))
        XCTAssertEqual(calls, [["first"]])
        service.closed()
        XCTAssertNil(service.cliObject(["closed"], commands: service.automaticCommands()))
        service.opened()
        XCTAssertNotNil(service.cliObject(["next-opening"], commands: service.automaticCommands()))
        XCTAssertEqual(calls, [["first"], ["next-opening"]])
    }

    func testJournalDeliveryRequiresTheSameVisibleAppearance() {
        var waiting: [() -> Void] = []
        var delivery: [() -> Void] = []
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { work in waiting.append(work) },
            main: { work in delivery.append(work) },
            http: { _ in (200, self.body(["day": "2026-03-08"])) }))
        service.selectPage(3)
        service.opened()
        var replies = 0
        service.loadJournal(day: "2026-03-08") { _ in replies += 1 }
        waiting.removeFirst()()
        service.closed()
        delivery.removeFirst()()
        XCTAssertEqual(replies, 0)
        service.opened()
        service.loadJournal(day: "2026-03-08") { _ in replies += 1 }
        waiting.removeFirst()()
        service.selectPage(0)
        delivery.removeFirst()()
        XCTAssertEqual(replies, 0)
        service.selectPage(3)
        service.loadJournal(day: "2026-03-08") { _ in replies += 1 }
        waiting.removeFirst()()
        delivery.removeFirst()()
        XCTAssertEqual(replies, 1)
    }

    func testDayDetailsDoNotCallAClosedOrReplacedView() {
        var waiting: [() -> Void] = []
        var delivery: [() -> Void] = []
        let service = MnemonicService(environment: WidgetEnvironment(
            fetch: { _, _ in nil },
            background: { work in waiting.append(work) },
            main: { work in delivery.append(work) },
            http: { _ in (200, self.body(["date": "2026-03-08", "total_seconds": 120])) }))
        service.selectPage(1)
        service.opened()
        waiting.removeFirst()()
        delivery.removeFirst()()
        var results = 0
        service.loadDayDetail(date: "2026-03-08") { _ in results += 1 }
        waiting.removeFirst()()
        service.closed()
        delivery.removeFirst()()
        XCTAssertEqual(results, 0)
        service.loadDayDetail(date: "2026-03-08") { _ in results += 1 }
        XCTAssertTrue(waiting.isEmpty)
    }

    func testMissingRoutesShareOneAutomaticCommandBudgetAcrossTheOpening() {
        var budgets: Set<ObjectIdentifier> = []
        var held: [CommandBudget] = []
        var commands = 0
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { work in work() }, main: { work in work() },
            http: { _ in nil },
            command: { args, budget in
                commands += 1
                budgets.insert(ObjectIdentifier(budget))
                held.append(budget)
                if args == ["status"] { return "is running" }
                return "{}"
            }))
        service.selectPage(1)
        service.opened()
        XCTAssertGreaterThan(commands, 1, "the page uses several bounded commands")
        XCTAssertEqual(budgets.count, 1)
        let first = commands
        service.selectPage(2)
        service.selectPage(5)
        XCTAssertEqual(commands, first, "another appearance reused automatic fallback")
        service.refresh()
        XCTAssertGreaterThan(commands, first)
        XCTAssertEqual(budgets.count, 2, "an explicit data action has its own deadline")
        XCTAssertFalse(held.isEmpty)
    }

    func testEmptyMemoryAnswersRemoveThePreviousLatestCard() {
        let service = MnemonicService(environment: WidgetEnvironment(
            background: { _ in }, main: { _ in }))
        var shown = data()
        service.applyLatestMemory(items: [], totalFallback: nil, to: &shown)
        XCTAssertNil(shown.latest)
        shown.sessionSeconds = 60
        service.applySummary(["session_seconds": NSNull()], to: &shown)
        XCTAssertNil(shown.sessionSeconds)
    }
}

final class WidgetDataTests: XCTestCase {
    func testDataFetchedTwiceFromTheSameAnswerIsEqual() {
        let first = BlockMin(startMin: 540, endMin: 600)
        let second = BlockMin(startMin: 540, endMin: 600)
        XCTAssertEqual(first, second)
        XCTAssertEqual(first.id, second.id)
        XCTAssertEqual(data(), data())
        XCTAssertNotEqual(data(), data(latest: "other"))
    }
}
