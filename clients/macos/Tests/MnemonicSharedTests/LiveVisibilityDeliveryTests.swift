import Foundation
import MnemonicShared
import XCTest

/// Held HTTP replies exercise the API's real ETag cache across visibility
/// changes. Extra requests answer immediately so a regression cannot hang.
final class LiveVisibilityDeliveryTests: XCTestCase {
    private actor Server: LocalTransport {
        let holds: Int
        private var asked: [URLRequest] = []
        private var pending: [Int: CheckedContinuation<LocalResponse, Never>] = [:]
        private var notices: [Int: @Sendable () -> Void] = [:]

        init(holds: Int) { self.holds = holds }

        func send(_ request: URLRequest) async throws -> LocalResponse {
            let index = asked.count
            asked.append(request)
            guard index < holds else {
                notices.removeValue(forKey: index)?()
                return LocalResponse(status: 304, tag: nil, body: Data())
            }
            return await withCheckedContinuation { continuation in
                pending[index] = continuation
                notices.removeValue(forKey: index)?()
            }
        }

        func notify(_ index: Int, _ notice: @escaping @Sendable () -> Void) {
            if asked.indices.contains(index) { notice() }
            else { notices[index] = notice }
        }

        func request(_ index: Int) -> URLRequest? {
            asked.indices.contains(index) ? asked[index] : nil
        }

        func finish(_ index: Int, status: Int = 304, body: Data = Data(), tag: String? = nil) {
            pending.removeValue(forKey: index)?.resume(
                returning: LocalResponse(status: status, tag: tag, body: body))
        }

        func finishAll() {
            let waiting = Array(pending.values)
            pending.removeAll()
            for continuation in waiting {
                continuation.resume(returning: LocalResponse(status: 500, tag: nil, body: Data()))
            }
        }

        var count: Int { asked.count }
    }

    private func fixture() throws -> Data {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/live.json")
        return try Data(contentsOf: url)
    }

    private func api(_ server: Server) -> LocalAPI {
        LocalAPI(endpoint: URL(string: "http://localhost:3737")!, enabled: { true },
                 token: { "fixture-token" }, transport: server)
    }

    @MainActor
    private func arrived(_ server: Server, _ index: Int) async throws -> URLRequest {
        let notice = expectation(description: "Live request \(index) arrived")
        await server.notify(index) { notice.fulfill() }
        await fulfillment(of: [notice], timeout: 5)
        let request = await server.request(index)
        return try XCTUnwrap(request)
    }

    @MainActor
    private func mainQueueBarrier() async {
        let notice = expectation(description: "Queued main-actor delivery finished")
        DispatchQueue.main.async { notice.fulfill() }
        await fulfillment(of: [notice], timeout: 5)
    }

    @MainActor
    func testOldReplyAfterReopeningWaitsForTheCurrent304BeforePublishing() async throws {
        let body = try fixture()
        let snapshot = try JSONDecoder().decode(LiveSnapshot.self, from: body)
        let server = Server(holds: 2)
        defer { Task { await server.finishAll() } }
        let api = api(server)
        let store = LiveStore()
        let coordinator = LiveCoordinator(store: store, clock: LiveTests.HeldClock(),
                                          allowHiddenRequests: false) { await api.live() }
        var answers: [LiveAnswer] = []
        coordinator.onAnswer = { answer in answers.append(answer) }
        let opening = Task { await coordinator.setSurfaceVisible(true) }
        _ = try await arrived(server, 0)
        coordinator.hide()
        await coordinator.setSurfaceVisible(true)
        await server.finish(0, status: 200, body: body, tag: "held-tag")
        let fresh = try await arrived(server, 1)
        XCTAssertEqual(fresh.value(forHTTPHeaderField: "If-None-Match"), "held-tag")
        XCTAssertEqual(store.snapshot, snapshot)
        XCTAssertNil(store.shown)
        XCTAssertEqual(store.publications, 0)
        XCTAssertTrue(answers.isEmpty)
        await server.finish(1)
        await opening.value
        XCTAssertEqual(store.shown, snapshot)
        XCTAssertEqual(store.publications, 1)
        XCTAssertEqual(answers, [.unchanged])
        let count = await server.count
        XCTAssertEqual(count, 2)
        coordinator.hide()
    }

    @MainActor
    func testOldReplyWhileHiddenAlsoWaitsForConfirmationAfterReopening() async throws {
        let body = try fixture()
        let snapshot = try JSONDecoder().decode(LiveSnapshot.self, from: body)
        let server = Server(holds: 2)
        defer { Task { await server.finishAll() } }
        let api = api(server)
        let store = LiveStore()
        let coordinator = LiveCoordinator(store: store, clock: LiveTests.HeldClock(),
                                          allowHiddenRequests: false) { await api.live() }
        var answers = 0
        coordinator.onAnswer = { _ in answers += 1 }
        let oldOpening = Task { await coordinator.setSurfaceVisible(true) }
        _ = try await arrived(server, 0)
        coordinator.hide()
        await server.finish(0, status: 200, body: body, tag: "held-tag")
        await oldOpening.value
        XCTAssertEqual(store.snapshot, snapshot)
        XCTAssertNil(store.shown)
        let reopening = Task { await coordinator.setSurfaceVisible(true) }
        let fresh = try await arrived(server, 1)
        XCTAssertEqual(fresh.value(forHTTPHeaderField: "If-None-Match"), "held-tag")
        XCTAssertNil(store.shown)
        XCTAssertEqual(store.publications, 0)
        XCTAssertEqual(answers, 0)
        await server.finish(1)
        await reopening.value
        XCTAssertEqual(store.shown, snapshot)
        XCTAssertEqual(answers, 1)
        coordinator.hide()
    }

    @MainActor
    func testDeliveredRefreshTaskCannotLeakIntoAReopenedVisit() async throws {
        let body = try fixture()
        let server = Server(holds: 3)
        defer { Task { await server.finishAll() } }
        let api = api(server)
        let clock = LiveTimerDeliveryTests.Clock()
        let coordinator = LiveCoordinator(store: LiveStore(), clock: clock,
                                          allowHiddenRequests: false) { await api.live() }
        let opening = Task { await coordinator.setSurfaceVisible(true) }
        _ = try await arrived(server, 0)
        await server.finish(0, status: 200, body: body, tag: "live-tag")
        await opening.value
        coordinator.setPlanVisible(true)
        var refreshes = 0
        coordinator.onRefreshTick = { refreshes += 1 }
        let oldRequest = Task { await coordinator.request() }
        _ = try await arrived(server, 1)
        let refresh = try clock.queue(30)
        refresh()
        coordinator.hide()
        // A request is already held, so reopening coalesces synchronously
        // and the new plan is visible before the queued timer task runs.
        await coordinator.setSurfaceVisible(true)
        coordinator.setPlanVisible(true)
        await mainQueueBarrier()
        XCTAssertEqual(refreshes, 0)
        await server.finish(1)
        _ = try await arrived(server, 2)
        await server.finish(2)
        await oldRequest.value
        await mainQueueBarrier()
        XCTAssertEqual(refreshes, 0)
        let count = await server.count
        XCTAssertEqual(count, 3)
        XCTAssertEqual(clock.live, [30, 60, 300])
        coordinator.hide()
    }

    @MainActor
    func testDeliveredAppSafetyTaskCannotQueueAnExtraRequestAfterReopening() async throws {
        let body = try fixture()
        let server = Server(holds: 2)
        defer { Task { await server.finishAll() } }
        let api = api(server)
        let clock = LiveTimerDeliveryTests.Clock()
        let coordinator = LiveCoordinator(store: LiveStore(), clock: clock,
                                          allowHiddenRequests: false) { await api.live() }
        let opening = Task { await coordinator.setSurfaceVisible(true) }
        _ = try await arrived(server, 0)
        await server.finish(0, status: 200, body: body, tag: "live-tag")
        await opening.value
        let safety = try clock.queue(300)
        safety()
        coordinator.hide()
        let release = Task {
            _ = try await arrived(server, 1)
            await mainQueueBarrier()
            await server.finish(1)
        }
        await coordinator.setSurfaceVisible(true)
        try await release.value
        await mainQueueBarrier()
        let count = await server.count
        XCTAssertEqual(count, 2)
        XCTAssertEqual(clock.live, [60, 300])
        coordinator.hide()
    }

    @MainActor
    func testWidgetStillAcceptsHeldRepliesAndSafetyRequestsWhileClosed() async throws {
        let body = try fixture()
        let snapshot = try JSONDecoder().decode(LiveSnapshot.self, from: body)
        let server = Server(holds: 2)
        defer { Task { await server.finishAll() } }
        let api = api(server)
        let clock = LiveTimerDeliveryTests.Clock()
        let store = LiveStore()
        let coordinator = LiveCoordinator(store: store, clock: clock) { await api.live() }
        var answers: [LiveAnswer] = []
        coordinator.onAnswer = { answer in answers.append(answer) }
        let opening = Task { await coordinator.setSurfaceVisible(true) }
        _ = try await arrived(server, 0)
        coordinator.hide()
        await server.finish(0, status: 200, body: body, tag: "live-tag")
        await opening.value
        XCTAssertEqual(store.snapshot, snapshot)
        XCTAssertNil(store.shown)
        XCTAssertEqual(answers, [.snapshot(snapshot)])
        let safety = try clock.queue(300)
        let safetyAnswered = expectation(description: "Closed widget accepted the safety answer")
        coordinator.onAnswer = { answer in
            answers.append(answer)
            if answers.count == 2 { safetyAnswered.fulfill() }
        }
        safety()
        coordinator.hide()
        _ = try await arrived(server, 1)
        await server.finish(1)
        await fulfillment(of: [safetyAnswered], timeout: 5)
        XCTAssertEqual(answers, [.snapshot(snapshot), .unchanged])
        XCTAssertEqual(store.publications, 0)
        XCTAssertEqual(clock.live, [300])
    }
}
