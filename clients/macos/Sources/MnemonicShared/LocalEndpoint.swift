import Foundation

/// An endpoint edit cannot expose the profile token or select another profile.
public enum LocalEndpointFailure: Error, Equatable, Sendable {
    case invalid
    case fixedProfile
}

enum LocalEndpoint {
    static var loopbackHost: String {
        String(cString: inet_ntoa(in_addr(s_addr: INADDR_LOOPBACK.bigEndian)))
    }

    /// Normalize a loopback HTTP origin. Paths and URL-carried settings are
    /// never accepted as part of the address of the daemon.
    static func parse(_ value: String) throws -> URL {
        guard value.unicodeScalars.allSatisfy({ scalar in
            scalar.value > 32 && scalar.value < 127
        }),
        let parts = URLComponents(string: value),
        parts.scheme?.lowercased() == "http",
        parts.user == nil, parts.password == nil,
        parts.query == nil, parts.fragment == nil,
        parts.percentEncodedPath.isEmpty || parts.percentEncodedPath == "/",
        let host = parts.percentEncodedHost?.lowercased(),
        ["localhost", loopbackHost, "[::1]", "::1"].contains(host)
        else { throw LocalEndpointFailure.invalid }
        let port = parts.port ?? 80
        guard (1...65535).contains(port) else { throw LocalEndpointFailure.invalid }
        let address = host == "::1" ? "[::1]" : host
        guard let url = URL(string: "http://\(address):\(port)") else {
            throw LocalEndpointFailure.invalid
        }
        return url
    }

    static func url(endpoint: URL, path: String, query: [URLQueryItem] = []) -> URL? {
        guard var parts = URLComponents(url: endpoint, resolvingAgainstBaseURL: false) else {
            return nil
        }
        parts.percentEncodedPath = path
        parts.queryItems = query.isEmpty ? nil : query
        return parts.url
    }
}
