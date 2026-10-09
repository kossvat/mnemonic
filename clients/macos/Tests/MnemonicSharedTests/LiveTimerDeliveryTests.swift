import Foundation
import MnemonicShared
import XCTest

/// A native timer can fire before cancellation while its main-actor
/// delivery is still queued. Keep those callbacks to exercise that order.
final class LiveTimerDeliveryTests: XCTestCase {
    @MainActor
    final class Clock: LiveClock {
        final class Ticket: LiveTimer {
            let seconds: TimeInterval
            let delivery: @MainActor () -> Void
            var queued = false
            var cancelled = false

            init(seconds: TimeInterval, delivery: @escaping @MainActor () -> Void) {
                self.seconds = seconds
                self.delivery = delivery
            }

            func cancel() { cancelled = true }
        }

        private var tickets: [Ticket] = []

        func arm(after seconds: TimeInterval, tolerance: TimeInterval,
                 _ fire: @escaping @MainActor () -> Void) -> LiveTimer {
            let ticket = Ticket(seconds: seconds, delivery: fire)
            tickets.append(ticket)
            return ticket
        }

        var live: [TimeInterval] {
            tickets.filter { ticket in !ticket.queued && !ticket.cancelled }
                .map(\.seconds).sorted()
        }

        func queue(_ seconds: TimeInterval) throws -> @MainActor () -> Void {
            let ticket = try XCTUnwrap(tickets.first { ticket in
                ticket.seconds == seconds && !ticket.queued && !ticket.cancelled
            })
            ticket.queued = true
            return ticket.delivery
        }
    }

    actor Answers {
        let snapshot: LiveSnapshot
        private(set) var count = 0

        init(_ snapshot: LiveSnapshot) { self.snapshot = snapshot }

        func next() -> LiveAnswer {
            count += 1
            return .snapshot(snapshot)
        }
    }

    private func snapshot() throws -> LiveSnapshot {
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/live.json")
        return try JSONDecoder().decode(LiveSnapshot.self, from: Data(contentsOf: file))
    }

    @MainActor
    private func settle() async {
        for _ in 0..<50 { await Task.yield() }
    }

    @MainActor
    func testQueuedRefreshCannotRequestAfterHide() async throws {
        let replies = Answers(try snapshot())
        let clock = Clock()
        let store = LiveStore()
        let coordinator = LiveCoordinator(store: store, clock: clock) { await replies.next() }
        await coordinator.setSurfaceVisible(true)
        coordinator.setPlanVisible(true)
        let queued = try clock.queue(30)
        coordinator.hide()
        let publications = store.publications
        queued()
        await settle()
        let count = await replies.count
        XCTAssertEqual(count, 1)
        XCTAssertEqual(store.publications, publications)
        XCTAssertEqual(clock.live, [300])
        XCTAssertEqual(coordinator.armed, ["safety"])
    }

    @MainActor
    func testOldDeliveriesCannotClearReplacementsAfterReopening() async throws {
        let replies = Answers(try snapshot())
        let clock = Clock()
        let store = LiveStore()
        let coordinator = LiveCoordinator(store: store, clock: clock) { await replies.next() }
        var labels = 0
        coordinator.onLabelsTick = { labels += 1 }
        await coordinator.setSurfaceVisible(true)
        coordinator.setPlanVisible(true)
        let oldRefresh = try clock.queue(30)
        let oldLabels = try clock.queue(60)
        let oldSafety = try clock.queue(300)
        coordinator.hide()
        // A failure while closed also cancels the previous safety lease.
        store.take(.failed(.unreachable))
        coordinator.hide()
        await coordinator.setSurfaceVisible(true)
        coordinator.setPlanVisible(true)
        for callback in [oldRefresh, oldLabels, oldSafety] {
            callback()
            callback()
        }
        await settle()
        let count = await replies.count
        XCTAssertEqual(count, 2, "cancelled callbacks started another request")
        XCTAssertEqual(labels, 0)
        XCTAssertEqual(clock.live, [30, 60, 300])
        XCTAssertEqual(coordinator.armed, ["refresh", "labels", "safety"])
    }

    @MainActor
    func testRepeatedOneShotDeliveryCannotCreateDuplicateTimers() async throws {
        let replies = Answers(try snapshot())
        let clock = Clock()
        let coordinator = LiveCoordinator(store: LiveStore(), clock: clock) { await replies.next() }
        var labels = 0
        coordinator.onLabelsTick = { labels += 1 }
        await coordinator.setSurfaceVisible(true)
        coordinator.setPlanVisible(true)
        let refresh = try clock.queue(30)
        let tick = try clock.queue(60)
        for _ in 0..<3 { refresh(); tick() }
        await settle()
        let count = await replies.count
        XCTAssertEqual(count, 2)
        XCTAssertEqual(labels, 1)
        XCTAssertEqual(clock.live, [30, 60, 300])
        XCTAssertEqual(coordinator.armed, ["refresh", "labels", "safety"])
    }

    @MainActor
    func testHideBetweenDeliveryAndRequestTaskPreventsHiddenRefresh() async throws {
        let replies = Answers(try snapshot())
        let clock = Clock()
        let coordinator = LiveCoordinator(store: LiveStore(), clock: clock) { await replies.next() }
        await coordinator.setSurfaceVisible(true)
        coordinator.setPlanVisible(true)
        let refresh = try clock.queue(30)
        refresh()
        coordinator.hide()
        await settle()
        let count = await replies.count
        XCTAssertEqual(count, 1)
        XCTAssertEqual(clock.live, [300])
    }

    @MainActor
    func testFailedSnapshotBeforeSafetyRequestTaskCancelsItsRequest() async throws {
        let replies = Answers(try snapshot())
        let clock = Clock()
        let store = LiveStore()
        let coordinator = LiveCoordinator(store: store, clock: clock) { await replies.next() }
        await coordinator.request()
        let safety = try clock.queue(300)
        safety()
        store.take(.failed(.unreachable))
        coordinator.hide()
        await settle()
        let count = await replies.count
        XCTAssertEqual(count, 1)
        XCTAssertEqual(clock.live, [])
        XCTAssertEqual(coordinator.armed, [])
    }
}
