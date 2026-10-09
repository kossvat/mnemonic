import Foundation
import notify

/// The name the daemon of a profile posts its signal under: the live
/// snapshot changed. The name carries the identity of the profile, an
/// opaque value in a file of its home, so two profiles on one port wake
/// only their own clients. The same rule as `live_signal::name_of` in the
/// daemon.
public enum LiveSignal {
    public static func name(identity: String) -> String {
        "mnemonic.live.\(identity)"
    }

    /// The name of this profile's signal. A client that starts before the
    /// daemon makes the identity here, and the daemon reads the same one.
    public static func name(of profile: MnemonicProfile) throws -> String {
        name(identity: try profile.identity())
    }
}

/// Listens for the signal of one name until it is cancelled or released.
/// The signal has no body: it only says "ask again".
public final class LiveSignalSubscriber: @unchecked Sendable {
    private var token: Int32 = NOTIFY_TOKEN_INVALID

    /// `heard` is called on `queue` for every signal posted under `name`.
    public init?(name: String, queue: DispatchQueue = .main, heard: @escaping @Sendable () -> Void) {
        let status = notify_register_dispatch(name, &token, queue) { _ in heard() }
        guard status == NOTIFY_STATUS_OK else { return nil }
    }

    public func cancel() {
        if token != NOTIFY_TOKEN_INVALID {
            notify_cancel(token)
            token = NOTIFY_TOKEN_INVALID
        }
    }

    deinit { cancel() }
}
