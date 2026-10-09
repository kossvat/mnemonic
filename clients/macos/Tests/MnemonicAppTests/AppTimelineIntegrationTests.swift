import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class AppTimelineIntegrationTests: XCTestCase {
    @MainActor
    private func requested(_ fixture: TimelineIntegrationFixture, _ count: Int) async {
        let notice = expectation(description: "Timeline read \(count) arrived")
        await fixture.reads.requested(count) { notice.fulfill() }
        await fulfillment(of: [notice], timeout: 5)
    }

    @MainActor
    private func liveRequested(_ fixture: TimelineIntegrationFixture, _ count: Int) async {
        let notice = expectation(description: "Live request arrived")
        let id = await fixture.projectSource.notifyWhenRequested(.live, count: count) { notice.fulfill() }
        await fulfillment(of: [notice], timeout: 5)
        await fixture.projectSource.cancelNotice(id)
    }

    @MainActor
    private func count(_ fixture: TimelineIntegrationFixture, _ expected: Int,
                       file: StaticString = #filePath, line: UInt = #line) async {
        let actual = await fixture.reads.count
        XCTAssertEqual(actual, expected, file: file, line: line)
    }

    @MainActor
    func testHiddenTimelineMakesNoRequestAndClosingWindowRevokesOpeningResponse() async throws {
        let fixture = try TimelineIntegrationFixture()
        defer { fixture.close() }
        await fixture.model.refreshAll()
        await fixture.model.projects.signalled()
        await count(fixture, 0)
        XCTAssertTrue(fixture.clock.active.isEmpty)
        await fixture.reads.hold()
        fixture.model.window(ObjectIdentifier(fixture.window), visible: true)
        await requested(fixture, 1)
        XCTAssertTrue(fixture.model.timeline.visible)
        fixture.model.window(ObjectIdentifier(fixture.window), visible: false)
        XCTAssertFalse(fixture.model.timeline.visible)
        XCTAssertFalse(fixture.model.timeline.loading)
        XCTAssertTrue(fixture.clock.active.isEmpty)
        await fixture.reads.finishAll()
        await fixture.model.refreshAll()
        await fixture.model.projects.signalled()
        await count(fixture, 1)
        XCTAssertNil(fixture.model.timeline.snapshot)
    }

    @MainActor
    func testPastDaySharesCacheAcrossSignalsAndVisitsButManualRefreshInvalidates() async throws {
        let fixture = try TimelineIntegrationFixture(today: "2026-06-18")
        defer { fixture.close() }
        // A legacy setup error must not prevent the independent Timeline route.
        fixture.model.setupMessage = "Synthetic legacy read failure"
        await fixture.model.projects.setVisible(true)
        XCTAssertEqual(fixture.model.timeline.snapshot?.day, fixture.selectedDay)
        XCTAssertTrue(fixture.model.isOnline)
        XCTAssertFalse(fixture.clock.active.contains(30))
        await fixture.model.projects.signalled()
        await fixture.model.projects.signalled()
        await count(fixture, 1)
        fixture.model.route = .search
        fixture.model.route = .timeline(fixture.selectedDay)
        await fixture.model.projects.refresh(force: false)
        await count(fixture, 1)
        await fixture.model.refreshAll()
        await count(fixture, 2)
        XCTAssertEqual(fixture.model.timeline.snapshot?.day, fixture.selectedDay)
    }

    @MainActor
    func testSignalReplacesHeldTodayReadBeforeLiveAnswerWithoutAThirdRequest() async throws {
        let fixture = try TimelineIntegrationFixture()
        defer { fixture.close() }
        await fixture.model.projects.setVisible(true)
        await fixture.reads.hold()
        await fixture.reads.set(.success(try TimelineIntegrationFixture.snapshot(
            day: fixture.selectedDay, marker: "obsolete")))
        let reading = Task { await fixture.model.projects.refresh() }
        await requested(fixture, 2)
        await fixture.projectSource.holdNext(.live)
        let signalling = Task { await fixture.model.projects.signalled() }
        await liveRequested(fixture, 2)
        await fixture.reads.set(.success(try TimelineIntegrationFixture.snapshot(
            day: fixture.selectedDay, marker: "current")))
        await fixture.reads.finish(2)
        await requested(fixture, 3)
        XCTAssertEqual(fixture.model.timeline.snapshot?.segments.first?.id, "first")
        await fixture.reads.finishAll()
        await reading.value
        XCTAssertEqual(fixture.model.timeline.snapshot?.segments.first?.id, "current")
        await fixture.projectSource.finishLive()
        await signalling.value
        await count(fixture, 3)
        let peak = await fixture.reads.peakActive
        XCTAssertEqual(peak, 1)
    }

    @MainActor
    func testExistingRefreshTickInvalidatesHeldTimelineAndHidingDropsQueuedTick() async throws {
        let fixture = try TimelineIntegrationFixture()
        defer { fixture.close() }
        await fixture.model.projects.setVisible(true)
        await fixture.reads.hold()
        let reading = Task { await fixture.model.projects.refresh() }
        await requested(fixture, 2)
        await fixture.projectSource.holdNext(.live)
        try fixture.clock.queue(30)()
        await liveRequested(fixture, 2)
        await fixture.reads.set(.success(try TimelineIntegrationFixture.snapshot(
            day: fixture.selectedDay, marker: "after-tick")))
        await fixture.reads.finish(2)
        await requested(fixture, 3)
        await fixture.reads.finishAll()
        await reading.value
        let rearmed = expectation(description: "The existing refresh clock rearmed")
        fixture.clock.onArm = { seconds in if seconds == 30 { rearmed.fulfill() } }
        await fixture.projectSource.finishLive()
        await fulfillment(of: [rearmed], timeout: 5)
        fixture.clock.onArm = nil
        XCTAssertEqual(fixture.model.timeline.snapshot?.segments.first?.id, "after-tick")
        let queued = try fixture.clock.queue(30)
        fixture.model.window(ObjectIdentifier(fixture.window), visible: false)
        queued()
        await fixture.model.projects.signalled()
        await count(fixture, 3)
        XCTAssertTrue(fixture.clock.active.isEmpty)
    }

    @MainActor
    func testMidnightFinishesTheDayOnceAndDisarmsItsThirtySecondRefresh() async throws {
        let fixture = try TimelineIntegrationFixture()
        defer { fixture.close() }
        await fixture.model.projects.setVisible(true)
        XCTAssertTrue(fixture.clock.active.contains(30))
        fixture.clock.now = try XCTUnwrap(TimelineDay.date("2026-06-18")).addingTimeInterval(1)
        await fixture.model.projects.refresh()
        XCTAssertFalse(fixture.clock.active.contains(30))
        XCTAssertEqual(fixture.model.route, .timeline("2026-06-17"))
        await fixture.model.projects.signalled()
        await count(fixture, 2)
    }

    @MainActor
    func testMidnightSignalDuringHeldTodayReadFinishesThePastDayBeforeCaching() async throws {
        let fixture = try TimelineIntegrationFixture()
        defer { fixture.close() }
        await fixture.model.projects.setVisible(true)
        await fixture.reads.set(.success(try TimelineIntegrationFixture.snapshot(
            day: fixture.selectedDay, marker: "partial")))
        await fixture.reads.hold()
        let reading = Task { await fixture.model.projects.refresh() }
        await requested(fixture, 2)
        fixture.clock.now = try XCTUnwrap(TimelineDay.date("2026-06-18")).addingTimeInterval(1)
        await fixture.model.projects.signalled()
        XCTAssertFalse(fixture.clock.active.contains(30))
        await fixture.reads.set(.success(try TimelineIntegrationFixture.snapshot(
            day: fixture.selectedDay, marker: "complete")))
        await fixture.reads.finish(2)
        await requested(fixture, 3)
        XCTAssertEqual(fixture.model.timeline.snapshot?.segments.first?.id, "first")
        await fixture.reads.finishAll()
        await reading.value
        XCTAssertEqual(fixture.model.timeline.snapshot?.segments.first?.id, "complete")
        await fixture.model.projects.signalled()
        await count(fixture, 3)
    }

    func testTimelineRoutesCarryValidDaysAndSidebarTargetsToday() throws {
        let route = try XCTUnwrap(AppRoute(rawValue: "timeline/2024-02-29"))
        XCTAssertEqual(route, .timeline("2024-02-29"))
        XCTAssertEqual(route.rawValue, "timeline/2024-02-29")
        XCTAssertNil(AppRoute(rawValue: "timeline/2025-02-29"))
        XCTAssertNil(AppRoute(rawValue: "timeline/2026-06-17/extra"))
        XCTAssertEqual(route.sidebarRoute, .timeline(TimelineDay.today()))
        XCTAssertEqual(AppRoute.allCases.count, 9)
        let current = try XCTUnwrap(TimelineDay.date("2026-06-17")).addingTimeInterval(100)
        XCTAssertTrue(AppRoute.timeline("2026-06-17").refreshes(at: current))
        XCTAssertFalse(AppRoute.timeline("2026-06-16").refreshes(at: current))
    }
}
