import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class AppProjectInvalidationTests: XCTestCase {
    @MainActor
    private func arrived(_ fixture: ProjectInvalidationFixture, _ count: Int) async {
        let requested = expectation(description: "Project read \(count) arrived")
        await fixture.requested(count) { requested.fulfill() }
        await fulfillment(of: [requested], timeout: 5)
    }

    @MainActor
    private func liveArrived(_ fixture: ProjectInvalidationFixture, _ count: Int) async {
        let requested = expectation(description: "Live read \(count) arrived")
        let notice = await fixture.live.notifyWhenRequested(.live, count: count) { requested.fulfill() }
        await fulfillment(of: [requested], timeout: 5)
        await fixture.live.cancelNotice(notice)
    }

    @MainActor
    private func counts(_ fixture: ProjectInvalidationFixture, _ expected: Int, peak: Int = 1,
                        file: StaticString = #filePath, line: UInt = #line) async {
        let count = await fixture.count(), active = await fixture.peak()
        XCTAssertEqual(count, expected, file: file, line: line)
        XCTAssertEqual(active, peak, file: file, line: line)
    }

    @MainActor
    func testForceRefreshCoalescesInvalidationsAndRejectsObsoleteSnapshotsAndErrors() async throws {
        for route in [AppRoute.projects, .project("demoapp")] {
            for failed in [false, true] {
                let fixture = try ProjectInvalidationFixture(route)
                defer { fixture.controller.hide(); Task { await fixture.finishAll() } }
                await fixture.controller.setVisible(true)
                XCTAssertEqual(fixture.shownDone, 0)
                try await fixture.answer(done: 1, failure: failed ? .undecodable : nil)
                await fixture.hold()
                let refreshing = Task { await fixture.controller.refresh() }
                await arrived(fixture, 2)
                await fixture.controller.refresh(force: false)
                await counts(fixture, 2)
                try await fixture.answer(done: 2)
                await fixture.controller.refresh()
                await fixture.controller.refresh()
                await counts(fixture, 2)
                await fixture.finish(2)
                await arrived(fixture, 3)
                XCTAssertEqual(fixture.shownDone, 0, "A revoked response cannot replace the visible plan")
                XCTAssertNil(fixture.controller.failure)
                XCTAssertTrue(fixture.controller.loading)
                await fixture.finishAll()
                await refreshing.value
                await counts(fixture, 3)
                XCTAssertEqual(fixture.shownDone, 2)
                XCTAssertNil(fixture.controller.failure)
                XCTAssertFalse(fixture.controller.loading)
            }
        }
    }

    @MainActor
    func testAReplacementCanItselfBeInvalidatedWithoutConcurrentReads() async throws {
        for route in [AppRoute.projects, .project("demoapp")] {
            let fixture = try ProjectInvalidationFixture(route)
            defer { fixture.controller.hide(); Task { await fixture.finishAll() } }
            await fixture.controller.setVisible(true)
            try await fixture.answer(done: 1)
            await fixture.hold()
            let refreshing = Task { await fixture.controller.refresh() }
            await arrived(fixture, 2)
            await fixture.controller.refresh()
            await fixture.finish(2)
            await arrived(fixture, 3)
            try await fixture.answer(done: 2)
            await fixture.controller.refresh()
            await fixture.controller.refresh()
            await fixture.finish(3)
            await arrived(fixture, 4)
            XCTAssertEqual(fixture.shownDone, 0)
            XCTAssertNil(fixture.controller.failure)
            await fixture.finishAll()
            await refreshing.value
            await counts(fixture, 4)
            XCTAssertEqual(fixture.shownDone, 2)
        }
    }

    @MainActor
    func testHideAndNavigationRevokeQueuedProjectReplacements() async throws {
        for route in [AppRoute.projects, .project("demoapp")] {
            for navigate in [false, true] {
                let fixture = try ProjectInvalidationFixture(route)
                defer { fixture.controller.hide(); Task { await fixture.finishAll() } }
                await fixture.controller.setVisible(true)
                try await fixture.answer(done: 1, failure: .undecodable)
                await fixture.hold()
                let refreshing = Task { await fixture.controller.refresh() }
                await arrived(fixture, 2)
                await fixture.controller.refresh()
                if navigate {
                    let selected = try XCTUnwrap(fixture.controller.select(.search))
                    await selected.value
                } else { fixture.controller.hide() }
                await fixture.finishAll()
                await refreshing.value
                await fixture.controller.refresh()
                await counts(fixture, 2)
                XCTAssertEqual(fixture.shownDone, 0)
                XCTAssertNil(fixture.controller.failure)
                XCTAssertFalse(fixture.controller.loading)
            }
        }
    }

    @MainActor
    func testSignalInvalidatesBeforeLiveAndDoesNotReadAgainAfterItsReplacementFinishes() async throws {
        for route in [AppRoute.projects, .project("demoapp")] {
            let fixture = try ProjectInvalidationFixture(route)
            defer { fixture.controller.hide(); Task { await fixture.finishAll() } }
            await fixture.controller.setVisible(true)
            try await fixture.answer(done: 1)
            await fixture.hold()
            let refreshing = Task { await fixture.controller.refresh() }
            await arrived(fixture, 2)
            await fixture.live.holdNext(.live)
            let signalling = Task { await fixture.controller.signalled() }
            await liveArrived(fixture, 2)
            try await fixture.answer(done: 2)
            await fixture.finish(2)
            await arrived(fixture, 3)
            XCTAssertEqual(fixture.shownDone, 0)
            await fixture.finish(3)
            await refreshing.value
            XCTAssertEqual(fixture.shownDone, 2)
            await fixture.live.finishLive()
            await signalling.value
            await counts(fixture, 3)
        }
    }

    @MainActor
    func testVisibleThirtySecondTickInvalidatesAnOwnedRead() async throws {
        for route in [AppRoute.projects, .project("demoapp")] {
            let fixture = try ProjectInvalidationFixture(route)
            defer { fixture.controller.hide(); Task { await fixture.finishAll() } }
            await fixture.controller.setVisible(true)
            try await fixture.answer(done: 1)
            await fixture.hold()
            let refreshing = Task { await fixture.controller.refresh() }
            await arrived(fixture, 2)
            await fixture.live.holdNext(.live)
            fixture.clock.now = fixture.clock.now.addingTimeInterval(30)
            try fixture.clock.queue(30)()
            await liveArrived(fixture, 2)
            XCTAssertEqual(fixture.controller.now, fixture.clock.now)
            try await fixture.answer(done: 2)
            await fixture.finish(2)
            await arrived(fixture, 3)
            XCTAssertEqual(fixture.shownDone, 0)
            await fixture.finish(3)
            await refreshing.value
            await fixture.live.finishLive()
            await counts(fixture, 3)
            XCTAssertEqual(fixture.shownDone, 2)
        }
    }

    @MainActor
    func testOldAndNewVisitsKeepTheirInvalidationsSeparate() async throws {
        for navigate in [false, true] {
            let fixture = try ProjectInvalidationFixture(.project("demoapp"))
            defer { fixture.controller.hide(); Task { await fixture.finishAll() } }
            await fixture.controller.setVisible(true)
            try await fixture.answer(done: 1, failure: .undecodable)
            await fixture.hold()
            let old = Task { await fixture.controller.refresh() }
            await arrived(fixture, 2)
            await fixture.controller.refresh()
            try await fixture.answer(done: 1, key: navigate ? "second-project" : "demoapp")
            let newer: Task<Void, Never>
            if navigate {
                newer = try XCTUnwrap(fixture.controller.select(.project("second-project")))
            } else {
                fixture.controller.hide()
                newer = Task { await fixture.controller.setVisible(true) }
            }
            await arrived(fixture, 3)
            await fixture.controller.refresh()
            try await fixture.answer(done: 2, key: navigate ? "second-project" : "demoapp")
            await fixture.finish(2)
            await old.value
            XCTAssertTrue(fixture.controller.loading, "The old visit cannot finish the new visit's loader")
            XCTAssertNil(fixture.controller.failure)
            await counts(fixture, 3, peak: 2)
            await fixture.finish(3)
            await arrived(fixture, 4)
            await fixture.finishAll()
            await newer.value
            await counts(fixture, 4, peak: 2)
            XCTAssertEqual(fixture.controller.detail?.key, navigate ? "second-project" : "demoapp")
            XCTAssertEqual(fixture.shownDone, 2)
            XCTAssertNil(fixture.controller.failure)
            XCTAssertFalse(fixture.controller.loading)
        }
    }
}
