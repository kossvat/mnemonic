import Foundation

enum MnemonicClientError: LocalizedError {
    case missingToken(URL)
    case badEndpoint(String)
    case unauthorized
    case server(Int, String)
    case emptyResponse

    var errorDescription: String? {
        switch self {
        case .missingToken(let url):
            "Token file not found at \(url.path)"
        case .badEndpoint(let value):
            "Invalid endpoint URL: \(value)"
        case .unauthorized:
            "Authentication failed after refreshing token"
        case .server(let code, let message):
            "HTTP \(code): \(message)"
        case .emptyResponse:
            "Server returned an empty response"
        }
    }
}

actor MnemonicClient {
    private var endpoint: URL
    private let tokenURL: URL
    private var cachedToken: String?
    private let session: URLSession
    private let decoder = JSONDecoder()
    private let encoder = JSONEncoder()

    init(
        endpoint: URL = URL(string: "http://127.0.0.1:3737")!,
        tokenURL: URL = FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent(".mnemonic/auth.token"),
        session: URLSession = .shared
    ) {
        self.endpoint = endpoint
        self.tokenURL = tokenURL
        self.session = session
    }

    func setEndpoint(_ value: String) throws {
        guard let url = URL(string: value), let scheme = url.scheme, !scheme.isEmpty else {
            throw MnemonicClientError.badEndpoint(value)
        }
        endpoint = url
    }

    func currentEndpoint() -> URL {
        endpoint
    }

    func tokenPath() -> String {
        tokenURL.path
    }

    func fetchStatus() async throws -> StatusResponse {
        try await request("/api/status")
    }

    func fetchMemories(limit: Int = 100) async throws -> MemoryListResponse {
        try await request("/api/memories?limit=\(limit)")
    }

    func fetchMemory(id: String) async throws -> Memory {
        try await request("/api/memories/\(id.urlPathEscaped)")
    }

    func fetchEntities(limit: Int = 500) async throws -> EntitiesResponse {
        try await request("/api/entities?limit=\(limit)")
    }

    func fetchEntity(name: String) async throws -> EntityDetail {
        try await request("/api/entities/\(name.urlPathEscaped)")
    }

    func fetchGraph(limit: Int = 200) async throws -> GraphResponse {
        try await request("/api/graph?limit=\(limit)")
    }

    /// Memory-centric graph with filters.
    /// - limit: cap on visible memories (default 40 — graph is unreadable
    ///   beyond a few dozen nodes).
    /// - sinceDays: only memories newer than N days.
    /// - type: "decision" / "feedback" / "note" / nil (all).
    /// - query: substring filter on title/content.
    /// - minShared: drop edges where memories share fewer than N entities
    ///   (default 1; raise to 2 to declutter).
    func fetchMemoryGraph(
        limit: Int = 40,
        sinceDays: Int? = nil,
        type: String? = nil,
        query: String? = nil,
        minShared: Int = 1
    ) async throws -> MemoryGraphResponse {
        var components = URLComponents()
        components.path = "/api/memory-graph"
        var items: [URLQueryItem] = [
            URLQueryItem(name: "limit", value: String(limit)),
            URLQueryItem(name: "min_shared", value: String(minShared)),
        ]
        if let sinceDays { items.append(URLQueryItem(name: "since_days", value: String(sinceDays))) }
        if let type, !type.isEmpty { items.append(URLQueryItem(name: "type", value: type)) }
        if let query, !query.trimmingCharacters(in: .whitespaces).isEmpty {
            items.append(URLQueryItem(name: "q", value: query))
        }
        components.queryItems = items
        let path = (components.percentEncodedPath) + "?" + (components.percentEncodedQuery ?? "")
        return try await request(path)
    }

    func fetchDaily(days: Int = 14) async throws -> DailyResponse {
        try await request("/api/stats/daily?days=\(days)")
    }

    func search(query: String, limit: Int = 20, withGraphHop: Bool = true) async throws -> SearchResponse {
        try await request(
            "/api/search",
            method: "POST",
            body: SearchRequest(query: query, limit: limit, withGraphHop: withGraphHop)
        )
    }

    func dedupe(apply: Bool) async throws -> DedupeReport {
        try await request("/api/dedupe", method: "POST", body: ["apply": apply])
    }

    func reextract(sinceDays: Int?, limit: Int?, dryRun: Bool) async throws -> ReextractReport {
        try await request(
            "/api/reextract",
            method: "POST",
            body: ReextractRequest(sinceDays: sinceDays, limit: limit, dryRun: dryRun)
        )
    }

    func cleanup(days: Int, threshold: Double, confirm: Bool) async throws -> CleanupReport {
        try await request(
            "/api/cleanup",
            method: "POST",
            body: CleanupRequest(days: days, threshold: threshold, confirm: confirm)
        )
    }

    func mergeEntity(name: String, into canonical: String) async throws -> MergeReport {
        try await request(
            "/api/entities/\(name.urlPathEscaped)/merge",
            method: "POST",
            body: ["into": canonical]
        )
    }

    func forgetMemory(id: String) async throws -> ForgetReport {
        try await request("/api/memories/\(id.urlPathEscaped)", method: "DELETE")
    }

    func reflect(apply: Bool, threshold: Double = 0.85) async throws -> ReflectionPlan {
        try await request(
            "/api/reflect",
            method: "POST",
            body: ReflectBody(apply: apply, threshold: threshold)
        )
    }

    func memorySources(id: String) async throws -> MemorySourcesResponse {
        try await request("/api/memories/\(id.urlPathEscaped)/sources")
    }

    private func request<T: Decodable, B: Encodable>(
        _ path: String,
        method: String = "GET",
        body: B? = Optional<EmptyBody>.none,
        retryingAfter401: Bool = false
    ) async throws -> T {
        let token = try readToken(force: retryingAfter401)
        var request = URLRequest(url: try url(for: path))
        request.httpMethod = method
        request.setValue(token, forHTTPHeaderField: "X-Mnemonic-Token")

        if let body {
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try encoder.encode(body)
        }

        let (data, response) = try await session.data(for: request)
        guard let http = response as? HTTPURLResponse else {
            throw MnemonicClientError.emptyResponse
        }

        if http.statusCode == 401 {
            cachedToken = nil
            guard !retryingAfter401 else { throw MnemonicClientError.unauthorized }
            return try await self.request(path, method: method, body: body, retryingAfter401: true)
        }

        guard (200..<300).contains(http.statusCode) else {
            let message = (try? decoder.decode(APIErrorResponse.self, from: data).error)
                ?? String(data: data, encoding: .utf8)
                ?? "Unknown server error"
            throw MnemonicClientError.server(http.statusCode, message)
        }

        guard !data.isEmpty else { throw MnemonicClientError.emptyResponse }
        return try decoder.decode(T.self, from: data)
    }

    private func request<T: Decodable>(
        _ path: String,
        method: String = "GET",
        retryingAfter401: Bool = false
    ) async throws -> T {
        let empty: EmptyBody? = nil
        return try await request(path, method: method, body: empty, retryingAfter401: retryingAfter401)
    }

    private func url(for path: String) throws -> URL {
        guard let url = URL(string: path, relativeTo: endpoint)?.absoluteURL else {
            throw MnemonicClientError.badEndpoint(endpoint.absoluteString + path)
        }
        return url
    }

    private func readToken(force: Bool = false) throws -> String {
        if !force, let cachedToken {
            return cachedToken
        }
        let token = try String(contentsOf: tokenURL, encoding: .utf8)
            .trimmingCharacters(in: .whitespacesAndNewlines)
        guard !token.isEmpty else {
            throw MnemonicClientError.missingToken(tokenURL)
        }
        cachedToken = token
        return token
    }
}

private struct EmptyBody: Encodable {}

private struct ReextractRequest: Encodable {
    let sinceDays: Int?
    let limit: Int?
    let dryRun: Bool

    enum CodingKeys: String, CodingKey {
        case sinceDays = "since_days"
        case limit
        case dryRun = "dry_run"
    }
}

private struct SearchRequest: Encodable {
    let query: String
    let limit: Int
    let withGraphHop: Bool

    enum CodingKeys: String, CodingKey {
        case query
        case limit
        case withGraphHop = "with_graph_hop"
    }
}

private struct CleanupRequest: Encodable {
    let days: Int
    let threshold: Double
    let confirm: Bool
}

private struct ReflectBody: Encodable {
    let apply: Bool
    let threshold: Double
}

private extension String {
    var urlPathEscaped: String {
        addingPercentEncoding(withAllowedCharacters: .urlPathAllowed) ?? self
    }
}
