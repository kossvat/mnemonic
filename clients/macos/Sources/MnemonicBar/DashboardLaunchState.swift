import CoreFoundation
import CryptoKit
import Foundation
import MnemonicShared

/// Routing state for only the process the widget launched. While it is
/// starting, the latest click replaces the earlier pending route.
struct DashboardLaunchState {
    static let readinessLimit: TimeInterval = 30
    private(set) var readiness: AppLinkReadiness?
    private(set) var pending: AppLink?
    private(set) var isReady = false
    private(set) var failed = false
    private var startedAt: TimeInterval?

    @discardableResult
    mutating func begin(
        _ launch: AppLinkReadiness, link: AppLink,
        now: TimeInterval = ProcessInfo.processInfo.systemUptime
    ) -> Bool {
        guard now.isFinite, now >= 0, link.url(identity: launch.identity) != nil else { return false }
        readiness = launch
        pending = link
        isReady = false
        failed = false
        startedAt = now
        return true
    }

    /// Evaluated only on a later click. A failed owned process stays known
    /// until exit, preventing duplicate launches without any timer or poll.
    mutating func rejectExpired(now: TimeInterval = ProcessInfo.processInfo.systemUptime) -> Bool {
        guard readiness != nil, !isReady else { return false }
        if failed { return true }
        guard let startedAt, now.isFinite, now - startedAt >= Self.readinessLimit else { return false }
        failed = true
        pending = nil
        return true
    }

    /// A returned link can be delivered now; nil means it stays pending.
    mutating func request(_ link: AppLink, identity: String) -> AppLink? {
        guard let readiness, !failed, readiness.identity == identity,
              link.url(identity: identity) != nil else { return nil }
        if isReady { return link }
        pending = link
        return nil
    }

    /// Late acknowledgements from another launch cannot release a route.
    mutating func received(_ launch: AppLinkReadiness) -> AppLink? {
        guard readiness == launch, !isReady, !failed else { return nil }
        isReady = true
        let latest = pending
        pending = nil
        return latest
    }

    /// Process termination is also scoped by launch. A previous process
    /// exiting after its replacement started cannot clear the new state.
    @discardableResult
    mutating func ended(_ launch: AppLinkReadiness) -> Bool {
        guard readiness == launch else { return false }
        readiness = nil
        pending = nil
        isReady = false
        failed = false
        startedAt = nil
        return true
    }
}

/// A packaged dashboard declares its protocol in its manifest. A raw
/// executable needs a sibling build receipt bound to those exact bytes.
/// These checks read files only; unsupported applications are never run.
enum DashboardCapability {
    static func firstCompatible(built: [URL], bundles: [URL]) -> URL? {
        for executable in built where compatibleExecutable(executable) { return executable }
        for bundle in bundles {
            let executable = bundle.appendingPathComponent("Contents/MacOS/MnemonicApp")
            let manifest = bundle.appendingPathComponent("Contents/Info.plist")
            if FileManager.default.isExecutableFile(atPath: executable.path),
               let fields = metadata(at: manifest), matchesVersion(fields) {
                return executable
            }
        }
        return nil
    }

    static func receiptURL(for executable: URL) -> URL {
        executable.appendingPathExtension("route-capability.plist")
    }

    private static func compatibleExecutable(_ executable: URL) -> Bool {
        guard FileManager.default.isExecutableFile(atPath: executable.path),
              let fields = metadata(at: receiptURL(for: executable)), matchesVersion(fields),
              let expected = fields[AppLinkReadiness.executableDigestKey] as? String,
              expected.utf8.count == 64,
              expected.utf8.allSatisfy({ byte in
                  (48...57).contains(byte) || (97...102).contains(byte)
              }), let actual = digest(of: executable) else { return false }
        return expected == actual
    }

    private static func metadata(at url: URL) -> [String: Any]? {
        guard let values = try? url.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey]),
              values.isRegularFile == true, let size = values.fileSize, size > 0, size <= 16_384,
              let data = try? Data(contentsOf: url),
              let fields = try? PropertyListSerialization.propertyList(
                from: data, options: [], format: nil) as? [String: Any] else { return nil }
        return fields
    }

    private static func matchesVersion(_ fields: [String: Any]) -> Bool {
        guard let number = fields[AppLinkReadiness.capabilityKey] as? NSNumber,
              CFGetTypeID(number) != CFBooleanGetTypeID() else { return false }
        return number.doubleValue == Double(AppLinkReadiness.protocolVersion)
    }

    private static func digest(of url: URL) -> String? {
        guard let values = try? url.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey]),
              values.isRegularFile == true, let size = values.fileSize, size > 0,
              size <= 256 * 1_024 * 1_024, let file = try? FileHandle(forReadingFrom: url) else { return nil }
        defer { try? file.close() }
        do {
            var digest = SHA256()
            while let bytes = try file.read(upToCount: 65_536), !bytes.isEmpty {
                digest.update(data: bytes)
            }
            return digest.finalize().map { byte in String(format: "%02x", byte) }.joined()
        } catch { return nil }
    }
}
