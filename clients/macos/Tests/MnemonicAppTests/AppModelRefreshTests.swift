import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class AppModelRefreshTests: XCTestCase {
    private struct ProfileFixture {
        let root: URL
        let profile: MnemonicProfile

        init() throws {
            let scratch = FileManager.default.temporaryDirectory
                .appendingPathComponent("mn-model-refresh-\(UUID().uuidString)")
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

    private actor ProjectReads {
        private let fixture: ProjectTestFixtures
        private var answer: LiveAnswer
        private(set) var counts = ProjectTestCounts()

        init(_ fixture: ProjectTestFixtures, answer: LiveAnswer) {
            self.fixture = fixture
            self.answer = answer
        }

        func setAnswer(_ value: LiveAnswer) { answer = value }

        func live() -> LiveAnswer {
            counts.live += 1
            return answer
        }

        func board() -> Result<ProjectBoard, LocalAPIFailure> {
            counts.board += 1
            return .success(fixture.board)
        }

        func detail(_ key: String) -> Result<ProjectDetail, LocalAPIFailure> {
            counts.details.append(key)
            return .success(fixture.detail)
        }
    }

    @MainActor
    private final class MemoryReads {
        let values = MemoryListResponse(results: [
            Memory(id: "fixture-memory", timestamp: "2026-10-04T12:00:00Z",
                   title: "Fixture decision", content: "A synthetic memory for the map",
                   memoryType: "decision", tags: [], importance: 0.5)
        ], count: 1)
        private(set) var count = 0
        var held = false
        var onRequest: (() -> Void)?
        private var pending: [CheckedContinuation<MemoryListResponse, Error>] = []

        func read() async throws -> MemoryListResponse {
            count += 1
            let notify = onRequest
            onRequest = nil
            if held {
                return try await withCheckedThrowingContinuation { continuation in
                    pending.append(continuation)
                    notify?()
                }
            }
            notify?()
            return values
        }

        func finish() {
            held = false
            let replies = pending
            pending.removeAll()
            for reply in replies { reply.resume(returning: values) }
        }
    }

    @MainActor
    private func makeModel(_ profile: MnemonicProfile, source: ProjectReads,
                           memories: MemoryReads, clock: ProjectTestClock) -> MnemonicAppModel {
        let controller = AppProjectController(
            clock: clock, date: { clock.now }, ask: { await source.live() },
            board: { await source.board() }, detail: { key in await source.detail(key) })
        let model = MnemonicAppModel(profile: profile, controller: controller,
                                     readMemories: { try await memories.read() })
        model.activateWindow = {}
        return model
    }

    @MainActor
    func testMemoryMapRouteLoadsBackingMemoriesAndCoalescesItsViewRequest() async throws {
        let profile = try ProfileFixture()
        defer { profile.remove() }
        let fixture = try ProjectTestFixtures()
        let source = ProjectReads(fixture, answer: .failed(.off))
        let memories = MemoryReads()
        let clock = ProjectTestClock()
        let model = makeModel(profile.profile, source: source, memories: memories, clock: clock)
        defer { model.projects.hide(); memories.finish() }
        model.route = .memoryMap
        await model.refreshMemories()
        XCTAssertEqual(memories.count, 0, "A hidden map must not load memories")

        memories.held = true
        let requested = expectation(description: "Opening the map requests its backing memories")
        memories.onRequest = { requested.fulfill() }
        let opening = Task { await model.projects.setVisible(true) }
        await fulfillment(of: [requested], timeout: 2)
        let coalesced = expectation(description: "The view joins the existing route read")
        let viewRead = Task {
            await model.refreshMemories()
            coalesced.fulfill()
        }
        await fulfillment(of: [coalesced], timeout: 2)
        XCTAssertEqual(memories.count, 1, "The view and route must share the in-flight read")
        memories.finish()
        await viewRead.value
        await opening.value
        XCTAssertEqual(model.memories, memories.values.results)
        XCTAssertFalse(model.isLoading)
        XCTAssertNil(model.setupMessage)
        let reads = await source.counts
        XCTAssertEqual(reads, ProjectTestCounts(live: 1, board: 0))
    }

    @MainActor
    func testLateMapMemoriesCannotPublishAfterNavigationOrHide() async throws {
        for navigate in [true, false] {
            let profile = try ProfileFixture()
            defer { profile.remove() }
            let fixture = try ProjectTestFixtures()
            let source = ProjectReads(fixture, answer: .failed(.off))
            let memories = MemoryReads()
            memories.held = true
            let clock = ProjectTestClock()
            let model = makeModel(profile.profile, source: source, memories: memories, clock: clock)
            defer { model.projects.hide(); memories.finish() }
            model.route = .memoryMap
            model.projects.present(board: nil, detail: nil, live: .failed(.off), route: .memoryMap)
            let requested = expectation(description: "The map read is held before its response")
            memories.onRequest = { requested.fulfill() }
            let reading = Task { await model.refreshMemories() }
            await fulfillment(of: [requested], timeout: 2)
            if navigate {
                model.route = .search
            } else {
                let window = NSObject()
                model.window(ObjectIdentifier(window), visible: false)
            }
            memories.finish()
            await reading.value
            XCTAssertEqual(memories.count, 1)
            XCTAssertTrue(model.memories.isEmpty)
            XCTAssertFalse(model.isLoading)
            XCTAssertNil(model.setupMessage)
            XCTAssertEqual(clock.starts, 0)
        }
    }

    @MainActor
    func testMemoryReadsStillWorkOnMemoriesButRejectUnrelatedRoutes() async throws {
        let profile = try ProfileFixture()
        defer { profile.remove() }
        let fixture = try ProjectTestFixtures()
        let source = ProjectReads(fixture, answer: .failed(.off))
        let memories = MemoryReads()
        let clock = ProjectTestClock()
        let model = makeModel(profile.profile, source: source, memories: memories, clock: clock)
        defer { model.projects.hide() }
        model.route = .search
        model.projects.present(board: nil, detail: nil, live: .failed(.off), route: .search)
        await model.refreshMemories()
        XCTAssertEqual(memories.count, 0)
        model.projects.hide()
        model.route = .memories
        await model.projects.setVisible(true)
        XCTAssertEqual(memories.count, 1)
        XCTAssertEqual(model.memories, memories.values.results)
    }

    @MainActor
    func testExplicitRefreshRecoversLiveFailureAndReloadsOnlyTheSelectedProjectRoute() async throws {
        for route in [AppRoute.projects, .project("demoapp")] {
            let profile = try ProfileFixture()
            defer { profile.remove() }
            let fixture = try ProjectTestFixtures()
            let source = ProjectReads(fixture, answer: .failed(.off))
            let memories = MemoryReads()
            let clock = ProjectTestClock()
            let model = makeModel(profile.profile, source: source, memories: memories, clock: clock)
            defer { model.projects.hide() }
            model.route = route
            await model.projects.setVisible(true)
            XCTAssertEqual(model.projects.live.failure, .off)
            XCTAssertFalse(model.isOnline)

            await source.setAnswer(.snapshot(fixture.live))
            await model.refreshAll()
            XCTAssertNil(model.projects.live.failure)
            XCTAssertEqual(model.projects.live.shown, fixture.live)
            XCTAssertTrue(model.isOnline)
            let reads = await source.counts
            let expected = route == .projects
                ? ProjectTestCounts(live: 2, board: 2)
                : ProjectTestCounts(live: 2, board: 0, details: ["demoapp", "demoapp"])
            XCTAssertEqual(reads, expected)
            XCTAssertEqual(memories.count, 0)

            model.projects.hide()
            await model.refreshAll()
            let hiddenReads = await source.counts
            XCTAssertEqual(hiddenReads, reads)
            XCTAssertTrue(model.projects.coordinator.armed.isEmpty)
            XCTAssertTrue(clock.active.isEmpty)
        }
    }

    @MainActor
    func testLegacyRoutesUseValidLiveHealthWithoutMaskingKnownFailures() throws {
        let profile = try ProfileFixture()
        defer { profile.remove() }
        let fixture = try ProjectTestFixtures()
        let source = ProjectReads(fixture, answer: .failed(.off))
        let memories = MemoryReads()
        let clock = ProjectTestClock()
        let model = makeModel(profile.profile, source: source, memories: memories, clock: clock)
        defer { model.projects.hide() }
        XCTAssertFalse(model.isOnline)
        let status = StatusResponse(totalMemories: 1, byType: [], entities: 0, edges: 0, lastActivity: nil)

        for route in [AppRoute.memories, .memoryMap, .entities, .search] {
            model.projects.hide()
            model.route = route
            model.status = nil
            model.setupMessage = nil
            model.projects.present(board: nil, detail: nil, live: .snapshot(fixture.live), route: route)
            XCTAssertTrue(model.isOnline, "A valid live snapshot proves the daemon answered")
            XCTAssertNil(model.status)

            model.status = status
            model.projects.live.take(.failed(.off))
            XCTAssertFalse(model.isOnline, "A stale status response must not hide the current live failure")
            model.projects.live.take(.snapshot(fixture.live))
            model.setupMessage = "Synthetic read failure"
            XCTAssertFalse(model.isOnline)
            model.setupMessage = nil
            XCTAssertTrue(model.isOnline)
        }
        XCTAssertEqual(clock.starts, 0)
        XCTAssertEqual(memories.count, 0)
    }
}
