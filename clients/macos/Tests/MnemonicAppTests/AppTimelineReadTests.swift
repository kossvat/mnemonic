import Combine
import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

@MainActor
final class AppTimelineReadTests: XCTestCase {
    private func arrived(_ fixture: TimelineControllerFixture, _ count: Int) async {
        let expected = expectation(description: "Timeline read \(count) is held")
        await fixture.reads.requested(count) { expected.fulfill() }
        await fulfillment(of: [expected], timeout: 5)
    }

    func testContextDoesNotRequestAndPastDaysCacheAcrossRevisitsUntilExplicitRefresh() async {
        let fixture = TimelineControllerFixture(day: "2026-09-29")
        defer { fixture.close() }
        let before = await fixture.reads.count
        XCTAssertEqual(before, 0)
        await fixture.controller.refresh()
        await fixture.controller.refresh()
        fixture.controller.setContext(day: "2026-09-28", visible: true)
        await fixture.controller.refresh()
        fixture.controller.setContext(day: "2026-09-29", visible: true)
        XCTAssertEqual(fixture.controller.snapshot?.day, "2026-09-29")
        await fixture.controller.refresh()
        let cached = await fixture.reads.count
        XCTAssertEqual(cached, 2)
        await fixture.reads.set(.success(TimelineTestValues.snapshot("2026-09-29", version: 2)))
        fixture.controller.invalidateSelected()
        await fixture.controller.refresh()
        let refreshed = await fixture.reads.count
        XCTAssertEqual(refreshed, 3)
        XCTAssertEqual(fixture.controller.snapshot?.projects.first?.name, "Version 2")
    }

    func testTodayAlwaysRefreshesAndFutureResponseDoesNotBecomeHistoricalCache() async {
        let fixture = TimelineControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.controller.refresh()
        var count = await fixture.reads.count
        XCTAssertEqual(count, 2)
        fixture.controller.setContext(day: "2026-10-01", visible: true)
        await fixture.controller.refresh()
        fixture.clock.now = TimelineTestValues.date("2026-10-02")
        await fixture.controller.refresh()
        await fixture.controller.refresh()
        count = await fixture.reads.count
        XCTAssertEqual(count, 4, "The future response must be replaced once the day is historical")
    }

    func testTodayReadCrossingMidnightReplacesPartialReplyBeforePublishingAndThenCaches() async {
        let fixture = TimelineControllerFixture()
        defer { fixture.close() }
        var published: [String] = []
        let subscription = fixture.controller.$snapshot.sink { value in
            if let name = value?.projects.first?.name { published.append(name) }
        }
        defer { subscription.cancel() }
        await fixture.reads.hold()
        let reading = Task { await fixture.controller.refresh() }
        await arrived(fixture, 1)
        fixture.clock.now = TimelineTestValues.date("2026-10-01")
        await fixture.reads.set(.success(TimelineTestValues.snapshot("2026-09-30", version: 2)))
        await fixture.reads.finish(1)
        await arrived(fixture, 2)
        XCTAssertNil(fixture.controller.snapshot, "The clipped reply cannot publish after midnight")
        XCTAssertTrue(published.isEmpty)
        XCTAssertTrue(fixture.controller.loading)
        XCTAssertFalse(fixture.controller.invalidateRunningRead(), "Ordinary signals leave the past replacement alone")
        await fixture.controller.refresh()
        let heldCount = await fixture.reads.count
        XCTAssertEqual(heldCount, 2)
        await fixture.reads.finish(2)
        await reading.value
        XCTAssertEqual(published, ["Version 2"])
        XCTAssertFalse(fixture.controller.loading)
        await fixture.reads.finishAll()
        await fixture.controller.refresh()
        await fixture.controller.refresh()
        let count = await fixture.reads.count, peak = await fixture.reads.peakActive
        XCTAssertEqual(count, 2)
        XCTAssertEqual(peak, 1)
    }

    func testPastCacheWithDifferentLocalDayBoundsIsEvictedBeforeReuse() async {
        let fixture = TimelineControllerFixture(day: "2026-09-29")
        defer { fixture.close() }
        let original = TimelineTestValues.snapshot("2026-09-29")
        let shifted = ProjectTimeline(day: original.day, from: original.from + 3600,
                                      to: original.to + 3600, projects: original.projects,
                                      segments: [], owner: [], ownerAvailable: true)
        await fixture.reads.set(.success(shifted))
        await fixture.controller.refresh()
        await fixture.reads.set(.success(original))
        fixture.controller.setContext(day: nil, visible: false)
        fixture.controller.setContext(day: original.day, visible: true)
        XCTAssertNil(fixture.controller.snapshot)
        await fixture.controller.refresh()
        let count = await fixture.reads.count
        XCTAssertEqual(count, 2)
        XCTAssertEqual(fixture.controller.snapshot, original)
    }

    func testInvalidationsSuppressOldSuccessAndFailureAndKeepSerialReplacementInvalidations() async {
        for failure in [false, true] {
            let fixture = TimelineControllerFixture()
            defer { fixture.close() }
            await fixture.controller.refresh()
            await fixture.reads.set(failure ? .failure(.undecodable)
                                   : .success(TimelineTestValues.snapshot("2026-09-30", version: 2)))
            await fixture.reads.hold()
            let reading = Task { await fixture.controller.refresh() }
            await arrived(fixture, 2)
            await fixture.controller.refresh()
            XCTAssertTrue(fixture.controller.invalidateRunningRead())
            XCTAssertTrue(fixture.controller.invalidateRunningRead())
            await fixture.reads.set(.success(TimelineTestValues.snapshot("2026-09-30", version: 3)))
            await fixture.reads.finish(2)
            await arrived(fixture, 3)
            XCTAssertEqual(fixture.controller.snapshot?.projects.first?.name, "Version 1")
            XCTAssertNil(fixture.controller.failure)
            XCTAssertTrue(fixture.controller.loading)
            XCTAssertTrue(fixture.controller.invalidateRunningRead())
            await fixture.reads.set(.success(TimelineTestValues.snapshot("2026-09-30", version: 4)))
            await fixture.reads.finish(3)
            await arrived(fixture, 4)
            XCTAssertEqual(fixture.controller.snapshot?.projects.first?.name, "Version 1")
            await fixture.reads.finishAll()
            await reading.value
            let count = await fixture.reads.count, peak = await fixture.reads.peakActive
            XCTAssertEqual(count, 4)
            XCTAssertEqual(peak, 1)
            XCTAssertEqual(fixture.controller.snapshot?.projects.first?.name, "Version 4")
            XCTAssertFalse(fixture.controller.loading)
        }
    }

    func testOrdinaryPastSignalDoesNotInvalidateButExplicitRefreshDoes() async {
        let fixture = TimelineControllerFixture(day: "2026-09-29")
        defer { fixture.close() }
        await fixture.reads.hold()
        let reading = Task { await fixture.controller.refresh() }
        await arrived(fixture, 1)
        XCTAssertFalse(fixture.controller.invalidateRunningRead())
        fixture.controller.invalidateSelected()
        await fixture.reads.set(.success(TimelineTestValues.snapshot("2026-09-29", version: 2)))
        await fixture.reads.finish(1)
        await arrived(fixture, 2)
        XCTAssertNil(fixture.controller.snapshot)
        await fixture.reads.finishAll()
        await reading.value
        let count = await fixture.reads.count
        XCTAssertEqual(count, 2)
        XCTAssertEqual(fixture.controller.snapshot?.projects.first?.name, "Version 2")
    }

    func testHiddenOldDayAndOldEndpointRepliesCannotPublishOrFinishANewLoader() async {
        for change in ["hide", "day", "endpoint"] {
            let fixture = TimelineControllerFixture()
            defer { fixture.close() }
            await fixture.reads.set(.failure(.undecodable))
            await fixture.reads.hold()
            let old = Task { await fixture.controller.refresh() }
            await arrived(fixture, 1)
            XCTAssertTrue(fixture.controller.invalidateRunningRead())
            if change == "hide" { fixture.controller.setContext(day: "2026-09-30", visible: false) }
            else {
                if change == "endpoint" { fixture.controller.resetEndpoint() }
                else { fixture.controller.setContext(day: "2026-09-29", visible: true) }
            }
            await fixture.reads.reset { day in .success(TimelineTestValues.snapshot(day, version: 2)) }
            let newer = Task { await fixture.controller.refresh() }
            if change != "hide" { await arrived(fixture, 2) }
            await fixture.reads.finish(1)
            await old.value
            XCTAssertNil(fixture.controller.failure)
            XCTAssertNil(fixture.controller.snapshot)
            XCTAssertEqual(fixture.controller.loading, change != "hide")
            await fixture.reads.finishAll()
            await newer.value
            let count = await fixture.reads.count
            XCTAssertEqual(count, change == "hide" ? 1 : 2)
            XCTAssertEqual(fixture.controller.snapshot?.projects.first?.name, change == "hide" ? nil : "Version 2")
            XCTAssertFalse(fixture.controller.loading)
        }
    }

    func testMalformedContextOrMismatchedResponseCannotBeShown() async {
        let fixture = TimelineControllerFixture(day: "2026-02-29")
        defer { fixture.close() }
        await fixture.controller.refresh()
        let initial = await fixture.reads.count
        XCTAssertEqual(initial, 0)
        XCTAssertNil(fixture.controller.day)
        XCTAssertFalse(fixture.controller.visible)
        fixture.controller.setContext(day: "2026-09-30", visible: true)
        await fixture.reads.set(.success(TimelineTestValues.snapshot("2026-09-29")))
        await fixture.controller.refresh()
        XCTAssertNil(fixture.controller.snapshot)
        XCTAssertEqual(fixture.controller.failure, .undecodable)
    }
}
