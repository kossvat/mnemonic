import Foundation

/// Why the local API gave no answer. Each has its own words on screen.
public enum LocalAPIFailure: Error, Equatable, Sendable {
    /// The API is off in the configuration of the profile.
    case off
    /// Nothing listens: the daemon is not running.
    case unreachable
    /// The token was refused, also after it was read again.
    case unauthorised
    /// The live route is not there: the daemon is of an earlier version.
    case earlierDaemon
    /// An answer that cannot be decoded.
    case undecodable
}

/// What a request for the live snapshot gave.
public enum LiveAnswer: Equatable, Sendable {
    case snapshot(LiveSnapshot)
    /// The snapshot held is still the one served.
    case unchanged
    case failed(LocalAPIFailure)
}

/// What an answer of the server holds for the actor.
public struct LocalResponse: Sendable {
    public let status: Int
    public let tag: String?
    public let body: Data

    public init(status: Int, tag: String?, body: Data) {
        self.status = status
        self.tag = tag
        self.body = body
    }
}

/// What carries a request. The tests stand in for it and count.
public protocol LocalTransport: Sendable {
    func send(_ request: URLRequest) async throws -> LocalResponse
}

/// Requests through a session that keeps no response on disk.
public struct SessionTransport: LocalTransport {
    private let session: URLSession

    public init(profile: MnemonicProfile) {
        session = URLSession(
            configuration: profile.sessionConfiguration(),
            delegate: LocalRedirectRefusal(), delegateQueue: nil)
    }

    public func send(_ request: URLRequest) async throws -> LocalResponse {
        let (body, response) = try await session.data(for: request)
        let http = response as? HTTPURLResponse
        return LocalResponse(
            status: http?.statusCode ?? 0,
            tag: http?.value(forHTTPHeaderField: "ETag"),
            body: body)
    }
}

/// Redirects must not move a request carrying the profile token elsewhere.
private final class LocalRedirectRefusal: NSObject, URLSessionTaskDelegate, @unchecked Sendable {
    func urlSession(
        _ session: URLSession, task: URLSessionTask,
        willPerformHTTPRedirection response: HTTPURLResponse,
        newRequest request: URLRequest,
        completionHandler: @escaping (URLRequest?) -> Void
    ) {
        completionHandler(nil)
    }
}

/// The shared profile transport. Live and bounded graph scopes keep their
/// entity tags in memory; project reads return their decoded value directly.
public actor LocalAPI {
    public static let tokenHeader = "x-mnemonic-token"

    private var endpoint: URL?
    private let fixedEndpoint: URL?
    private let endpointLocked: Bool
    private var endpointVersion: UInt64 = 0
    private let enabled: @Sendable () -> Bool
    private let readToken: @Sendable () -> String?
    private let transport: LocalTransport
    private var token: String?
    private var tag: String?
    private var held: LiveSnapshot?
    var graphCache = CuratedGraphCache()

    public init(
        endpoint: URL,
        enabled: @escaping @Sendable () -> Bool,
        token: @escaping @Sendable () -> String?,
        transport: LocalTransport,
        endpointLocked: Bool = false
    ) {
        let normalized = try? LocalEndpoint.parse(endpoint.absoluteString)
        self.endpoint = normalized
        fixedEndpoint = endpointLocked ? normalized : nil
        self.endpointLocked = endpointLocked
        self.enabled = enabled
        readToken = token
        self.transport = transport
    }

    public init(profile: MnemonicProfile) {
        self.init(
            endpoint: profile.endpoint,
            enabled: { profile.localAPIEnabled() },
            token: { profile.token() },
            transport: SessionTransport(profile: profile),
            endpointLocked: profile.isolated || profile.endpointNamed)
    }

    /// Apply a saved setting synchronously before the first window request.
    /// The profile always supplies the token, switch and transport policy.
    public init(profile: MnemonicProfile, endpoint: URL) throws {
        let selected = try Self.validatedEndpoint(endpoint.absoluteString, profile: profile)
        self.init(
            endpoint: selected,
            enabled: { profile.localAPIEnabled() },
            token: { profile.token() },
            transport: SessionTransport(profile: profile),
            endpointLocked: profile.isolated || profile.endpointNamed)
    }

    public static func validatedEndpoint(_ value: String, profile: MnemonicProfile) throws -> URL {
        let selected = try LocalEndpoint.parse(value)
        if profile.isolated || profile.endpointNamed {
            let original = try LocalEndpoint.parse(profile.endpoint.absoluteString)
            guard selected == original else { throw LocalEndpointFailure.fixedProfile }
        }
        return selected
    }

    /// A default profile may move to another local port. An isolated or
    /// explicitly named endpoint stays fixed for the lifetime of this actor.
    public func setEndpoint(_ value: String) throws {
        let selected = try LocalEndpoint.parse(value)
        if endpointLocked && selected != fixedEndpoint {
            throw LocalEndpointFailure.fixedProfile
        }
        guard selected != endpoint else { return }
        endpoint = selected
        endpointVersion &+= 1
        token = nil
        tag = nil
        held = nil
        graphCache = CuratedGraphCache()
    }

    var graphEndpointVersion: UInt64 { endpointVersion }

    func graphResponse(path: String, tag: String? = nil, query: [URLQueryItem] = [],
                       method: String = "GET", body: Data? = nil) async
        -> Result<LocalResponse, LocalAPIFailure>
    {
        await response(path: path, tag: tag, query: query, method: method, body: body)
    }

    public func currentEndpoint() -> URL? { endpoint }

    /// The snapshot last decoded.
    public func snapshot() -> LiveSnapshot? { held }

    public func projects() async -> Result<ProjectBoard, LocalAPIFailure> {
        await decoded(ProjectBoard.self, path: "/api/projects")
    }

    public func project(key: String) async -> Result<ProjectDetail, LocalAPIFailure> {
        guard let encoded = ProjectKey.encoded(key) else { return .failure(.undecodable) }
        return await decoded(ProjectDetail.self, path: "/api/projects/\(encoded)")
    }

    public func timeline(day: String) async -> Result<ProjectTimeline, LocalAPIFailure> {
        guard TimelineDay.isValid(day) else { return .failure(.undecodable) }
        let answer = await decoded(ProjectTimeline.self, path: "/api/projects/timeline",
                                   query: [URLQueryItem(name: "day", value: day)])
        if case .success(let value) = answer,
           value.day != day || !value.from.isFinite || !value.to.isFinite || value.from >= value.to {
            return .failure(.undecodable)
        }
        return answer
    }

    public func addAttributionOverride(_ correction: AttributionCorrection) async
        -> Result<Int64, LocalAPIFailure>
    {
        guard correction.isValid, let body = try? JSONEncoder().encode(correction) else {
            return .failure(.undecodable)
        }
        let answer = await decoded(AddedCorrection.self, path: "/api/attribution/overrides",
                                   method: "POST", body: body)
        switch answer {
        case .success(let value) where value.id > 0: return .success(value.id)
        case .success: return .failure(.undecodable)
        case .failure(let failure): return .failure(failure)
        }
    }

    public func removeAttributionOverride(id: Int64) async -> Result<Void, LocalAPIFailure> {
        guard id > 0 else { return .failure(.undecodable) }
        let answer = await decoded(RemovedCorrection.self, path: "/api/attribution/overrides/\(id)",
                                   method: "DELETE")
        switch answer {
        case .success(let value) where value.removed == id: return .success(())
        case .success: return .failure(.undecodable)
        case .failure(let failure): return .failure(failure)
        }
    }

    private struct AddedCorrection: Decodable, Sendable { let id: Int64 }
    private struct RemovedCorrection: Decodable, Sendable { let removed: Int64 }

    /// After failure the next live request carries no tag. A response from a
    /// previous endpoint cannot replace or clear the current endpoint's cache.
    public func live() async -> LiveAnswer {
        let version = endpointVersion
        let answer = await ask()
        if case .failed = answer, version == endpointVersion {
            tag = nil
            held = nil
        }
        return answer
    }

    private func ask() async -> LiveAnswer {
        let version = endpointVersion
        let result = await response(path: "/api/live", tag: held == nil ? nil : tag)
        guard version == endpointVersion else { return .failed(.unreachable) }
        switch result {
        case .failure(let failure): return .failed(failure)
        case .success(let response):
            switch response.status {
            case 304 where held != nil:
                return .unchanged
            case 200:
                guard let decoded = try? JSONDecoder().decode(LiveSnapshot.self, from: response.body)
                else { return .failed(.undecodable) }
                held = decoded
                tag = response.tag
                return .snapshot(decoded)
            default:
                return .failed(Self.failure(status: response.status))
            }
        }
    }

    private func decoded<Value: Decodable & Sendable>(
        _ type: Value.Type, path: String, query: [URLQueryItem] = [],
        method: String = "GET", body: Data? = nil
    ) async -> Result<Value, LocalAPIFailure> {
        let version = endpointVersion
        let result = await response(path: path, query: query, method: method, body: body)
        guard version == endpointVersion else { return .failure(.unreachable) }
        switch result {
        case .failure(let failure): return .failure(failure)
        case .success(let response):
            guard response.status == 200 else {
                return .failure(Self.failure(status: response.status))
            }
            guard let value = try? JSONDecoder().decode(type, from: response.body) else {
                return .failure(.undecodable)
            }
            return .success(value)
        }
    }

    private func response(path: String, tag: String? = nil, query: [URLQueryItem] = [],
                          method: String = "GET", body: Data? = nil) async
        -> Result<LocalResponse, LocalAPIFailure>
    {
        guard enabled() else { return .failure(.off) }
        guard let endpoint, let url = LocalEndpoint.url(endpoint: endpoint, path: path, query: query) else {
            return .failure(.unreachable)
        }
        let version = endpointVersion
        if token == nil { token = readToken() }
        do {
            var answer = try await transport.send(request(url: url, tag: tag, method: method, body: body))
            guard version == endpointVersion else { return .failure(.unreachable) }
            if answer.status == 401 {
                token = readToken()
                answer = try await transport.send(request(url: url, tag: tag, method: method, body: body))
                guard version == endpointVersion else { return .failure(.unreachable) }
            }
            return .success(answer)
        } catch {
            return .failure(.unreachable)
        }
    }

    private static func failure(status: Int) -> LocalAPIFailure {
        switch status {
        case 401: return .unauthorised
        case 404: return .earlierDaemon
        default: return .undecodable
        }
    }

    private func request(url: URL, tag: String?, method: String, body: Data?) -> URLRequest {
        var request = URLRequest(url: url, cachePolicy: .reloadIgnoringLocalCacheData)
        request.timeoutInterval = 5
        request.httpMethod = method
        request.httpBody = body
        if body != nil { request.setValue("application/json", forHTTPHeaderField: "Content-Type") }
        if let token { request.setValue(token, forHTTPHeaderField: Self.tokenHeader) }
        if let tag { request.setValue(tag, forHTTPHeaderField: "If-None-Match") }
        return request
    }
}
