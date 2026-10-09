import Foundation

/// A route inside the already selected profile. A link never supplies an
/// endpoint, token or home, and its identity must match the running client.
public enum AppLink: Equatable, Sendable {
    case project(String)
    case search
    case projects

    public func url(identity: String) -> URL? {
        guard Self.validIdentity(identity) else { return nil }
        var parts = URLComponents()
        parts.scheme = "mnemonic"
        switch self {
        case .project(let key):
            guard let encoded = ProjectKey.encoded(key) else { return nil }
            parts.host = "project"
            parts.percentEncodedPath = "/\(encoded)"
        case .search:
            parts.host = "search"
        case .projects:
            parts.host = "projects"
        }
        parts.queryItems = [URLQueryItem(name: "profile", value: identity)]
        return parts.url
    }

    public init?(url: URL, expectedIdentity: String) {
        guard Self.validIdentity(expectedIdentity),
              let parts = URLComponents(url: url, resolvingAgainstBaseURL: false),
              parts.scheme == "mnemonic",
              parts.user == nil, parts.password == nil, parts.port == nil,
              parts.fragment == nil,
              let query = parts.queryItems, query.count == 1,
              query[0].name == "profile", query[0].value == expectedIdentity
        else { return nil }
        switch parts.percentEncodedHost {
        case "project":
            let path = parts.percentEncodedPath
            guard path.hasPrefix("/"),
                  !path.dropFirst().contains("/"),
                  let key = String(path.dropFirst()).removingPercentEncoding,
                  ProjectKey.isValid(key)
            else { return nil }
            self = .project(key)
        case "search", "projects":
            guard parts.percentEncodedPath.isEmpty || parts.percentEncodedPath == "/" else {
                return nil
            }
            self = parts.percentEncodedHost == "search" ? .search : .projects
        default:
            return nil
        }
    }

    public static func notificationName(identity: String) -> Notification.Name? {
        guard validIdentity(identity) else { return nil }
        return Notification.Name("mnemonic.route.\(identity)")
    }

    /// Notify an already running app of this profile. Launching the app is
    /// the caller's responsibility; this method neither selects nor opens it.
    @discardableResult
    public func post(
        identity: String, center: DistributedNotificationCenter = .default()
    ) -> Bool {
        guard let name = Self.notificationName(identity: identity),
              let url = url(identity: identity) else { return false }
        center.postNotificationName(name, object: url.absoluteString,
                                    userInfo: nil, deliverImmediately: true)
        return true
    }

    private static func validIdentity(_ value: String) -> Bool {
        value.utf8.count == 32 && value.utf8.allSatisfy { byte in
            (48...57).contains(byte) || (97...102).contains(byte)
        }
    }
}

final class AppLinkDelivery: @unchecked Sendable {
    private let lock = NSLock()
    private var active = true

    func accepts() -> Bool { lock.withLock { active } }
    func cancel() { lock.withLock { active = false } }
}

/// Receives only links for one expected identity, on the main operation queue.
/// Callers that own UI state can forward the value to their main actor.
public final class AppLinkSubscriber {
    private let center: DistributedNotificationCenter
    private let delivery = AppLinkDelivery()
    private var observer: NSObjectProtocol?

    public init?(
        identity: String, center: DistributedNotificationCenter = .default(),
        received: @escaping @Sendable (AppLink) -> Void
    ) {
        guard let name = AppLink.notificationName(identity: identity) else { return nil }
        self.center = center
        let delivery = self.delivery
        observer = center.addObserver(forName: name, object: nil, queue: .main) { notification in
            guard let text = notification.object as? String,
                  let url = URL(string: text),
                  let link = AppLink(url: url, expectedIdentity: identity),
                  delivery.accepts() else { return }
            received(link)
        }
    }

    public func cancel() {
        delivery.cancel()
        if let observer {
            center.removeObserver(observer)
            self.observer = nil
        }
    }

    deinit { cancel() }
}
