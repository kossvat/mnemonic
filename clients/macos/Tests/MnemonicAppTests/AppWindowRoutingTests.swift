import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class AppWindowRoutingTests: XCTestCase {
    private actor OfflineRoutes {
        private(set) var counts = ProjectTestCounts()
        private var unexpectedRead: XCTestExpectation?

        func rejectReads(_ expectation: XCTestExpectation) { unexpectedRead = expectation }
        func allowReads() { unexpectedRead = nil }

        func live() -> LiveAnswer {
            counts.live += 1
            unexpectedRead?.fulfill()
            return .failed(.off)
        }

        func board() -> Result<ProjectBoard, LocalAPIFailure> {
            counts.board += 1
            unexpectedRead?.fulfill()
            return .failure(.off)
        }

        func detail(_ key: String) -> Result<ProjectDetail, LocalAPIFailure> {
            counts.details.append(key)
            unexpectedRead?.fulfill()
            return .failure(.off)
        }
    }

    private struct ProfileFixture {
        let root: URL
        let profile: MnemonicProfile

        init() throws {
            let scratch = FileManager.default.temporaryDirectory
                .appendingPathComponent("mn-app-routing-\(UUID().uuidString)")
            root = scratch
            let home = scratch.appendingPathComponent("profile")
            let user = scratch.appendingPathComponent("user")
            do {
                try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
                try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
                try "[ui]\nenabled = false\nport = 9\n".write(
                    to: home.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
                profile = try MnemonicProfile.resolve(
                    environment: [MnemonicProfile.homeVariable: home.path],
                    arguments: ["fixture-client"], userHome: user)
            } catch {
                try? FileManager.default.removeItem(at: scratch)
                throw error
            }
        }

        func remove() { try? FileManager.default.removeItem(at: root) }
    }

    @MainActor
    private func makeModel(_ profile: MnemonicProfile, source: OfflineRoutes,
                       clock: ProjectTestClock) -> MnemonicAppModel {
        let controller = AppProjectController(
            clock: clock, date: { clock.now }, ask: { await source.live() },
            board: { await source.board() }, detail: { key in await source.detail(key) })
        let model = MnemonicAppModel(profile: profile, controller: controller)
        model.activateWindow = {}
        return model
    }

    /// A route's final publication marks completion, not an arbitrary yield count.
    @MainActor
    private func completed(_ model: MnemonicAppModel, route: AppRoute,
                           action: @MainActor () -> Void) async {
        let ready = expectation(description: "The injected route read completed")
        var fulfilled = false
        let previous = model.onWindowStateChange
        model.onWindowStateChange = {
            previous?()
            guard !fulfilled, model.visible, model.route == route,
                  model.projects.route == route, !model.projects.loading,
                  model.projects.failure == .off else { return }
            fulfilled = true
            ready.fulfill()
        }
        defer { model.onWindowStateChange = previous }
        action()
        await fulfillment(of: [ready], timeout: 2)
    }

    @MainActor
    private func noReads(_ source: OfflineRoutes,
                         action: @MainActor () async -> Void) async {
        let unexpected = expectation(description: "A hidden or rejected route must not read")
        unexpected.isInverted = true
        await source.rejectReads(unexpected)
        await action()
        await fulfillment(of: [unexpected], timeout: 0.1)
        await source.allowReads()
    }

    @MainActor
    func testClosingOneWindowKeepsTheOtherActiveAndFinalCloseCancelsWork() async throws {
        let fixture = try ProfileFixture()
        defer { fixture.remove() }
        let source = OfflineRoutes()
        let clock = ProjectTestClock()
        let model = makeModel(fixture.profile, source: source, clock: clock)
        let windows = [NSObject(), NSObject()]
        let first = ObjectIdentifier(windows[0]), second = ObjectIdentifier(windows[1])
        defer { model.projects.hide() }

        await completed(model, route: .projects) { model.window(first, visible: true) }
        model.window(second, visible: true)
        model.window(first, visible: false)
        XCTAssertTrue(model.visible)
        XCTAssertTrue(model.projects.coordinator.surfaceVisible)
        XCTAssertEqual(clock.active, [30])
        let before = await source.counts
        XCTAssertEqual(before, ProjectTestCounts(live: 1, board: 1))

        let queued = try clock.queue(30)
        model.window(second, visible: false)
        XCTAssertFalse(model.visible)
        XCTAssertTrue(model.projects.coordinator.armed.isEmpty)
        XCTAssertTrue(clock.active.isEmpty)
        await noReads(source) {
            queued()
            queued()
            await model.projects.signalled()
            await model.refreshAll()
        }
        let after = await source.counts
        XCTAssertEqual(after, before)
        XCTAssertEqual(clock.starts, 1)
    }

    @MainActor
    func testWindowClosedBeforeItsOpeningTaskRunsStartsNothing() async throws {
        let fixture = try ProfileFixture()
        defer { fixture.remove() }
        let source = OfflineRoutes()
        let clock = ProjectTestClock()
        let model = makeModel(fixture.profile, source: source, clock: clock)
        let window = NSObject()
        let id = ObjectIdentifier(window)
        await noReads(source) {
            model.window(id, visible: true)
            model.window(id, visible: false)
        }
        let reads = await source.counts
        XCTAssertEqual(reads, ProjectTestCounts())
        XCTAssertFalse(model.visible)
        XCTAssertTrue(model.projects.coordinator.armed.isEmpty)
        XCTAssertEqual(clock.starts, 0)
    }

    @MainActor
    func testSameProfileURLSelectsItsKeyReadsOnceAndActivatesOnce() async throws {
        let fixture = try ProfileFixture()
        defer { fixture.remove() }
        let source = OfflineRoutes()
        let clock = ProjectTestClock()
        let model = makeModel(fixture.profile, source: source, clock: clock)
        let window = NSObject()
        let id = ObjectIdentifier(window)
        defer { model.window(id, visible: false) }
        await completed(model, route: .projects) { model.window(id, visible: true) }
        var activations = 0
        model.activateWindow = { activations += 1 }
        let identity = try XCTUnwrap(model.identity)
        let url = try XCTUnwrap(AppLink.project("alpha:part-two").url(identity: identity))
        await completed(model, route: .project("alpha:part-two")) {
            XCTAssertTrue(model.open(url))
        }
        XCTAssertEqual(model.route, .project("alpha:part-two"))
        XCTAssertEqual(model.projects.route, model.route)
        XCTAssertEqual(activations, 1)
        let reads = await source.counts
        XCTAssertEqual(reads, ProjectTestCounts(live: 1, board: 1, details: ["alpha:part-two"]))
    }

    @MainActor
    func testWrongProfileAndEndpointPayloadChangeNothing() async throws {
        let fixture = try ProfileFixture()
        defer { fixture.remove() }
        let source = OfflineRoutes()
        let clock = ProjectTestClock()
        let model = makeModel(fixture.profile, source: source, clock: clock)
        var activations = 0
        model.activateWindow = { activations += 1 }
        let identity = try XCTUnwrap(model.identity)
        let other = String(repeating: identity.first == "a" ? "b" : "a", count: 32)
        let wrongProfile = try XCTUnwrap(AppLink.project("beta").url(identity: other))
        let endpointPayload = try XCTUnwrap(URL(string:
            "mnemonic://project/beta?profile=\(identity)&endpoint=http%3A%2F%2Flocalhost%3A9999"))
        let endpoint = model.endpoint
        await noReads(source) {
            XCTAssertFalse(model.open(wrongProfile))
            XCTAssertFalse(model.open(endpointPayload))
            await model.projects.signalled()
            await model.refreshAll()
        }
        let reads = await source.counts
        XCTAssertEqual(reads, ProjectTestCounts())
        XCTAssertEqual(model.route, .projects)
        XCTAssertEqual(model.projects.route, .projects)
        XCTAssertEqual(model.endpoint, endpoint)
        XCTAssertEqual(model.profile.home, fixture.profile.home)
        XCTAssertEqual(activations, 0)
        XCTAssertEqual(clock.starts, 0)
    }

    @MainActor
    func testURLWhileHiddenReadsOnlyAfterAWindowBecomesVisible() async throws {
        let fixture = try ProfileFixture()
        defer { fixture.remove() }
        let source = OfflineRoutes()
        let clock = ProjectTestClock()
        let model = makeModel(fixture.profile, source: source, clock: clock)
        var activations = 0
        model.activateWindow = { activations += 1 }
        let identity = try XCTUnwrap(model.identity)
        let url = try XCTUnwrap(AppLink.project("alpha").url(identity: identity))
        await noReads(source) {
            XCTAssertTrue(model.open(url))
            await model.projects.signalled()
            await model.refreshAll()
        }
        let hiddenReads = await source.counts
        XCTAssertEqual(hiddenReads, ProjectTestCounts())
        XCTAssertEqual(model.route, .project("alpha"))
        XCTAssertEqual(activations, 1)
        XCTAssertEqual(clock.starts, 0)
        XCTAssertFalse(model.visible)

        let window = NSObject()
        let id = ObjectIdentifier(window)
        defer { model.window(id, visible: false) }
        await completed(model, route: .project("alpha")) { model.window(id, visible: true) }
        let shownReads = await source.counts
        XCTAssertEqual(shownReads, ProjectTestCounts(live: 1, board: 0, details: ["alpha"]))
        XCTAssertTrue(model.visible)
        XCTAssertEqual(clock.active, [30])
        XCTAssertEqual(activations, 1)
    }
}
