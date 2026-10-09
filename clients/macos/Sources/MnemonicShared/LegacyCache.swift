import Foundation

/// The cache of responses that earlier builds kept.
///
/// They asked through the shared session, which stores responses in the
/// user's caches folder under the client's name. A response of the local
/// API is text of memories, so that folder was a copy of memories outside
/// the store. A build that keeps no response removes it when it starts.
public enum LegacyCache {
    /// The names a client's cache folder can have: its bundle identifier
    /// when it runs from a bundle, and the name of its process.
    public static func clientNames(
        bundle: Bundle = .main, process: ProcessInfo = .processInfo
    ) -> [String] {
        var names = [process.processName]
        if let identifier = bundle.bundleIdentifier, !names.contains(identifier) {
            names.append(identifier)
        }
        return names
    }

    /// Remove the cache folders of this client under `caches`. Returns the
    /// folders removed.
    ///
    /// Only a folder that holds a session's database is taken for the cache
    /// of a session. An isolated client never had one, and leaves the one
    /// of the default profile alone: it is not its own.
    @discardableResult
    public static func remove(
        clients: [String], caches: URL, profile: MnemonicProfile
    ) -> [URL] {
        guard !profile.isolated else { return [] }
        let manager = FileManager.default
        return clients.compactMap { client in
            let folder = caches.appendingPathComponent(client, isDirectory: true)
            let database = folder.appendingPathComponent("Cache.db")
            guard manager.fileExists(atPath: database.path),
                  (try? manager.removeItem(at: folder)) != nil
            else { return nil }
            return folder
        }
    }

    /// Remove the cache of this client from the user's caches folder.
    public static func removeForThisClient(profile: MnemonicProfile = .current) {
        guard let caches = FileManager.default.urls(
            for: .cachesDirectory, in: .userDomainMask).first
        else { return }
        remove(clients: clientNames(), caches: caches, profile: profile)
    }
}
