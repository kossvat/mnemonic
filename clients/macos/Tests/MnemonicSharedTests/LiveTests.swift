import Foundation
import MnemonicShared
import XCTest
import notify

/// The one way both clients ask for the live snapshot and listen for its
/// signal: what is asked, when, and what is not. The server, the clock
/// and the token are stand-ins the tests count on.
final class LiveTests: XCTestCase {
    /// The snapshot the daemon's test compares with what it serves.
    static let fixtureURL = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Fixtures/live.json")

    private func fixture() throws -> Data { try Data(contentsOf: Self.fixtureURL) }

    private func snapshot() throws -> LiveSnapshot {
        try JSONDecoder().decode(LiveSnapshot.self, from: fixture())
    }

    /// The fixture with nobody live in it.
    private func quiet() throws -> LiveSnapshot {
        let empty = """
            {"boot":"b","v":9,"projects":[],"more_projects":0,
             "unknown":{"sessions":[],"more_sessions":0},"today":{"owner_s":5,"agent_s":0}}
            """
        return try JSONDecoder().decode(LiveSnapshot.self, from: Data(empty.utf8))
    }

    // MARK: The models

    func testTheModelsDecodeTheSnapshotTheDaemonServes() throws {
        let live = try snapshot()
        XCTAssertEqual(live.boot, "0123456789abcdef")
        XCTAssertEqual(live.projects.map(\.key), ["demoapp", "notes"])
        let first = live.projects[0]
        XCTAssertEqual(first.state, .running)
        XCTAssertEqual(first.sessions.map(\.agent), ["claude", "codex"])
        XCTAssertEqual(first.sessions.map(\.driver), ["owner", "program"])
        XCTAssertEqual(first.sessions[1].started.timeIntervalSince1970, 1_790_762_405)
        XCTAssertEqual(
            first.today, LiveToday(ownerSeconds: 5400, agentSeconds: 9000, confidence: "high"))
        XCTAssertEqual(
            first.plan,
            LivePlan(
                unit: "a change", done: 3, total: 7, stage: "building",
                task: "the fourth task", gate: "stale"))
        XCTAssertEqual(live.projects[1].state, .waiting)
        XCTAssertNil(live.projects[1].plan)
        XCTAssertNil(live.projects[1].today.confidence)
        XCTAssertEqual(live.unknown.sessions.count, 1)
        XCTAssertEqual(live.today, LiveTotal(ownerSeconds: 6000, agentSeconds: 10_800))
        XCTAssertTrue(live.hasLiveSession)
        XCTAssertFalse(try quiet().hasLiveSession)
    }

    // MARK: The request actor

    /// Answers requests from a script, and keeps what was asked.
    final class Server: LocalTransport, @unchecked Sendable {
        private let lock = NSLock()
        private var script: [Result<LocalResponse, Error>]
        private(set) var requests: [URLRequest] = []

        init(_ script: [Result<LocalResponse, Error>]) { self.script = script }

        func send(_ request: URLRequest) async throws -> LocalResponse {
            try lock.withLock {
                requests.append(request)
                return try script.isEmpty
                    ? LocalResponse(status: 500, tag: nil, body: Data())
                    : script.removeFirst().get()
            }
        }

        var asked: Int { lock.withLock { requests.count } }
        func header(_ name: String, of index: Int) -> String? {
            lock.withLock { requests[index].value(forHTTPHeaderField: name) }
        }
    }

    /// Counts how often the token is read.
    final class Tokens: @unchecked Sendable {
        private let lock = NSLock()
        private var values: [String]
        private(set) var reads = 0
        init(_ values: [String]) { self.values = values }
        func next() -> String? {
            lock.withLock {
                reads += 1
                return values.count > 1 ? values.removeFirst() : values.first
            }
        }
    }

    private func api(_ server: Server, tokens: Tokens = Tokens(["t"]), enabled: Bool = true)
        -> LocalAPI
    {
        LocalAPI(
            endpoint: URL(string: "http://localhost:3737")!,
            enabled: { enabled },
            token: { tokens.next() },
            transport: server)
    }

    private func ok(_ body: Data, tag: String) -> Result<LocalResponse, Error> {
        .success(LocalResponse(status: 200, tag: tag, body: body))
    }

    private func status(_ code: Int) -> Result<LocalResponse, Error> {
        .success(LocalResponse(status: code, tag: nil, body: Data()))
    }

    func testTheActorSendsTheTagItHoldsAndKeepsItsValueOnA304() async throws {
        let server = Server([ok(try fixture(), tag: "\"b-6\""), status(304), status(304)])
        let api = api(server)
        let first = await api.live()
        XCTAssertEqual(first, .snapshot(try snapshot()))
        // The first request holds no snapshot, so it sends no tag.
        XCTAssertNil(server.header("If-None-Match", of: 0))
        XCTAssertEqual(server.header(LocalAPI.tokenHeader, of: 0), "t")
        XCTAssertEqual(server.requests[0].url?.path, "/api/live")

        let second = await api.live()
        XCTAssertEqual(second, .unchanged)
        XCTAssertEqual(server.header("If-None-Match", of: 1), "\"b-6\"")
        let held = await api.snapshot()
        XCTAssertEqual(held, try snapshot())
        let third = await api.live()
        XCTAssertEqual(third, .unchanged)
        XCTAssertEqual(server.asked, 3)
    }

    func testTheFiveStatesOfTheLocalAPIAreToldApart() async throws {
        struct Refused: Error {}
        // Off by configuration: nothing is asked at all.
        let unasked = Server([])
        let off = await api(unasked, enabled: false).live()
        XCTAssertEqual(off, .failed(.off))
        XCTAssertEqual(unasked.asked, 0)
        // A refused connection.
        let refused = await api(Server([.failure(Refused())])).live()
        XCTAssertEqual(refused, .failed(.unreachable))
        // Not authorised, also with the token read again.
        let closed = Server([status(401), status(401)])
        let unauthorised = await api(closed).live()
        XCTAssertEqual(unauthorised, .failed(.unauthorised))
        XCTAssertEqual(closed.asked, 2)
        // A daemon of an earlier version has no live route.
        let earlier = await api(Server([status(404)])).live()
        XCTAssertEqual(earlier, .failed(.earlierDaemon))
        // An answer that cannot be decoded.
        let garbled = await api(Server([ok(Data("{\"boot\":1}".utf8), tag: "x")])).live()
        XCTAssertEqual(garbled, .failed(.undecodable))
        // A 304 for a tag that was never held is no answer either.
        let unheld = await api(Server([status(304)])).live()
        XCTAssertEqual(unheld, .failed(.undecodable))
    }

    func testARotatedTokenIsReadOnceOnA401() async throws {
        let server = Server([status(401), ok(try fixture(), tag: "t1"), status(304)])
        let tokens = Tokens(["old", "new"])
        let api = api(server, tokens: tokens)
        let answer = await api.live()
        XCTAssertEqual(answer, .snapshot(try snapshot()))
        XCTAssertEqual(server.header(LocalAPI.tokenHeader, of: 0), "old")
        XCTAssertEqual(server.header(LocalAPI.tokenHeader, of: 1), "new")
        // Read at the first request and once more on the 401, and not
        // again while it is accepted.
        XCTAssertEqual(tokens.reads, 2)
        _ = await api.live()
        XCTAssertEqual(tokens.reads, 2)
        XCTAssertEqual(server.header(LocalAPI.tokenHeader, of: 2), "new")
    }

    // MARK: The store

    @MainActor
    func testTheStorePublishesOnlyWhatDiffersAndOnlyWhenVisible() throws {
        let store = LiveStore()
        // Closed: the answer is kept, and nothing is published.
        store.take(.snapshot(try snapshot()))
        XCTAssertNil(store.shown)
        XCTAssertEqual(store.publications, 0)
        XCTAssertTrue(store.hasLiveSession)
        // Opened: what is held is shown, once.
        store.visible = true
        XCTAssertEqual(store.shown, try snapshot())
        XCTAssertEqual(store.publications, 1)
        // The same snapshot again, and "nothing changed": no publication.
        store.take(.snapshot(try snapshot()))
        store.take(.unchanged)
        XCTAssertEqual(store.publications, 1)
        // Another snapshot is published.
        store.take(.snapshot(try quiet()))
        XCTAssertEqual(store.shown, try quiet())
        XCTAssertEqual(store.publications, 2)
        XCTAssertFalse(store.hasLiveSession)
        // A daemon that is gone: the reason is shown, and nobody is live.
        store.take(.snapshot(try snapshot()))
        store.take(.failed(.unreachable))
        XCTAssertEqual(store.failure, .unreachable)
        XCTAssertNil(store.shown)
        XCTAssertFalse(store.hasLiveSession)
        // It answers again: the reason goes.
        store.take(.snapshot(try snapshot()))
        XCTAssertNil(store.failure)
        // Closed again: what comes is kept and not published.
        store.visible = false
        let before = store.publications
        store.take(.snapshot(try quiet()))
        XCTAssertEqual(store.publications, before)
        XCTAssertEqual(store.shown, try snapshot())
    }

    // MARK: The signal

    private func profile(_ name: String) throws -> MnemonicProfile {
        let home = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-live-\(UUID().uuidString.prefix(8))-\(name)")
        addTeardownBlock { try? FileManager.default.removeItem(at: home) }
        return try MnemonicProfile.resolve(
            environment: [MnemonicProfile.homeVariable: home.path],
            arguments: [],
            userHome: URL(fileURLWithPath: "/nonexistent-user-home"))
    }

    func testTheSubscriberHearsItsOwnProfilesSignalAndNotAnothers() throws {
        let mine = try LiveSignal.name(of: try profile("mine"))
        let theirs = try LiveSignal.name(of: try profile("theirs"))
        XCTAssertNotEqual(mine, theirs)
        let heard = expectation(description: "the signal of this profile")
        let subscriber = try XCTUnwrap(
            LiveSignalSubscriber(name: mine) { heard.fulfill() })
        // The other profile posts first: it must not wake this client.
        notify_post(theirs)
        notify_post(mine)
        wait(for: [heard], timeout: 5)
        subscriber.cancel()
    }

    func testAClientThatStartsBeforeTheDaemonHearsItsFirstSignal() throws {
        // No daemon of this profile ever ran: the client makes the
        // identity and listens under its name.
        let profile = try profile("first")
        XCTAssertFalse(FileManager.default.fileExists(atPath: profile.identityURL.path))
        let name = try LiveSignal.name(of: profile)
        let heard = expectation(description: "the first signal of the daemon")
        let subscriber = try XCTUnwrap(
            LiveSignalSubscriber(name: name) { heard.fulfill() })
        // The daemon starts, reads the same file, and posts under the
        // name that value gives.
        let read = try String(contentsOf: profile.identityURL, encoding: .utf8)
        XCTAssertEqual(LiveSignal.name(identity: read), name)
        XCTAssertEqual(name, "mnemonic.live.\(read)")
        notify_post(LiveSignal.name(identity: read))
        wait(for: [heard], timeout: 5)
        subscriber.cancel()
    }

    // MARK: The coordinator

    /// A clock the test holds: timers fire when the test says so.
    @MainActor
    final class HeldClock: LiveClock {
        final class Held: LiveTimer {
            let seconds: TimeInterval
            let tolerance: TimeInterval
            var fire: (@MainActor () -> Void)?
            init(_ seconds: TimeInterval, _ tolerance: TimeInterval, _ fire: @escaping @MainActor () -> Void) {
                self.seconds = seconds
                self.tolerance = tolerance
                self.fire = fire
            }
            func cancel() { fire = nil }
        }

        private(set) var timers: [Held] = []

        func arm(
            after seconds: TimeInterval, tolerance: TimeInterval,
            _ fire: @escaping @MainActor () -> Void
        ) -> LiveTimer {
            let held = Held(seconds, tolerance, fire)
            timers.append(held)
            return held
        }

        /// The timers that are armed and not cancelled, by their pace.
        var live: [TimeInterval] { timers.filter { held in held.fire != nil }.map(\.seconds).sorted() }

        /// Fire the armed timer of this pace.
        func fire(_ seconds: TimeInterval) {
            guard let held = timers.first(where: { held in held.seconds == seconds && held.fire != nil })
            else { return XCTFail("no timer of \(seconds) s is armed") }
            let fire = held.fire
            held.fire = nil
            fire?()
        }
    }

    /// Answers of the daemon, by a script, and how often it was asked.
    final class Daemon: @unchecked Sendable {
        private let lock = NSLock()
        private var answers: [LiveAnswer]
        private(set) var asked = 0
        init(_ answers: [LiveAnswer]) { self.answers = answers }
        func answer() -> LiveAnswer {
            lock.withLock {
                asked += 1
                return answers.count > 1 ? answers.removeFirst() : answers[0]
            }
        }
    }

    @MainActor
    func testTheCoordinatorHoldsOnlyTheTimersItsStateCallsFor() async throws {
        let live = try snapshot()
        let nobody = try quiet()
        let daemon = Daemon([.snapshot(nobody), .snapshot(live), .unchanged, .snapshot(nobody)])
        let clock = HeldClock()
        let store = LiveStore()
        let coordinator = LiveCoordinator(store: store, clock: clock) { daemon.answer() }

        // Everything closed and nothing asked: no timer at all.
        XCTAssertEqual(coordinator.armed, [])
        // A signal while closed: one request; nobody is live, so still no
        // timer, and nothing is published to a closed surface.
        await coordinator.signalled()
        XCTAssertEqual(daemon.asked, 1)
        XCTAssertEqual(coordinator.armed, [])
        XCTAssertEqual(store.publications, 0)

        // A signal brings a live session: the safety request is armed,
        // with the popover closed, and nothing else.
        await coordinator.signalled()
        XCTAssertEqual(daemon.asked, 2)
        XCTAssertEqual(coordinator.armed, ["safety"])
        XCTAssertEqual(clock.live, [300])
        XCTAssertEqual(clock.timers.last?.tolerance, 60)
        XCTAssertEqual(store.publications, 0)

        // The popover opens: it asks once, shows what is held, and the
        // elapsed labels tick while a session is live.
        await coordinator.setSurfaceVisible(true)
        XCTAssertEqual(daemon.asked, 3)
        XCTAssertEqual(store.shown, live)
        XCTAssertEqual(coordinator.armed, ["safety", "labels"])
        var ticks = 0
        coordinator.onLabelsTick = { ticks += 1 }
        clock.fire(60)
        XCTAssertEqual(ticks, 1)
        // The tick asks nothing, and is armed again.
        XCTAssertEqual(daemon.asked, 3)
        XCTAssertEqual(coordinator.armed, ["safety", "labels"])

        // A plan surface is shown: the refresh is armed, only now.
        coordinator.setPlanVisible(true)
        XCTAssertEqual(coordinator.armed, ["safety", "labels", "refresh"])
        XCTAssertEqual(clock.live, [30, 60, 300])
        coordinator.setPlanVisible(false)
        XCTAssertEqual(coordinator.armed, ["safety", "labels"])

        // The popover closes: only the safety request stays.
        await coordinator.setSurfaceVisible(false)
        XCTAssertEqual(coordinator.armed, ["safety"])
        XCTAssertEqual(daemon.asked, 3)

        // The safety request finds nobody live: it is not armed again,
        // and the client holds no timer.
        clock.fire(300)
        await Task.yield()
        for _ in 0..<50 where daemon.asked < 4 { await Task.yield() }
        XCTAssertEqual(daemon.asked, 4)
        for _ in 0..<50 where !coordinator.armed.isEmpty { await Task.yield() }
        XCTAssertEqual(coordinator.armed, [])
        XCTAssertEqual(clock.live, [])
    }

    @MainActor
    func testAPlanSurfaceRefreshesWhileItIsVisibleAndADeadDaemonLeavesNobodyLive() async throws {
        let live = try snapshot()
        let daemon = Daemon([.snapshot(live), .unchanged, .failed(.unreachable)])
        let clock = HeldClock()
        let store = LiveStore()
        let coordinator = LiveCoordinator(store: store, clock: clock) { daemon.answer() }
        await coordinator.setSurfaceVisible(true)
        coordinator.setPlanVisible(true)
        XCTAssertEqual(daemon.asked, 1)
        // The refresh asks, and is armed again while the plan is shown.
        clock.fire(30)
        for _ in 0..<50 where daemon.asked < 2 { await Task.yield() }
        XCTAssertEqual(daemon.asked, 2)
        for _ in 0..<50 where !coordinator.armed.contains("refresh") { await Task.yield() }
        XCTAssertEqual(coordinator.armed, ["safety", "labels", "refresh"])
        XCTAssertEqual(store.shown, live)

        // The daemon dies and posts nothing. The next request finds it
        // gone: the project does not stay live on screen, and with
        // nobody live the safety request and the labels are let go.
        clock.fire(30)
        for _ in 0..<50 where daemon.asked < 3 { await Task.yield() }
        for _ in 0..<50 where store.failure == nil { await Task.yield() }
        XCTAssertEqual(store.failure, .unreachable)
        XCTAssertNil(store.shown)
        for _ in 0..<50 where coordinator.armed.contains("safety") { await Task.yield() }
        XCTAssertEqual(coordinator.armed, ["refresh"])
    }
}

/// Whether the local API is on, as the profile's configuration says it.
final class LocalAPISwitchTests: XCTestCase {
    private func profile(config: String?) throws -> MnemonicProfile {
        let home = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-switch-\(UUID().uuidString.prefix(8))")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: home) }
        if let config {
            try config.write(
                to: home.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
        }
        return try MnemonicProfile.resolve(
            environment: [MnemonicProfile.homeVariable: home.path],
            arguments: [],
            userHome: URL(fileURLWithPath: "/nonexistent-user-home"))
    }

    func testTheLocalAPIIsOnOnlyWhereTheConfigurationSaysSo() throws {
        XCTAssertTrue(try profile(config: "[ui]\nenabled = true\nport = 3737\n").localAPIEnabled())
        XCTAssertTrue(
            try profile(config: "[daemon]\nx = 1\n\n[ui] # the dashboard\nenabled = true # on\n")
                .localAPIEnabled())
        // Off, left out, in another table, or with no file: off, as the
        // daemon takes it.
        XCTAssertFalse(try profile(config: "[ui]\nenabled = false\n").localAPIEnabled())
        XCTAssertFalse(try profile(config: "[ui]\nport = 3737\n").localAPIEnabled())
        XCTAssertFalse(try profile(config: "[activity]\nenabled = true\n").localAPIEnabled())
        XCTAssertFalse(try profile(config: nil).localAPIEnabled())
    }
}

/// A daemon that was out of reach and came back with the snapshot it had.
final class LiveRecoveryTests: XCTestCase {
    @MainActor
    func testAfterAFailureTheSnapshotIsAskedForWholeAndShownAgain() async throws {
        struct Refused: Error {}
        let body = try Data(contentsOf: LiveTests.fixtureURL)
        let snapshot = try JSONDecoder().decode(LiveSnapshot.self, from: body)
        let whole = Result<LocalResponse, Error>.success(
            LocalResponse(status: 200, tag: "\"b-6\"", body: body))
        // The daemon answers, then a request times out, then it answers
        // again: its snapshot did not change in between.
        let server = LiveTests.Server([whole, .failure(Refused()), whole])
        let api = LocalAPI(
            endpoint: URL(string: "http://localhost:3737")!,
            enabled: { true }, token: { "t" }, transport: server)
        let store = LiveStore()
        store.visible = true
        store.take(await api.live())
        XCTAssertEqual(store.shown, snapshot)
        store.take(await api.live())
        XCTAssertEqual(store.failure, .unreachable)
        XCTAssertNil(store.shown)
        // The next request carries no tag, so the answer is the snapshot
        // and not "nothing changed": the surface shows it again.
        store.take(await api.live())
        XCTAssertNil(server.header("If-None-Match", of: 2))
        XCTAssertEqual(store.shown, snapshot)
        XCTAssertNil(store.failure)
        XCTAssertTrue(store.hasLiveSession)
    }
}
