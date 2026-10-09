import XCTest
@testable import MnemonicShared

final class ProfileTests: XCTestCase {
    private var scratch: URL!
    private var userHome: URL!

    override func setUpWithError() throws {
        scratch = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-profile-\(UUID().uuidString)")
        userHome = scratch.appendingPathComponent("owner")
        try FileManager.default.createDirectory(at: userHome, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: scratch)
    }

    private func resolve(
        _ environment: [String: String] = [:], arguments: [String] = ["client"]
    ) throws -> MnemonicProfile {
        try MnemonicProfile.resolve(
            environment: environment, arguments: arguments, userHome: userHome)
    }

    private func profileHome(_ name: String = "fixture") throws -> URL {
        let home = scratch.appendingPathComponent(name)
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        return home
    }

    // MARK: The default profile

    func testWithoutASelectionTheDefaultStoreIsUsed() throws {
        let profile = try resolve()
        XCTAssertFalse(profile.isolated)
        XCTAssertEqual(profile.home.path, userHome.appendingPathComponent(".mnemonic").path)
        XCTAssertEqual(
            profile.tokenURL.path, userHome.appendingPathComponent(".mnemonic/auth.token").path)
        XCTAssertEqual(profile.endpoint.port, MnemonicProfile.defaultPort)
        XCTAssertEqual(profile.commandArguments, [])
        XCTAssertEqual(
            profile.command.path, userHome.appendingPathComponent(".cargo/bin/mnemonic").path)
        XCTAssertEqual(
            profile.configURL.path,
            userHome.appendingPathComponent(".config/mnemonic/config.toml").path)
    }

    func testTheDefaultProfileReadsItsPortFromItsConfiguration() throws {
        let folder = userHome.appendingPathComponent(".config/mnemonic")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try "[daemon]\nport = 1\n\n[ui]\nenabled = true\nport = 4811 # local\n"
            .write(to: folder.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
        XCTAssertEqual(try resolve().endpoint.port, 4811)
    }

    // MARK: An isolated profile

    func testASelectedHomeKeepsEverythingInside() throws {
        let home = try profileHome()
        try "[ui]\nport = 4822\n".write(
            to: home.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        XCTAssertTrue(profile.isolated)
        XCTAssertEqual(profile.home.path, home.path)
        XCTAssertEqual(profile.tokenURL.path, home.appendingPathComponent("auth.token").path)
        XCTAssertEqual(profile.configURL.path, home.appendingPathComponent("config.toml").path)
        XCTAssertEqual(profile.logURL.path, home.appendingPathComponent("daemon.log").path)
        XCTAssertEqual(profile.endpoint.port, 4822)
        XCTAssertEqual(profile.commandArguments, ["--home", home.path])
        XCTAssertNil(profile.sessionConfiguration().urlCache)
    }

    func testAnIsolatedProfileNeverReadsTheOwnersPort() throws {
        let folder = userHome.appendingPathComponent(".config/mnemonic")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        try "[ui]\nport = 4811\n".write(
            to: folder.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
        let home = try profileHome()
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        XCTAssertEqual(profile.endpoint.port, MnemonicProfile.defaultPort)
    }

    func testTheFlagSelectsAndMustAgreeWithTheVariable() throws {
        let home = try profileHome()
        let other = try profileHome("other")
        XCTAssertEqual(try resolve(arguments: ["client", "--home", home.path]).home.path, home.path)
        XCTAssertNoThrow(
            try resolve([MnemonicProfile.homeVariable: home.path],
                        arguments: ["client", "--home", home.path]))
        XCTAssertThrowsError(
            try resolve([MnemonicProfile.homeVariable: other.path],
                        arguments: ["client", "--home", home.path]))
        XCTAssertThrowsError(try resolve(arguments: ["client", "--home"])) { error in
            XCTAssertEqual(error as? MnemonicProfile.Failure, .flagWithoutValue)
        }
    }

    func testTheFlagIsTakenInBothSpellings() throws {
        let home = try profileHome()
        let joined = try resolve(arguments: ["client", "--home=\(home.path)"])
        XCTAssertTrue(joined.isolated)
        XCTAssertEqual(joined.home.path, home.path)
        XCTAssertEqual(joined.commandArguments, ["--home", home.path])
        XCTAssertThrowsError(try resolve(arguments: ["client", "--home="])) { error in
            XCTAssertEqual(error as? MnemonicProfile.Failure, .empty)
        }
        XCTAssertThrowsError(try resolve(arguments: ["client", "--home=relative"]))
    }

    // MARK: Files that lead out of the profile

    func testAConfigurationThatLeadsOutOfTheProfileIsRefused() throws {
        let folder = userHome.appendingPathComponent(".config/mnemonic")
        try FileManager.default.createDirectory(at: folder, withIntermediateDirectories: true)
        let owners = folder.appendingPathComponent("config.toml")
        try "[ui]\nport = 4811\n".write(to: owners, atomically: true, encoding: .utf8)
        let home = try profileHome()
        try FileManager.default.createSymbolicLink(
            at: home.appendingPathComponent("config.toml"), withDestinationURL: owners)
        XCTAssertThrowsError(try resolve([MnemonicProfile.homeVariable: home.path])) { error in
            XCTAssertEqual(error as? MnemonicProfile.Failure, .outside("config.toml"))
        }
    }

    func testATokenThatLeadsOutOfTheProfileIsNotRead() throws {
        let owners = userHome.appendingPathComponent(".mnemonic")
        try FileManager.default.createDirectory(at: owners, withIntermediateDirectories: true)
        let ownersToken = owners.appendingPathComponent("auth.token")
        try "token-of-the-owner\n".write(to: ownersToken, atomically: true, encoding: .utf8)
        let home = try profileHome()
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        XCTAssertNil(profile.token(), "no token yet")
        try FileManager.default.createSymbolicLink(at: profile.tokenURL, withDestinationURL: ownersToken)
        XCTAssertNil(profile.token())
        try FileManager.default.removeItem(at: profile.tokenURL)
        try "token-of-the-profile\n".write(to: profile.tokenURL, atomically: true, encoding: .utf8)
        XCTAssertEqual(profile.token(), "token-of-the-profile")
        // The default profile reads its own token wherever it lies.
        XCTAssertEqual(try resolve().token(), "token-of-the-owner")
    }

    func testALinkThatLeadsNowhereIsNotInsideTheProfile() throws {
        let home = try profileHome()
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        try FileManager.default.createSymbolicLink(
            at: profile.identityURL,
            withDestinationURL: scratch.appendingPathComponent("not-there"))
        XCTAssertFalse(profile.contains(profile.identityURL))
        XCTAssertThrowsError(try profile.identity()) { error in
            XCTAssertEqual(error as? MnemonicProfile.Failure, .outside("profile.id"))
        }
    }

    func testPreferencesThatLeadOutOfTheProfileAreNeitherReadNorWritten() throws {
        let home = try profileHome()
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        let outside = scratch.appendingPathComponent("elsewhere.plist")
        let planted = try PropertyListSerialization.data(
            fromPropertyList: ["page": 4], format: .xml, options: 0)
        try planted.write(to: outside)
        try FileManager.default.createSymbolicLink(
            at: profile.preferencesURL(client: "widget"), withDestinationURL: outside)
        let preferences = ProfilePreferences(profile: profile, client: "widget")
        XCTAssertNil(preferences.object(forKey: "page"))
        preferences.set(2, forKey: "page")
        XCTAssertEqual(preferences.object(forKey: "page") as? Int, 2)
        XCTAssertEqual(try Data(contentsOf: outside), planted)
    }

    func testASelectionThatCannotBeUsedIsAnErrorNotTheDefaultStore() throws {
        let cases: [(String, MnemonicProfile.Failure)] = [
            ("", .empty),
            ("relative/dir", .relative("relative/dir")),
            ("/tmp/a/../b", .parent("/tmp/a/../b")),
            (userHome.path, .notDedicated(userHome.path)),
            (scratch.path, .notDedicated(scratch.path)),
            (userHome.appendingPathComponent(".mnemonic").path,
             .overlapsDefault(userHome.appendingPathComponent(".mnemonic").path)),
            (userHome.appendingPathComponent(".mnemonic/inner").path,
             .overlapsDefault(userHome.appendingPathComponent(".mnemonic/inner").path)),
            (userHome.appendingPathComponent(".config").path,
             .overlapsDefault(userHome.appendingPathComponent(".config").path)),
        ]
        for (raw, expected) in cases {
            XCTAssertThrowsError(try resolve([MnemonicProfile.homeVariable: raw]), raw) { error in
                XCTAssertEqual(error as? MnemonicProfile.Failure, expected, raw)
            }
        }
    }

    // MARK: The endpoint and the command

    func testAnEndpointOfThisMachineIsTakenAndAnyOtherRefused() throws {
        let home = try profileHome()
        let named = try resolve([
            MnemonicProfile.homeVariable: home.path,
            MnemonicProfile.endpointVariable: "http://localhost:4833",
        ])
        XCTAssertEqual(named.endpoint.absoluteString, "http://localhost:4833")
        for raw in ["https://localhost:4833", "http://memory.example:4833", "localhost:4833", ""] {
            XCTAssertThrowsError(
                try resolve([MnemonicProfile.endpointVariable: raw]), raw
            ) { error in
                XCTAssertEqual(error as? MnemonicProfile.Failure, .endpoint(raw), raw)
            }
        }
    }

    func testTheCommandCanBeNamedAndMustBeAbsolute() throws {
        let named = try resolve([MnemonicProfile.commandVariable: "/opt/build/mnemonic"])
        XCTAssertEqual(named.command.path, "/opt/build/mnemonic")
        XCTAssertThrowsError(try resolve([MnemonicProfile.commandVariable: "mnemonic"]))
    }

    func testThePortIsReadOnlyFromItsSection() throws {
        let file = scratch.appendingPathComponent("config.toml")
        let texts: [(String, Int?)] = [
            ("[ui]\nport = 4100\n", 4100),
            ("[ui]\nport=4101\n", 4101),
            ("[other]\nport = 4102\n[ui]\nenabled = true\n", nil),
            ("[ui]\nportal = 4103\n", nil),
            ("[ui]\nport = 0\n", nil),
            ("[ui]\nport = seventy\n", nil),
            ("[ui] # the local API\nport = 4105\n", 4105),
            ("[ ui ]\nport = 4106 # chosen\n", 4106),
            ("[ui.extra]\nport = 4107\n", nil),
            ("[[ui]]\nport = 4108\n", nil),
            ("# [ui]\nport = 4109\n", nil),
            ("port = 4104\n", nil),
        ]
        for (text, expected) in texts {
            try text.write(to: file, atomically: true, encoding: .utf8)
            XCTAssertEqual(MnemonicProfile.configuredPort(in: file), expected, text)
        }
        XCTAssertNil(MnemonicProfile.configuredPort(in: scratch.appendingPathComponent("absent")))
    }

    // MARK: Preferences

    func testAnIsolatedClientKeepsItsPreferencesInItsHome() throws {
        let home = try profileHome()
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        let first = ProfilePreferences(profile: profile, client: "widget")
        XCTAssertNil(first.object(forKey: "page"))
        XCTAssertFalse(first.bool(forKey: "moved"))
        first.set(3, forKey: "page")
        first.set("expanded", forKey: "size")
        first.set(true, forKey: "moved")
        first.set(712.5, forKey: "height")
        XCTAssertTrue(FileManager.default.fileExists(
            atPath: profile.preferencesURL(client: "widget").path))
        let second = ProfilePreferences(profile: profile, client: "widget")
        XCTAssertEqual(second.object(forKey: "page") as? Int, 3)
        XCTAssertEqual(second.string(forKey: "size"), "expanded")
        XCTAssertTrue(second.bool(forKey: "moved"))
        XCTAssertEqual(second.double(forKey: "height"), 712.5)
    }

    func testTwoClientsOfOneProfileDoNotLoseEachOthersPreferences() throws {
        let home = try profileHome()
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        let widget = ProfilePreferences(profile: profile, client: "widget")
        let app = ProfilePreferences(profile: profile, client: "app")
        widget.set(3, forKey: "page")
        app.set("dark", forKey: "theme")
        widget.set(4, forKey: "page")
        XCTAssertEqual(
            ProfilePreferences(profile: profile, client: "app").string(forKey: "theme"), "dark")
        XCTAssertEqual(
            ProfilePreferences(profile: profile, client: "widget").object(forKey: "page") as? Int, 4)
    }

    func testANamedEndpointIsMarkedAsNamed() throws {
        XCTAssertFalse(try resolve().endpointNamed)
        XCTAssertTrue(
            try resolve([MnemonicProfile.endpointVariable: "http://localhost:4833"]).endpointNamed)
    }

    // MARK: The identity

    func testTheIdentityIsMadeOnceAndReadAfterwards() throws {
        let home = scratch.appendingPathComponent("not-made-yet")
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        let made = try profile.identity()
        XCTAssertEqual(made.count, 32)
        XCTAssertEqual(try profile.identity(), made)
        XCTAssertEqual(
            try String(contentsOf: profile.identityURL, encoding: .utf8), made)
        let leftovers = try FileManager.default.contentsOfDirectory(atPath: home.path)
        XCTAssertEqual(leftovers, ["profile.id"])
    }

    func testTwoThatMakeTheIdentityAtOnceReadTheSameValue() throws {
        let home = try profileHome()
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        let lock = NSLock()
        var values: [String] = []
        DispatchQueue.concurrentPerform(iterations: 24) { _ in
            let value = try? profile.identity()
            lock.lock()
            values.append(value ?? "failed")
            lock.unlock()
        }
        XCTAssertEqual(Set(values).count, 1, "\(Set(values))")
        XCTAssertEqual(values.first?.count, 32)
    }

    func testAValueThatIsNotAnIdentityIsNotReadAsOne() throws {
        let home = try profileHome()
        let profile = try resolve([MnemonicProfile.homeVariable: home.path])
        try "not an identity".write(to: profile.identityURL, atomically: true, encoding: .utf8)
        XCTAssertThrowsError(try profile.identity())
    }
}
