import Combine
import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicApp

final class AppProjectControllerTests: XCTestCase {
    private enum MissingRequest: Error { case timedOut }

    @MainActor
    private func mainQueueBarrier() async {
        let delivered = expectation(description: "Queued main-actor callbacks have had a delivery turn")
        DispatchQueue.main.async { delivered.fulfill() }
        await fulfillment(of: [delivered], timeout: 5)
    }

    @MainActor
    private func arrived(_ source: ProjectTestSource, _ kind: ProjectTestRequest, count: Int = 1) async throws {
        let requested = expectation(description: "Injected \(kind) request \(count) arrived")
        let notice = await source.notifyWhenRequested(kind, count: count) { requested.fulfill() }
        await fulfillment(of: [requested], timeout: 5)
        await source.cancelNotice(notice)
        guard await source.counts.number(kind) >= count else {
            throw MissingRequest.timedOut
        }
    }

    @MainActor
    private func counts(_ source: ProjectTestSource, live: Int, board: Int, details: [String] = [],
                        file: StaticString = #filePath, line: UInt = #line) async {
        let actual = await source.counts
        XCTAssertEqual(actual, ProjectTestCounts(live: live, board: board, details: details), file: file, line: line)
    }

    @MainActor
    func testInitialHiddenSignalsSelectionsAndRefreshesStartNoWork() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        var legacy: [AppRoute] = []
        controller.onLegacyRefresh = { route in legacy.append(route) }
        await controller.signalled()
        await controller.refresh()
        XCTAssertNil(controller.select(.overview))
        await controller.signalled()
        await controller.refresh()
        XCTAssertNil(controller.select(.project("demoapp")))
        await controller.setVisible(false)
        await mainQueueBarrier()
        await counts(source, live: 0, board: 0)
        XCTAssertEqual(clock.starts, 0)
        XCTAssertEqual(controller.coordinator.armed, [])
        XCTAssertTrue(legacy.isEmpty)
        XCTAssertFalse(controller.visible)
        XCTAssertFalse(controller.loading)
    }

    @MainActor
    func testOpeningBoardAndSelectingDetailAskOnceAndExplicitRefreshCanRepeat() async throws {
        let fixture = try ProjectTestFixtures()
        let source = ProjectTestSource(fixture)
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await controller.setVisible(true)
        await controller.setVisible(true)
        await counts(source, live: 1, board: 1)
        XCTAssertEqual(controller.board, fixture.board)
        XCTAssertEqual(clock.active, [30, 60, 300])
        let selection = try XCTUnwrap(controller.select(.project("demoapp")))
        await selection.value
        XCTAssertNil(controller.select(.project("demoapp")))
        await controller.refresh(force: false)
        await counts(source, live: 1, board: 1, details: ["demoapp"])
        XCTAssertEqual(controller.detail, fixture.detail)
        await controller.refresh()
        await counts(source, live: 1, board: 1, details: ["demoapp", "demoapp"])
        controller.hide()
    }

    @MainActor
    func testSignalRefreshTickAndReopeningReadOnlyTheVisibleBoard() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await controller.setVisible(true)
        await controller.signalled()
        await counts(source, live: 2, board: 2)
        let completed = expectation(description: "The refreshed board reached the controller")
        let rearmed = expectation(description: "The refresh timer was rearmed after the live answer")
        var started = false
        var finished = false
        let loading = controller.$loading.dropFirst().sink { value in
            if value { started = true }
        }
        controller.onPresentationChange = {
            guard started, !finished, !controller.loading else { return }
            finished = true
            completed.fulfill()
        }
        clock.onArm = { seconds in
            if seconds == 30 { rearmed.fulfill() }
        }
        try clock.queue(30)()
        try await arrived(source, .live, count: 3)
        try await arrived(source, .board, count: 3)
        await fulfillment(of: [completed, rearmed], timeout: 5)
        loading.cancel()
        controller.onPresentationChange = nil
        clock.onArm = nil
        await counts(source, live: 3, board: 3)
        XCTAssertEqual(clock.active, [30, 60, 300])
        controller.hide()
        XCTAssertEqual(clock.active, [])
        XCTAssertEqual(controller.coordinator.armed, [])
        await controller.signalled()
        await controller.refresh()
        await counts(source, live: 3, board: 3)
        await controller.setVisible(true)
        await counts(source, live: 4, board: 4)
        controller.hide()
    }

    @MainActor
    func testOverviewRejectsQueuedCanceledCallbacksBeforeAndAfterReopening() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        var legacy: [AppRoute] = []
        controller.onLegacyRefresh = { route in legacy.append(route) }
        XCTAssertNil(controller.select(.overview))
        await controller.setVisible(true)
        XCTAssertEqual(legacy, [.overview])
        let oldRefresh = try clock.queue(30)
        let oldLabels = try clock.queue(60)
        let oldSafety = try clock.queue(300)
        let shownAt = controller.now
        controller.hide()
        clock.now = shownAt.addingTimeInterval(600)
        for delivery in [oldRefresh, oldLabels, oldSafety] { delivery() }
        await controller.signalled()
        await controller.refresh()
        await mainQueueBarrier()
        await counts(source, live: 1, board: 0)
        XCTAssertEqual(legacy, [.overview])
        XCTAssertEqual(controller.now, shownAt)
        XCTAssertTrue(clock.active.isEmpty)

        await controller.setVisible(true)
        for delivery in [oldRefresh, oldLabels, oldSafety] { delivery(); delivery() }
        await mainQueueBarrier()
        await counts(source, live: 2, board: 0)
        XCTAssertEqual(legacy, [.overview, .overview])
        XCTAssertEqual(controller.now, clock.now)
        XCTAssertEqual(clock.active, [30, 60, 300])
        XCTAssertEqual(controller.coordinator.armed, ["refresh", "labels", "safety"])
        controller.hide()
    }

    @MainActor
    func testHideAfterTimerDeliveryButBeforeItsTaskPreventsLegacyRefresh() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        var legacy = 0
        controller.onLegacyRefresh = { _ in legacy += 1 }
        controller.select(.overview)
        await controller.setVisible(true)
        try clock.queue(30)()
        controller.hide()
        await mainQueueBarrier()
        await counts(source, live: 1, board: 0)
        XCTAssertEqual(legacy, 1)
        XCTAssertTrue(clock.active.isEmpty)
    }

    @MainActor
    func testLateBoardReplyAfterHideCannotPublish() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await source.holdNext(.board)
        var publications = 0
        let listening = controller.$board.dropFirst().sink { _ in publications += 1 }
        defer { listening.cancel() }
        let opening = Task { await controller.setVisible(true) }
        try await arrived(source, .board)
        XCTAssertTrue(controller.loading)
        controller.hide()
        await source.finishBoard()
        await opening.value
        XCTAssertNil(controller.board)
        XCTAssertEqual(publications, 0)
        XCTAssertFalse(controller.loading)
        XCTAssertTrue(clock.active.isEmpty)
        await counts(source, live: 1, board: 1)
    }

    @MainActor
    func testLateDetailReplyAfterNavigationCannotReplaceTheCurrentPage() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await controller.setVisible(true)
        await source.holdNext(.detail)
        let detail = try XCTUnwrap(controller.select(.project("demoapp")))
        try await arrived(source, .detail)
        let overview = try XCTUnwrap(controller.select(.overview))
        await overview.value
        var publications = 0
        let listening = controller.$detail.dropFirst().sink { _ in publications += 1 }
        defer { listening.cancel() }
        await source.finishDetail()
        await detail.value
        XCTAssertEqual(controller.route, .overview)
        XCTAssertNil(controller.detail)
        XCTAssertNil(controller.failure)
        XCTAssertEqual(publications, 0)
        XCTAssertFalse(controller.loading)
        controller.hide()
    }

    @MainActor
    func testLateBoardFailureCannotOverwriteASuccessfulDetailNavigation() async throws {
        let fixture = try ProjectTestFixtures()
        let source = ProjectTestSource(fixture)
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await controller.setVisible(true)
        await source.holdNext(.board)
        let refresh = Task { await controller.refresh() }
        try await arrived(source, .board, count: 2)
        let selection = try XCTUnwrap(controller.select(.project("demoapp")))
        await selection.value
        await source.finishBoard(.undecodable)
        await refresh.value
        XCTAssertEqual(controller.route, .project("demoapp"))
        XCTAssertEqual(controller.detail, fixture.detail)
        XCTAssertEqual(controller.board, fixture.board)
        XCTAssertNil(controller.failure)
        XCTAssertFalse(controller.loading)
        controller.hide()
    }

    @MainActor
    func testLateOpeningLiveReplyAfterHideStaysUnpublishedAndNeverLoadsTheBoard() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await source.holdNext(.live)
        let opening = Task { await controller.setVisible(true) }
        try await arrived(source, .live)
        controller.hide()
        await source.finishLive()
        await opening.value
        XCTAssertNil(controller.live.shown)
        XCTAssertEqual(controller.live.publications, 0)
        XCTAssertNil(controller.board)
        XCTAssertTrue(clock.active.isEmpty)
        await counts(source, live: 1, board: 0)
    }

    @MainActor
    func testMissingProjectFailureAndCanonicalDetailResponseAreAccepted() async throws {
        let fixture = try ProjectTestFixtures()
        let source = ProjectTestSource(fixture)
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await controller.setVisible(true)
        await source.failDetail(.earlierDaemon)
        let missing = try XCTUnwrap(controller.select(.project("missing")))
        await missing.value
        XCTAssertEqual(controller.failure, .earlierDaemon)
        XCTAssertNil(controller.detail)
        XCTAssertFalse(controller.loading)
        await source.failDetail(nil)
        let redirected = try XCTUnwrap(controller.select(.project("alias")))
        await redirected.value
        XCTAssertEqual(controller.detail?.key, "demoapp")
        XCTAssertNil(controller.failure)
        XCTAssertFalse(controller.loading)
        await counts(source, live: 1, board: 1, details: ["missing", "alias"])
        controller.hide()
    }

    @MainActor
    func testRouteChangeWhileOpeningLiveIsDelayedDoesNotDuplicateTheDetailRead() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await source.holdNext(.live)
        let opening = Task { await controller.setVisible(true) }
        try await arrived(source, .live)
        let selection = try XCTUnwrap(controller.select(.project("demoapp")))
        await selection.value
        await counts(source, live: 1, board: 0, details: ["demoapp"])
        await source.finishLive()
        await opening.value
        await counts(source, live: 1, board: 0, details: ["demoapp"])
        XCTAssertEqual(controller.route, .project("demoapp"))
        XCTAssertEqual(controller.detail?.key, "demoapp")
        controller.hide()
    }

    @MainActor
    func testPresentingFixturesArmsNoTimersAndReadsNoSources() async throws {
        let fixture = try ProjectTestFixtures()
        let source = ProjectTestSource(fixture)
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        controller.present(board: fixture.board, detail: fixture.detail,
                           live: .snapshot(fixture.live), route: .project("demoapp"))
        XCTAssertEqual(controller.board, fixture.board)
        XCTAssertEqual(controller.detail, fixture.detail)
        XCTAssertEqual(controller.live.shown, fixture.live)
        XCTAssertEqual(clock.starts, 0)
        XCTAssertEqual(controller.coordinator.armed, [])
        await counts(source, live: 0, board: 0)
        controller.hide()
    }
    @MainActor
    func testHiddenRouteChangeClearsAnotherProjectBeforeReopening() async throws {
        let fixture = try ProjectTestFixtures()
        let source = ProjectTestSource(fixture)
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        controller.select(.project("demoapp"))
        await controller.setVisible(true)
        XCTAssertEqual(controller.detail, fixture.detail)
        controller.hide()
        XCTAssertNil(controller.select(.project("second-project")))
        XCTAssertNil(controller.detail)
        await source.holdNext(.live)
        let opening = Task { await controller.setVisible(true) }
        try await arrived(source, .live, count: 2)
        XCTAssertNil(controller.detail)
        await source.finishLive()
        await opening.value
        await counts(source, live: 2, board: 0, details: ["demoapp", "second-project"])
        controller.hide()
    }

    @MainActor
    func testVisibleRefreshAdvancesStaleCutoffWithoutLiveLabelTimer() async throws {
        let fixture = try ProjectTestFixtures()
        var object = try XCTUnwrap(JSONSerialization.jsonObject(
            with: JSONEncoder().encode(fixture.live)) as? [String: Any])
        object["projects"] = []
        object["unknown"] = ["sessions": [], "more_sessions": 0]
        object["more_projects"] = 0
        let empty = try JSONDecoder().decode(LiveSnapshot.self,
            from: JSONSerialization.data(withJSONObject: object))
        let clock = ProjectTestClock()
        let activity = try XCTUnwrap(fixture.board.projects.first?.lastActiveAt)
        clock.now = Date(timeIntervalSince1970: Double(activity + 30 * 86_400))
        let controller = AppProjectController(clock: clock, date: { clock.now },
            ask: { .snapshot(empty) }, board: { .success(fixture.board) },
            detail: { _ in .success(fixture.detail) })
        await controller.setVisible(true)
        XCTAssertEqual(clock.active, [30])
        XCTAssertTrue(ProjectPresentation.groups(board: controller.board, live: nil,
                                                  now: controller.now).stale.isEmpty)
        let refreshed = expectation(description: "Visible refresh advances the board clock")
        clock.now = clock.now.addingTimeInterval(30)
        var done = false
        controller.onPresentationChange = {
            guard !done, controller.now == clock.now, !controller.loading else { return }
            done = true
            refreshed.fulfill()
        }
        try clock.queue(30)()
        await fulfillment(of: [refreshed], timeout: 5)
        XCTAssertEqual(controller.now, clock.now)
        XCTAssertEqual(ProjectPresentation.groups(board: controller.board, live: nil,
                                                  now: controller.now).stale.count, 1)
        controller.hide()
    }

    @MainActor
    func testHeldSignalCannotReloadAReopenedVisit() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await controller.setVisible(true)
        await source.holdNext(.live)
        let signal = Task { await controller.signalled() }
        try await arrived(source, .live, count: 2)
        controller.hide()
        await controller.setVisible(true)
        await counts(source, live: 2, board: 2)
        await source.finishLive()
        await signal.value
        await counts(source, live: 3, board: 2)
        controller.hide()
    }

    @MainActor
    func testHeldSignalCannotReloadANewlySelectedProject() async throws {
        let source = ProjectTestSource(try ProjectTestFixtures())
        let clock = ProjectTestClock()
        let controller = projectTestController(source, clock: clock)
        await controller.setVisible(true)
        await source.holdNext(.live)
        let signal = Task { await controller.signalled() }
        try await arrived(source, .live, count: 2)
        let selection = try XCTUnwrap(controller.select(.project("demoapp")))
        await selection.value
        await counts(source, live: 2, board: 1, details: ["demoapp"])
        await source.finishLive()
        await signal.value
        await counts(source, live: 2, board: 1, details: ["demoapp"])
        controller.hide()
    }



}
