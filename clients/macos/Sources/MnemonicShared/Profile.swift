import Foundation

/// Where a client finds its daemon: the home of the store, the endpoint of
/// the local API, the token, the command line, the preferences and the
/// identity its signal is named by.
///
/// Resolved once, by the rule the daemon uses (`src/profile.rs`):
/// `MNEMONIC_HOME` or `--home` selects an isolated profile, and nothing then
/// falls back to the owner's default paths. A selection that is present but
/// unusable is an error, never a silent return to the default store, and a
/// file of an isolated profile that leads out of its home is not read.
public struct MnemonicProfile: Equatable, Sendable {
    public static let homeVariable = "MNEMONIC_HOME"
    public static let endpointVariable = "MNEMONIC_ENDPOINT"
    public static let commandVariable = "MNEMONIC_BIN"
    public static let defaultPort = 3737

    public enum Failure: Error, Equatable, CustomStringConvertible {
        case empty
        case relative(String)
        case parent(String)
        case notDedicated(String)
        case overlapsDefault(String)
        case disagree(flag: String, variable: String)
        case flagWithoutValue
        case outside(String)
        case endpoint(String)
        case command(String)

        public var description: String {
            switch self {
            case .empty:
                "\(MnemonicProfile.homeVariable) is set but empty"
            case .relative(let path):
                "\(MnemonicProfile.homeVariable) must be an absolute path, got \(path)"
            case .parent(let path):
                "\(MnemonicProfile.homeVariable) must not contain `..`: \(path)"
            case .notDedicated(let path):
                "\(MnemonicProfile.homeVariable) must be a dedicated directory, not \(path)"
            case .overlapsDefault(let path):
                "\(MnemonicProfile.homeVariable) \(path) overlaps the default store"
            case .disagree(let flag, let variable):
                "--home \(flag) and \(MnemonicProfile.homeVariable) \(variable) disagree"
            case .flagWithoutValue:
                "--home needs a directory"
            case .outside(let name):
                "\(name) of the isolated profile leads outside it"
            case .endpoint(let value):
                "\(MnemonicProfile.endpointVariable) must be an http address of this machine, got \(value)"
            case .command(let value):
                "\(MnemonicProfile.commandVariable) must be an absolute path, got \(value)"
            }
        }
    }

    /// The home of the store: the isolated profile, or the default one.
    public let home: URL
    /// Whether a profile was selected. An isolated client keeps everything
    /// it writes inside `home`.
    public let isolated: Bool
    /// The local API.
    public let endpoint: URL
    /// Whether the endpoint was named outright, by the variable. A named
    /// endpoint and the endpoint of an isolated profile are not replaced by
    /// one a client saved earlier.
    public let endpointNamed: Bool
    /// The token the daemon wrote for this home (see `token()`).
    public let tokenURL: URL
    /// The configuration file of this home.
    public let configURL: URL
    /// The command line tool, and what selects this profile for it.
    public let command: URL
    public let commandArguments: [String]

    /// Where the identity of the profile is kept (see `identity()`).
    public var identityURL: URL { home.appendingPathComponent("profile.id") }
    /// The daemon's log of this home.
    public var logURL: URL { home.appendingPathComponent("daemon.log") }

    /// Where an isolated client keeps its preferences: a file for each
    /// client, since two processes that rewrite one file lose each other's
    /// changes.
    public func preferencesURL(client: String) -> URL {
        home.appendingPathComponent("preferences-\(client).plist")
    }

    /// The profile of this process. A selection that cannot be used ends
    /// the process: showing the owner's store in its place would be worse.
    public static let current: MnemonicProfile = {
        do {
            return try resolve(
                environment: ProcessInfo.processInfo.environment,
                arguments: CommandLine.arguments,
                userHome: FileManager.default.homeDirectoryForCurrentUser
            )
        } catch {
            FileHandle.standardError.write(Data("mnemonic client: \(error)\n".utf8))
            exit(78)
        }
    }()

    public static func resolve(
        environment: [String: String],
        arguments: [String],
        userHome: URL
    ) throws -> MnemonicProfile {
        let fromVariable = try environment[homeVariable].map { raw in
            try parse(raw, userHome: userHome)
        }
        let fromFlag = try flagValue(in: arguments).map { raw in
            try parse(raw, userHome: userHome)
        }
        if let flag = fromFlag, let variable = fromVariable, !sameLocation(flag, variable) {
            throw Failure.disagree(flag: flag.path, variable: variable.path)
        }
        let selected = fromFlag ?? fromVariable
        let home = selected ?? userHome.appendingPathComponent(".mnemonic")
        let configURL = selected.map { chosen in chosen.appendingPathComponent("config.toml") }
            ?? userHome.appendingPathComponent(".config/mnemonic/config.toml")
        if let chosen = selected, !isInside(configURL, home: chosen) {
            throw Failure.outside("config.toml")
        }

        let endpoint: URL
        if let raw = environment[endpointVariable] {
            endpoint = try loopback(raw)
        } else {
            let port = configuredPort(in: configURL) ?? defaultPort
            endpoint = loopbackEndpoint(port: port)
        }

        let command: URL
        if let raw = environment[commandVariable] {
            guard raw.hasPrefix("/") else { throw Failure.command(raw) }
            command = URL(fileURLWithPath: raw)
        } else {
            command = userHome.appendingPathComponent(".cargo/bin/mnemonic")
        }

        return MnemonicProfile(
            home: home,
            isolated: selected != nil,
            endpoint: endpoint,
            endpointNamed: environment[endpointVariable] != nil,
            tokenURL: home.appendingPathComponent("auth.token"),
            configURL: configURL,
            command: command,
            commandArguments: selected.map { chosen in ["--home", chosen.path] } ?? []
        )
    }

    /// The token of this profile. In an isolated profile a token file that
    /// leads out of the home is not read: it would be another store's.
    public func token() -> String? {
        guard contains(tokenURL),
              let text = try? String(contentsOf: tokenURL, encoding: .utf8)
        else { return nil }
        let token = text.trimmingCharacters(in: .whitespacesAndNewlines)
        return token.isEmpty ? nil : token
    }

    /// Whether a file belongs to this profile. Every file of the default
    /// profile does; of an isolated one, those that resolve inside its home.
    public func contains(_ url: URL) -> Bool {
        !isolated || Self.isInside(url, home: home)
    }

    /// The configuration of the session requests go through. No response
    /// is kept on disk, in any profile: a response of the local API is text
    /// of memories, and a copy of it outside the store is a second store.
    public func sessionConfiguration() -> URLSessionConfiguration {
        let configuration = URLSessionConfiguration.ephemeral
        configuration.urlCache = nil
        configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
        return configuration
    }

    /// The identity of the profile: an opaque value kept in its home, made
    /// by whoever needs it first, the daemon or a client. The file is linked
    /// into place whole, so a reader never sees it half written, and of two
    /// that make it at once one wins and both read the same value.
    public func identity() throws -> String {
        guard contains(identityURL) else { throw Failure.outside("profile.id") }
        let manager = FileManager.default
        if let found = Self.readIdentity(identityURL) { return found }
        try manager.createDirectory(
            at: home, withIntermediateDirectories: true,
            attributes: [.posixPermissions: 0o700])
        var bytes = [UInt8](repeating: 0, count: 16)
        var generator = SystemRandomNumberGenerator()
        for index in bytes.indices { bytes[index] = generator.next() }
        let value = bytes.map { byte in String(format: "%02x", byte) }.joined()
        let scratch = home.appendingPathComponent(
            "profile.id.\(ProcessInfo.processInfo.processIdentifier).\(UUID().uuidString)")
        guard manager.createFile(
            atPath: scratch.path, contents: Data(value.utf8),
            attributes: [.posixPermissions: 0o600])
        else { throw CocoaError(.fileWriteUnknown) }
        defer { try? manager.removeItem(at: scratch) }
        if link(scratch.path, identityURL.path) == 0 { return value }
        guard errno == EEXIST, let found = Self.readIdentity(identityURL) else {
            throw POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
        }
        return found
    }

    private static func readIdentity(_ url: URL) -> String? {
        guard let text = try? String(contentsOf: url, encoding: .utf8) else { return nil }
        let value = text.trimmingCharacters(in: .whitespacesAndNewlines)
        let hex = value.unicodeScalars.allSatisfy { scalar in
            ("0"..."9").contains(scalar) || ("a"..."f").contains(scalar)
        }
        return value.count == 32 && hex ? value : nil
    }

    // MARK: The rule of `src/profile.rs`

    /// The directory the flag names, in either spelling the command line of
    /// the daemon takes: `--home DIR` and `--home=DIR`.
    private static func flagValue(in arguments: [String]) throws -> String? {
        for (index, argument) in arguments.enumerated() {
            if argument == "--home" {
                guard index + 1 < arguments.count else { throw Failure.flagWithoutValue }
                return arguments[index + 1]
            }
            if argument.hasPrefix("--home=") {
                return String(argument.dropFirst("--home=".count))
            }
        }
        return nil
    }

    private static func parse(_ raw: String, userHome: URL) throws -> URL {
        if raw.isEmpty { throw Failure.empty }
        guard raw.hasPrefix("/") else { throw Failure.relative(raw) }
        if raw.split(separator: "/").contains("..") { throw Failure.parent(raw) }
        let home = URL(fileURLWithPath: raw)
        let resolved = components(home)
        // Never the user's home or one of its ancestors.
        if starts(components(userHome), with: resolved) { throw Failure.notDedicated(raw) }
        for reserved in [".mnemonic", ".config/mnemonic"] {
            let taken = components(userHome.appendingPathComponent(reserved))
            if starts(resolved, with: taken) || starts(taken, with: resolved) {
                throw Failure.overlapsDefault(raw)
            }
        }
        return home
    }

    private static func components(_ url: URL) -> [String] {
        url.resolvingSymlinksInPath().standardizedFileURL.pathComponents
    }

    private static func starts(_ path: [String], with prefix: [String]) -> Bool {
        path.count >= prefix.count && Array(path.prefix(prefix.count)) == prefix
    }

    private static func sameLocation(_ a: URL, _ b: URL) -> Bool {
        a.path == b.path || components(a) == components(b)
    }

    /// Does `url`, as the system resolves it, lie inside `home`? A link that
    /// leads out does not, and neither does one that leads nowhere: whoever
    /// made it could point it anywhere later.
    static func isInside(_ url: URL, home: URL) -> Bool {
        let manager = FileManager.default
        if (try? manager.destinationOfSymbolicLink(atPath: url.path)) != nil,
           !manager.fileExists(atPath: url.path) {
            return false
        }
        return starts(components(url), with: components(home))
    }

    // MARK: The endpoint

    /// The address of this machine, as the system names it.
    private static var loopbackHost: String {
        String(cString: inet_ntoa(in_addr(s_addr: INADDR_LOOPBACK.bigEndian)))
    }

    private static func loopbackEndpoint(port: Int) -> URL {
        var parts = URLComponents()
        parts.scheme = "http"
        parts.host = loopbackHost
        parts.port = port
        // The three parts above always make an address.
        return parts.url ?? URL(fileURLWithPath: "/")
    }

    /// A token goes only to this machine: an endpoint anywhere else is
    /// refused, whoever set it.
    private static func loopback(_ raw: String) throws -> URL {
        guard let url = URL(string: raw), url.scheme == "http",
              let host = url.host,
              host == "localhost" || host == loopbackHost || host == "::1"
        else { throw Failure.endpoint(raw) }
        return url
    }

    /// `port` of the `[ui]` table of a configuration file, read without a
    /// parser of the whole format: the one key this client needs. A comment
    /// after the header of the table or after the value is allowed, as the
    /// format allows it.
    static func configuredPort(in url: URL) -> Int? {
        for value in configuredValues(of: "port", in: url) {
            if let port = Int(value), (1...65535).contains(port) { return port }
        }
        return nil
    }

    /// Whether the local API is on in the configuration of this profile:
    /// `enabled = true` in its `[ui]` table. Left out, it is off, as the
    /// daemon takes it. Read when asked: the owner may turn it on while a
    /// client runs.
    public func localAPIEnabled() -> Bool {
        guard contains(configURL) else { return false }
        return Self.configuredValues(of: "enabled", in: configURL).first == "true"
    }

    /// The values a key of the `[ui]` table is given, in the order of the
    /// file.
    static func configuredValues(of key: String, in url: URL) -> [String] {
        guard let text = try? String(contentsOf: url, encoding: .utf8) else { return [] }
        var inside = false
        var values: [String] = []
        for line in text.split(separator: "\n", omittingEmptySubsequences: false) {
            let code = line.split(separator: "#", maxSplits: 1, omittingEmptySubsequences: false)
                .first.map { part in part.trimmingCharacters(in: .whitespaces) } ?? ""
            if code.hasPrefix("[") {
                let table = code.dropFirst().dropLast()
                    .trimmingCharacters(in: .whitespaces)
                inside = code.hasSuffix("]") && table == "ui"
                continue
            }
            guard inside else { continue }
            let parts = code.split(separator: "=", maxSplits: 1)
            guard parts.count == 2,
                  parts[0].trimmingCharacters(in: .whitespaces) == key
            else { continue }
            values.append(parts[1].trimmingCharacters(in: .whitespaces))
        }
        return values
    }
}

/// The preferences of a client. The default profile keeps them where the
/// system keeps an application's defaults; an isolated profile keeps them in
/// a file of its home, one for each client, so that nothing of it lives
/// outside that folder.
public final class ProfilePreferences: @unchecked Sendable {
    public static let current = ProfilePreferences(
        profile: .current, client: ProcessInfo.processInfo.processName)

    private enum Store {
        /// The defaults of the system: the default profile.
        case system
        /// A file of an isolated profile.
        case file(URL)
        /// Nowhere: the file of an isolated profile leads out of its home,
        /// so it is neither read nor written and the preferences live for
        /// this run only.
        case memory
    }

    private let store: Store
    private let lock = NSLock()
    private var values: [String: Any] = [:]

    public init(profile: MnemonicProfile, client: String) {
        let url = profile.preferencesURL(client: client)
        if !profile.isolated {
            store = .system
        } else if !profile.contains(url) {
            store = .memory
        } else {
            store = .file(url)
            let stored = (try? Data(contentsOf: url)).flatMap { data in
                try? PropertyListSerialization.propertyList(from: data, format: nil)
            }
            values = stored as? [String: Any] ?? [:]
        }
    }

    public func object(forKey key: String) -> Any? {
        if case .system = store { return UserDefaults.standard.object(forKey: key) }
        lock.lock()
        defer { lock.unlock() }
        return values[key]
    }

    public func bool(forKey key: String) -> Bool {
        (object(forKey: key) as? NSNumber)?.boolValue ?? false
    }

    public func string(forKey key: String) -> String? {
        object(forKey: key) as? String
    }

    public func double(forKey key: String) -> Double {
        (object(forKey: key) as? NSNumber)?.doubleValue ?? 0
    }

    public func set(_ value: Any?, forKey key: String) {
        if case .system = store {
            UserDefaults.standard.set(value, forKey: key)
            return
        }
        lock.lock()
        defer { lock.unlock() }
        values[key] = value
        guard case .file(let url) = store,
              let data = try? PropertyListSerialization.data(
                fromPropertyList: values, format: .xml, options: 0)
        else { return }
        try? data.write(to: url, options: .atomic)
    }
}
