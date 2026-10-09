import Foundation

/// One launch's acknowledgement that its profile route subscriber exists.
/// Both the selected profile and the launch nonce must match the listener.
public struct AppLinkReadiness: Equatable, Sendable {
    /// Build artifacts declare this version before the widget may run them.
    public static let protocolVersion = 1
    public static let capabilityKey = "MnemonicRouteProtocolVersion"
    public static let executableDigestKey = "MnemonicExecutableSHA256"

    public let identity: String
    public let nonce: String

    public init?(identity: String, nonce: String) {
        guard AppLink.notificationName(identity: identity) != nil,
              AppLink.notificationName(identity: nonce) != nil else { return nil }
        self.identity = identity
        self.nonce = nonce
    }

    public init?(arguments: [String], identity: String) {
        let flags = arguments.enumerated().filter { entry in
            entry.element == "--route-ready" || entry.element.hasPrefix("--route-ready=")
        }
        guard flags.count == 1, let flag = flags.first,
              flag.element == "--route-ready", arguments.count > flag.offset + 1 else { return nil }
        self.init(identity: identity, nonce: arguments[flag.offset + 1])
    }

    public var notificationName: Notification.Name {
        Notification.Name("mnemonic.route-ready.\(identity).\(nonce)")
    }

    /// Kept separate from the observer so fixtures can verify the protocol
    /// without posting a system notification or starting an application.
    public func accepts(_ notification: Notification) -> Bool {
        notification.name == notificationName && notification.object as? String == nonce
    }

    public func post(center: DistributedNotificationCenter = .default()) {
        center.postNotificationName(notificationName, object: nonce,
                                    userInfo: nil, deliverImmediately: true)
    }

    /// Call only after the app has installed its route subscriber.
    @discardableResult
    public static func announce(
        arguments: [String], identity: String, center: DistributedNotificationCenter = .default()
    ) -> Bool {
        guard let readiness = Self(arguments: arguments, identity: identity) else { return false }
        readiness.post(center: center)
        return true
    }
}

/// A single launch's readiness listener. Install it before starting the
/// process, and cancel it after readiness, launch failure or process exit.
public final class AppLinkReadinessSubscriber {
    private let center: DistributedNotificationCenter
    private let delivery = AppLinkDelivery()
    private var observer: NSObjectProtocol?

    public init(
        readiness: AppLinkReadiness, center: DistributedNotificationCenter = .default(),
        received: @escaping @Sendable (AppLinkReadiness) -> Void
    ) {
        self.center = center
        let delivery = self.delivery
        observer = center.addObserver(forName: readiness.notificationName, object: readiness.nonce,
                                      queue: .main) { notification in
            guard readiness.accepts(notification), delivery.accepts() else { return }
            received(readiness)
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
