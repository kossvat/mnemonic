import Foundation
import MnemonicShared
import XCTest

/// All requests stay in these stand-ins. Profiles use synthetic homes and
/// the only files read are the daemon's checked-in response fixtures.
final class ProjectAPITests: XCTestCase {
    private struct Refused: Error {}

    private var loopbackHost: String {
        String(cString: inet_ntoa(in_addr(s_addr: INADDR_LOOPBACK.bigEndian)))
    }

    private var adjacentLoopbackHost: String {
        String(cString: inet_ntoa(in_addr(s_addr: (INADDR_LOOPBACK + 1).bigEndian)))
    }

    private final class Server: LocalTransport, @unchecked Sendable {
        private let lock = NSLock()
        private var script: [Result<LocalResponse, Error>]
        private var asked: [URLRequest] = []

        init(_ script: [Result<LocalResponse, Error>]) { self.script = script }

        func send(_ request: URLRequest) async throws -> LocalResponse {
            try lock.withLock {
                asked.append(request)
                guard !script.isEmpty else { throw Refused() }
                return try script.removeFirst().get()
            }
        }

        var requests: [URLRequest] { lock.withLock { asked } }
    }

    private final class Tokens: @unchecked Sendable {
        private let lock = NSLock()
        private var values: [String]
        private var count = 0

        init(_ values: [String] = ["fixture-token"]) { self.values = values }
        var reads: Int { lock.withLock { count } }

        func next() -> String? {
            lock.withLock {
                count += 1
                return values.count > 1 ? values.removeFirst() : values.first
            }
        }
    }

    private actor HeldServer: LocalTransport {
        private var requests: [URLRequest] = []
        private var pending: [Int: CheckedContinuation<LocalResponse, Error>] = [:]
        private var waiters: [Int: CheckedContinuation<URLRequest, Never>] = [:]

        func send(_ request: URLRequest) async throws -> LocalResponse {
            let index = requests.count
            requests.append(request)
            return try await withCheckedThrowingContinuation { continuation in
                pending[index] = continuation
                waiters.removeValue(forKey: index)?.resume(returning: request)
            }
        }

        func arrived(_ index: Int) async -> URLRequest {
            if requests.indices.contains(index) { return requests[index] }
            return await withCheckedContinuation { continuation in
                waiters[index] = continuation
            }
        }

        func finish(_ index: Int, with result: Result<LocalResponse, Error>) {
            pending.removeValue(forKey: index)?.resume(with: result)
        }

        var count: Int { requests.count }
    }

    private func fixture(_ name: String) throws -> Data {
        let url = URL(fileURLWithPath: #filePath)
            .deletingLastPathComponent().deletingLastPathComponent()
            .appendingPathComponent("Fixtures/\(name).json")
        return try Data(contentsOf: url)
    }

    private func response(_ status: Int, body: Data = Data(), tag: String? = nil)
        -> Result<LocalResponse, Error>
    {
        .success(LocalResponse(status: status, tag: tag, body: body))
    }

    private func api(
        _ server: LocalTransport, tokens: Tokens = Tokens(), enabled: Bool = true,
        endpoint: String = "http://localhost:3737", locked: Bool = false
    ) -> LocalAPI {
        LocalAPI(
            endpoint: URL(string: endpoint)!, enabled: { enabled },
            token: { tokens.next() }, transport: server, endpointLocked: locked)
    }

    private func profile(isolated: Bool = false, named: Bool = false) throws -> MnemonicProfile {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-api-fixture-\(UUID().uuidString)")
        var environment: [String: String] = [:]
        if isolated { environment[MnemonicProfile.homeVariable] = root.appendingPathComponent("store").path }
        if named { environment[MnemonicProfile.endpointVariable] = "http://localhost:3737" }
        return try MnemonicProfile.resolve(
            environment: environment, arguments: [], userHome: root.appendingPathComponent("user"))
    }

    func testBoardAndDetailDecodeDaemonFixturesWithOneRequestEach() async throws {
        let boardData = try fixture("project-board")
        let detailData = try fixture("project-detail")
        let board = try JSONDecoder().decode(ProjectBoard.self, from: boardData)
        let detail = try JSONDecoder().decode(ProjectDetail.self, from: detailData)
        let tokens = Tokens()
        let server = Server([response(200, body: boardData), response(200, body: detailData)])
        let api = api(server, tokens: tokens)
        let boardAnswer = await api.projects()
        let detailAnswer = await api.project(key: "demoapp")
        XCTAssertEqual(boardAnswer, .success(board))
        XCTAssertEqual(detailAnswer, .success(detail))
        XCTAssertEqual(board.projects.map(\.key), ["demoapp"])
        XCTAssertEqual(detail.plan.units.first?.tasks.count, 2)
        XCTAssertEqual(server.requests.map { request in request.url?.path },
                       ["/api/projects", "/api/projects/demoapp"])
        XCTAssertEqual(tokens.reads, 1)
        for request in server.requests {
            XCTAssertEqual(request.value(forHTTPHeaderField: LocalAPI.tokenHeader), "fixture-token")
            XCTAssertEqual(request.timeoutInterval, 5)
            XCTAssertEqual(request.cachePolicy, .reloadIgnoringLocalCacheData)
            XCTAssertNil(request.value(forHTTPHeaderField: "If-None-Match"))
        }
        let held = await api.snapshot()
        XCTAssertNil(held)
        let configuration = try profile().sessionConfiguration()
        XCTAssertNil(configuration.urlCache)
        XCTAssertEqual(configuration.requestCachePolicy, .reloadIgnoringLocalCacheData)
    }

    func testProjectReadsShareTokenRotationButNeverUseOrReplaceTheLiveTag() async throws {
        let live = try fixture("live")
        let server = Server([
            response(200, body: live, tag: "live-tag"), response(401),
            response(200, body: try fixture("project-board"), tag: "board-tag"),
            response(200, body: try fixture("project-detail"), tag: "detail-tag"), response(304),
        ])
        let tokens = Tokens(["first-token", "rotated-token"])
        let api = api(server, tokens: tokens)
        _ = await api.live()
        _ = await api.projects()
        _ = await api.project(key: "demoapp")
        let last = await api.live()
        XCTAssertEqual(last, .unchanged)
        XCTAssertEqual(tokens.reads, 2)
        XCTAssertEqual(server.requests.count, 5)
        XCTAssertEqual(server.requests[1].value(forHTTPHeaderField: LocalAPI.tokenHeader), "first-token")
        for index in 2...4 {
            XCTAssertEqual(server.requests[index].value(forHTTPHeaderField: LocalAPI.tokenHeader),
                           "rotated-token")
        }
        for index in 1...3 { XCTAssertNil(server.requests[index].value(forHTTPHeaderField: "If-None-Match")) }
        XCTAssertEqual(server.requests[4].value(forHTTPHeaderField: "If-None-Match"), "live-tag")
        let held = await api.snapshot()
        XCTAssertEqual(held, try JSONDecoder().decode(LiveSnapshot.self, from: live))
    }

    func testProjectFailuresKeepTheFiveExistingStatesAndRetryA401OnlyOnce() async throws {
        let tokens = Tokens()
        let unasked = Server([])
        let off = api(unasked, tokens: tokens, enabled: false)
        let offBoard = await off.projects()
        let offDetail = await off.project(key: "demoapp")
        XCTAssertEqual(offBoard, .failure(.off))
        XCTAssertEqual(offDetail, .failure(.off))
        XCTAssertEqual(unasked.requests.count, 0)
        XCTAssertEqual(tokens.reads, 0)
        let cases: [([Result<LocalResponse, Error>], LocalAPIFailure, Int)] = [
            ([.failure(Refused())], .unreachable, 1),
            ([response(401), response(401)], .unauthorised, 2),
            ([response(404)], .earlierDaemon, 1),
            ([response(200, body: Data("{broken".utf8))], .undecodable, 1),
            ([response(304)], .undecodable, 1),
            ([response(500)], .undecodable, 1),
        ]
        for (script, failure, count) in cases {
            for detail in [false, true] {
                let server = Server(script)
                let localTokens = Tokens()
                let api = api(server, tokens: localTokens)
                if detail {
                    let result = await api.project(key: "demoapp")
                    XCTAssertEqual(result, .failure(failure))
                } else {
                    let result = await api.projects()
                    XCTAssertEqual(result, .failure(failure))
                }
                XCTAssertEqual(server.requests.count, count)
                XCTAssertEqual(localTokens.reads, count)
            }
        }
    }

    func testProjectKeyIsValidatedAndEncodedAsOnePathComponent() async throws {
        let tokens = Tokens()
        let server = Server([])
        let api = api(server, tokens: tokens)
        for key in ["", ".", "..", "alpha/beta", "../alpha", "alpha\\beta", "alpha%2Fbeta",
                    "alpha?token=fixture", "alpha#fragment", "alpha beta", "alpha\n", "alpha\0",
                    "alpha::beta", "alpha:", ":alpha", "alpha--beta", "-alpha", "alpha-"] {
            let result = await api.project(key: key)
            XCTAssertEqual(result, .failure(.undecodable), key)
        }
        XCTAssertTrue(server.requests.isEmpty)
        XCTAssertEqual(tokens.reads, 0)
        let data = try fixture("project-detail")
        let keys = ["alpha:part-two", "café:étude-2", String(repeating: "a", count: 320)]
        let encodedServer = Server(keys.map { _ in response(200, body: data) })
        let encodedAPI = self.api(encodedServer)
        for key in keys { _ = await encodedAPI.project(key: key) }
        for (index, key) in keys.enumerated() {
            let url = try XCTUnwrap(encodedServer.requests[index].url)
            let parts = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
            XCTAssertEqual(parts.path, "/api/projects/\(key)")
            XCTAssertEqual(parts.percentEncodedPath.split(separator: "/").count, 3)
            XCTAssertNil(parts.query)
            XCTAssertNil(parts.fragment)
        }
        XCTAssertTrue(encodedServer.requests[0].url!.absoluteString.contains("%3A"))
        XCTAssertTrue(encodedServer.requests[1].url!.absoluteString.contains("%C3%A9"))
    }

    func testEndpointEditsAcceptOnlyLoopbackHTTPOrigins() async throws {
        let profile = try profile()
        let valid = [
            ("http://localhost:4444", "http://localhost:4444"),
            ("http://LOCALHOST:4444/", "http://localhost:4444"),
            ("http://\(loopbackHost)", "http://\(loopbackHost):80"),
            ("http://[::1]:4444", "http://[::1]:4444"),
        ]
        for (input, expected) in valid {
            XCTAssertEqual(try LocalAPI.validatedEndpoint(input, profile: profile).absoluteString, expected)
        }
        let server = Server([])
        let api = api(server)
        for value in ["", "localhost:3737", "https://localhost:3737", "http://remote:3737",
                      "http://\(adjacentLoopbackHost):3737", "http://localhost-remote:3737",
                      "http://localhost:0", "http://localhost:65536", "http://localhost:-1",
                      "http://fixture@localhost:3737", "http://fixture:password@localhost:3737",
                      "http://localhost:3737/api", "http://localhost:3737//",
                      "http://localhost:3737/%2F", "http://localhost:3737?",
                      "http://localhost:3737?token=fixture", "http://localhost:3737#",
                      "http://localhost:3737#part", " http://localhost:3737", "http://localhost:3737\n",
                      "http://%6cocalhost:3737", "http://localhost.:3737"] {
            XCTAssertThrowsError(try LocalAPI.validatedEndpoint(value, profile: profile), value)
            do {
                try await api.setEndpoint(value)
                XCTFail("accepted invalid endpoint: \(value)")
            } catch {
                XCTAssertEqual(error as? LocalEndpointFailure, .invalid, value)
            }
        }
        let unchanged = await api.currentEndpoint()
        XCTAssertEqual(unchanged?.absoluteString, "http://localhost:3737")
        try await api.setEndpoint("http://\(loopbackHost):4444/")
        let changed = await api.currentEndpoint()
        XCTAssertEqual(changed?.absoluteString, "http://\(loopbackHost):4444")
        XCTAssertTrue(server.requests.isEmpty)
    }

    func testIsolatedAndExplicitProfilesKeepTheirEndpointAndTransportPolicy() async throws {
        for selected in [try profile(isolated: true), try profile(named: true)] {
            let original = selected.endpoint
            XCTAssertEqual(try LocalAPI.validatedEndpoint(original.absoluteString, profile: selected), original)
            XCTAssertThrowsError(try LocalAPI.validatedEndpoint("http://localhost:4444", profile: selected)) {
                error in XCTAssertEqual(error as? LocalEndpointFailure, .fixedProfile)
            }
            XCTAssertThrowsError(try LocalAPI(profile: selected, endpoint: URL(string: "http://localhost:4444")!))
            let local = try LocalAPI(profile: selected, endpoint: original)
            let current = await local.currentEndpoint()
            XCTAssertEqual(current, original)
            do {
                try await local.setEndpoint("http://localhost:4444")
                XCTFail("changed a fixed profile endpoint")
            } catch {
                XCTAssertEqual(error as? LocalEndpointFailure, .fixedProfile)
            }
            XCTAssertNil(selected.sessionConfiguration().urlCache)
        }
        let server = Server([])
        let locked = api(server, locked: true)
        try await locked.setEndpoint("http://LOCALHOST:3737/")
        do {
            try await locked.setEndpoint("http://localhost:4444")
            XCTFail("changed a fixed injected endpoint")
        } catch { XCTAssertEqual(error as? LocalEndpointFailure, .fixedProfile) }
        XCTAssertTrue(server.requests.isEmpty)
    }

    func testAnInvalidInjectedEndpointCannotReadATokenOrMakeARequest() async {
        let server = Server([])
        let tokens = Tokens()
        let api = api(server, tokens: tokens, endpoint: "http://remote:3737")
        let live = await api.live()
        let projects = await api.projects()
        XCTAssertEqual(live, .failed(.unreachable))
        XCTAssertEqual(projects, .failure(.unreachable))
        XCTAssertEqual(tokens.reads, 0)
        XCTAssertTrue(server.requests.isEmpty)
    }

    func testOldEndpointRepliesCannotReplaceOrClearNewLiveStateOrRetryA401() async throws {
        let oldData = try fixture("live")
        let newData = Data("""
            {"boot":"new","v":1,"projects":[],"more_projects":0,
             "unknown":{"sessions":[],"more_sessions":0},"today":{"owner_s":0,"agent_s":0}}
            """.utf8)
        let newSnapshot = try JSONDecoder().decode(LiveSnapshot.self, from: newData)
        let endings: [Result<LocalResponse, Error>] = [
            response(200, body: oldData, tag: "old-tag"), response(401), .failure(Refused()),
        ]
        for ending in endings {
            let server = HeldServer()
            let tokens = Tokens(["old-token", "new-token"])
            let api = api(server, tokens: tokens)
            let old = Task { await api.live() }
            let oldRequest = await server.arrived(0)
            XCTAssertEqual(oldRequest.url?.port, 3737)
            try await api.setEndpoint("http://localhost:4444")
            let cleared = await api.snapshot()
            XCTAssertNil(cleared)
            let new = Task { await api.live() }
            let newRequest = await server.arrived(1)
            XCTAssertEqual(newRequest.url?.port, 4444)
            XCTAssertNil(newRequest.value(forHTTPHeaderField: "If-None-Match"))
            XCTAssertEqual(newRequest.value(forHTTPHeaderField: LocalAPI.tokenHeader), "new-token")
            await server.finish(1, with: response(200, body: newData, tag: "new-tag"))
            let newAnswer = await new.value
            XCTAssertEqual(newAnswer, .snapshot(newSnapshot))
            await server.finish(0, with: ending)
            let oldAnswer = await old.value
            XCTAssertEqual(oldAnswer, .failed(.unreachable))
            let held = await api.snapshot()
            XCTAssertEqual(held, newSnapshot)
            let requestCount = await server.count
            XCTAssertEqual(requestCount, 2)
            XCTAssertEqual(tokens.reads, 2)
            let next = Task { await api.live() }
            let nextRequest = await server.arrived(2)
            XCTAssertEqual(nextRequest.url?.port, 4444)
            XCTAssertEqual(nextRequest.value(forHTTPHeaderField: "If-None-Match"), "new-tag")
            await server.finish(2, with: response(304))
            let nextAnswer = await next.value
            XCTAssertEqual(nextAnswer, .unchanged)
        }
    }
}
