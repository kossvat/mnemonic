import Combine
import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicApp

/// The open window at rest (MN-42-11): the timer's refresh of a shown screen
/// that brings nothing new publishes nothing, so nothing is drawn again.
@MainActor
final class IdleRefreshTests: XCTestCase {
    /// Answers the first read at once and holds every later one until released.
    private actor Held<Answer: Sendable> {
        private let first: Answer
        private var calls = 0
        private var waiting: CheckedContinuation<Answer, Never>?
        private var onHeld: (@Sendable () -> Void)?
        private var spare: Answer?

        init(_ first: Answer) { self.first = first }

        func ask() async -> Answer {
            calls += 1
            if calls == 1 { return first }
            if let spare { return spare }
            return await withCheckedContinuation { continuation in
                waiting = continuation
                onHeld?()
                onHeld = nil
            }
        }

        func whenHeld(_ notify: @escaping @Sendable () -> Void) {
            if waiting != nil { notify() } else { onHeld = notify }
        }

        func release(_ answer: Answer) {
            waiting?.resume(returning: answer)
            waiting = nil
        }

        /// Teardown: lets every read held now or asked later finish.
        func releaseAll(_ answer: Answer) {
            spare = answer
            release(answer)
        }
    }

    private func renamed<Value: Codable>(_ value: Value, row: Bool) throws -> Value {
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(value)) as? [String: Any])
        if row {
            var rows = try XCTUnwrap(object["projects"] as? [[String: Any]])
            rows[0]["name"] = "Renamed"
            object["projects"] = rows
        } else {
            object["name"] = "Renamed"
        }
        return try JSONDecoder().decode(Value.self, from: JSONSerialization.data(withJSONObject: object))
    }

    private func held<Answer: Sendable>(_ reads: Held<Answer>) async {
        let arrived = expectation(description: "The read is held")
        await reads.whenHeld { arrived.fulfill() }
        await fulfillment(of: [arrived], timeout: 5)
    }

    /// A test that throws midway still lets its held reads and tasks end.
    private func finally<Answer: Sendable>(_ reads: Held<Answer>, _ answer: Answer, _ tasks: [Task<Void, Never>],
                                           _ controller: AppProjectController) {
        addTeardownBlock { @MainActor in
            controller.onPresentationChange = nil
            controller.hide()
            await reads.releaseAll(answer)
            for task in tasks { await task.value }
        }
    }

    private func presented(_ controller: AppProjectController, _ what: String) -> XCTestExpectation {
        let shown = expectation(description: what)
        controller.onPresentationChange = {
            controller.onPresentationChange = nil
            shown.fulfill()
        }
        return shown
    }

    func testAnUnchangedRefreshWhileTheLabelClockTicksPublishesNothing() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        var raised = false
        let loading = controller.$loading.dropFirst().sink { value in if value { raised = true } }
        await controller.setVisible(true)
        XCTAssertTrue(raised, "The first read of the board shows that it loads")
        XCTAssertNotNil(controller.board)
        XCTAssertTrue(controller.coordinator.labelsArmed)
        raised = false
        let opened = controller.now
        clock.now = clock.now.addingTimeInterval(30)
        var changes = 0
        let watched = controller.objectWillChange.sink { changes += 1 }
        var shown = 0
        let live = controller.live.objectWillChange.sink { shown += 1 }
        await controller.refresh(quiet: true)
        await controller.coordinator.request()
        watched.cancel()
        live.cancel()
        XCTAssertEqual(changes, 0)
        XCTAssertEqual(shown, 0)
        XCTAssertFalse(raised, "The timer's refresh of a shown board does not show loading")
        XCTAssertEqual(controller.now, opened, "While it ticks, the label clock alone advances the labels")
        await controller.refresh()
        XCTAssertTrue(raised, "A refresh asked for shows that it loads")
        XCTAssertEqual(controller.now, clock.now)
        loading.cancel()
        let asked = await source.counts
        XCTAssertEqual(asked, ProjectTestCounts(live: 2, board: 3))
        controller.hide()
    }

    func testTheTimersTickIsQuietWhicheverOfItsTwoAnswersArrivesFirst() async throws {
        for boardFirst in [false, true] {
            let source = ProjectTestSource(try ProjectTestFixtures())
            let clock = ProjectTestClock()
            let controller = projectTestController(source, clock: clock)
            await controller.setVisible(true)
            var changes = 0
            let watched = controller.objectWillChange.sink { changes += 1 }
            await source.holdNext(.board)
            await source.holdNext(.live)
            try clock.queue(30)()
            for kind in [ProjectTestRequest.live, .board] {
                let asked = expectation(description: "The tick asked for \(kind)")
                let notice = await source.notifyWhenRequested(kind, count: 2) { asked.fulfill() }
                await fulfillment(of: [asked], timeout: 5)
                await source.cancelNotice(notice)
            }
            for releaseBoard in boardFirst ? [true, false] : [false, true] {
                let shown = presented(controller, releaseBoard ? "The board's answer" : "The live answer")
                if releaseBoard { await source.finishBoard() } else { await source.finishLive() }
                await fulfillment(of: [shown], timeout: 5)
            }
            watched.cancel()
            XCTAssertEqual(changes, 0, "Board first: \(boardFirst)")
            XCTAssertEqual(clock.active, [30, 60, 300])
            controller.hide()
        }
    }

    func testTodaysTimelineFollowsMidnightWhenTheLiveAnswerThenFails() async throws {
        let fixture = try ProjectTestFixtures()
        let lives = Held<LiveAnswer>(.snapshot(fixture.live))
        let clock = ProjectTestClock()
        let midnight = Calendar.current.startOfDay(for: clock.now.addingTimeInterval(86_400))
        clock.now = midnight.addingTimeInterval(-20)
        let today = TimelineDay.today(now: clock.now)
        let controller = AppProjectController(clock: clock, date: { clock.now },
            ask: { await lives.ask() }, board: { .success(fixture.board) },
            detail: { _ in .success(fixture.detail) })
        controller.select(.timeline(today))
        await controller.setVisible(true)
        finally(lives, .failed(.undecodable), [], controller)
        XCTAssertTrue(controller.coordinator.labelsArmed)
        XCTAssertEqual(clock.active, [30, 60, 300])
        // The live answer is held, so the first presentation is the quiet refresh's,
        // made while the label clock is still armed.
        let refreshed = presented(controller, "The tick's quiet refresh")
        clock.now = midnight.addingTimeInterval(10)
        try clock.queue(30)()
        await fulfillment(of: [refreshed], timeout: 5)
        XCTAssertTrue(controller.coordinator.labelsArmed)
        XCTAssertEqual(controller.now, clock.now, "The day moved although the label clock is armed")
        XCTAssertNotEqual(TimelineDay.today(now: controller.now), today)
        let failed = presented(controller, "The live answer's failure")
        await held(lives)
        await lives.release(.failed(.undecodable))
        await fulfillment(of: [failed], timeout: 5)
        XCTAssertEqual(controller.now, clock.now)
        XCTAssertFalse(controller.coordinator.labelsArmed)
        XCTAssertEqual(clock.active, [], "Yesterday is no longer refreshed and nothing is live")
        controller.hide()
    }

    func testAChangedReplacementReadIsDrawnAtTheTimeItArrived() async throws {
        let fixture = try ProjectTestFixtures()
        for page in [false, true] {
            let boards = Held<Result<ProjectBoard, LocalAPIFailure>>(.success(fixture.board))
            let details = Held<Result<ProjectDetail, LocalAPIFailure>>(.success(fixture.detail))
            let clock = ProjectTestClock()
            let live = fixture.live
            let controller = AppProjectController(clock: clock, date: { clock.now },
                ask: { .snapshot(live) }, board: { await boards.ask() },
                detail: { _ in await details.ask() })
            if page { controller.select(.project(fixture.detail.key)) }
            await controller.setVisible(true)
            let refreshing = Task { await controller.refresh(quiet: true) }
            finally(boards, .failure(.undecodable), [refreshing], controller)
            finally(details, .failure(.undecodable), [], controller)
            if page { await held(details) } else { await held(boards) }
            // A refresh asked for takes the running read back; its answer is dropped.
            await controller.refresh()
            if page { await details.release(.success(fixture.detail)) } else { await boards.release(.success(fixture.board)) }
            if page { await held(details) } else { await held(boards) }
            clock.now = clock.now.addingTimeInterval(60)
            try clock.queue(60)()
            clock.now = clock.now.addingTimeInterval(1)
            if page {
                let changed = try renamed(fixture.detail, row: false)
                await details.release(.success(changed))
                await refreshing.value
                XCTAssertEqual(controller.detail, changed)
            } else {
                let changed = try renamed(fixture.board, row: true)
                await boards.release(.success(changed))
                await refreshing.value
                XCTAssertEqual(controller.board, changed)
            }
            XCTAssertEqual(controller.now, clock.now, "Page: \(page)")
            controller.hide()
        }
    }

    func testAChangedBoardHeldAcrossALabelTickIsDrawnAtTheTimeItArrived() async throws {
        let fixture = try ProjectTestFixtures()
        let reads = Held<Result<ProjectBoard, LocalAPIFailure>>(.success(fixture.board))
        let clock = ProjectTestClock()
        let live = fixture.live
        let controller = AppProjectController(clock: clock, date: { clock.now },
            ask: { .snapshot(live) }, board: { await reads.ask() },
            detail: { _ in .success(fixture.detail) })
        await controller.setVisible(true)
        let refreshing = Task { await controller.refresh(quiet: true) }
        finally(reads, .failure(.undecodable), [refreshing], controller)
        await held(reads)
        clock.now = clock.now.addingTimeInterval(60)
        try clock.queue(60)()
        XCTAssertEqual(controller.now, clock.now)
        clock.now = clock.now.addingTimeInterval(1)
        let changed = try renamed(fixture.board, row: true)
        await reads.release(.success(changed))
        await refreshing.value
        XCTAssertEqual(controller.board, changed)
        XCTAssertEqual(controller.now, clock.now, "Not the time the read was asked for")
        controller.hide()
    }

    func testAChangedPageHeldAcrossALabelTickIsDrawnAtTheTimeItArrived() async throws {
        let fixture = try ProjectTestFixtures()
        let reads = Held<Result<ProjectDetail, LocalAPIFailure>>(.success(fixture.detail))
        let clock = ProjectTestClock()
        let live = fixture.live
        let controller = AppProjectController(clock: clock, date: { clock.now },
            ask: { .snapshot(live) }, board: { .success(fixture.board) },
            detail: { _ in await reads.ask() })
        controller.select(.project(fixture.detail.key))
        await controller.setVisible(true)
        XCTAssertEqual(controller.detail, fixture.detail)
        let refreshing = Task { await controller.refresh(quiet: true) }
        finally(reads, .failure(.undecodable), [refreshing], controller)
        await held(reads)
        clock.now = clock.now.addingTimeInterval(60)
        try clock.queue(60)()
        clock.now = clock.now.addingTimeInterval(1)
        let changed = try renamed(fixture.detail, row: false)
        await reads.release(.success(changed))
        await refreshing.value
        XCTAssertEqual(controller.detail, changed)
        XCTAssertEqual(controller.now, clock.now)
        controller.hide()
    }

    func testAQuietRefreshPublishesAFailureAndItsRecoveryOnceEach() async throws {
        let fixture = try ProjectTestFixtures()
        let reads = Held<Result<ProjectBoard, LocalAPIFailure>>(.success(fixture.board))
        let clock = ProjectTestClock()
        let live = fixture.live
        let controller = AppProjectController(clock: clock, date: { clock.now },
            ask: { .snapshot(live) }, board: { await reads.ask() },
            detail: { _ in .success(fixture.detail) })
        await controller.setVisible(true)
        finally(reads, .failure(.undecodable), [], controller)
        var failures: [LocalAPIFailure?] = []
        let watched = controller.$failure.dropFirst().sink { value in failures.append(value) }
        let answers: [Result<ProjectBoard, LocalAPIFailure>] = [
            .failure(.undecodable), .failure(.undecodable), .success(fixture.board), .success(fixture.board)]
        for answer in answers {
            let refreshing = Task { await controller.refresh(quiet: true) }
            await held(reads)
            await reads.release(answer)
            await refreshing.value
        }
        watched.cancel()
        XCTAssertEqual(failures, [.undecodable, nil])
        XCTAssertEqual(controller.board, fixture.board)
        controller.hide()
    }

    func testATickOfTheLabelClockDrawsTheRouteAloneNotTheWindowsFrame() throws {
        let fixture = try TimelineIntegrationFixture(today: "2026-06-18")
        defer { fixture.close() }
        let model = fixture.model
        model.projects.present(board: nil, detail: nil, live: .snapshot(try FixtureWire.live()), route: model.route)
        // The root view observes the model alone; the route's screen observes the controllers.
        let observed = Mirror(reflecting: ContentView(model: model)).children.map { child in
            String(describing: type(of: child.value))
        }
        XCTAssertEqual(observed.filter { name in name.hasPrefix("ObservedObject<") }, ["ObservedObject<MnemonicAppModel>"])
        var frame = 0, route = 0
        let watchedFrame = model.objectWillChange.sink { frame += 1 }
        let watchedRoute = model.projects.objectWillChange.sink { route += 1 }
        fixture.clock.now = fixture.clock.now.addingTimeInterval(60)
        model.projects.coordinator.onLabelsTick?()
        watchedFrame.cancel()
        watchedRoute.cancel()
        XCTAssertEqual(route, 1, "The tick moves the clock the route draws")
        XCTAssertEqual(frame, 0, "Nothing the frame, sidebar or toolbar observes changed")
    }

    func testAnUnchangedRefreshOfTodayDoesNotRepublishTheDay() async {
        let fixture = TimelineControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        XCTAssertNotNil(fixture.controller.snapshot)
        var changes = 0
        let watched = fixture.controller.$snapshot.dropFirst().sink { _ in changes += 1 }
        let failed = fixture.controller.$failure.dropFirst().sink { _ in changes += 1 }
        await fixture.controller.refresh()
        watched.cancel()
        failed.cancel()
        XCTAssertEqual(changes, 0, "An equal day and no failure are not published again")
        let count = await fixture.reads.count
        XCTAssertEqual(count, 2, "Today is still read again")
    }
}
