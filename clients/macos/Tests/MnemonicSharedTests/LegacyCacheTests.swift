import XCTest
@testable import MnemonicShared

final class LegacyCacheTests: XCTestCase {
    private var scratch: URL!
    private var caches: URL!

    override func setUpWithError() throws {
        scratch = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-cache-\(UUID().uuidString)")
        caches = scratch.appendingPathComponent("Library/Caches")
        try FileManager.default.createDirectory(at: caches, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: scratch)
    }

    private func profile(isolated: Bool) throws -> MnemonicProfile {
        var environment: [String: String] = [:]
        if isolated {
            let home = scratch.appendingPathComponent("profile")
            try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
            environment[MnemonicProfile.homeVariable] = home.path
        }
        return try MnemonicProfile.resolve(
            environment: environment, arguments: ["client"],
            userHome: scratch.appendingPathComponent("owner"))
    }

    private func folder(_ name: String, files: [String]) throws -> URL {
        let folder = caches.appendingPathComponent(name)
        try FileManager.default.createDirectory(
            at: folder.appendingPathComponent("fsCachedData"), withIntermediateDirectories: true)
        for file in files {
            try "kept response".write(
                to: folder.appendingPathComponent(file), atomically: true, encoding: .utf8)
        }
        return folder
    }

    func testTheCacheOfAnEarlierBuildIsRemoved() throws {
        let widget = try folder("MnemonicBar", files: ["Cache.db", "Cache.db-wal"])
        let bundled = try folder("org.example.widget", files: ["Cache.db"])
        let other = try folder("SomethingElse", files: ["Cache.db"])
        let removed = LegacyCache.remove(
            clients: ["MnemonicBar", "org.example.widget", "NeverThere"],
            caches: caches, profile: try profile(isolated: false))
        XCTAssertEqual(removed.map(\.lastPathComponent), ["MnemonicBar", "org.example.widget"])
        XCTAssertFalse(FileManager.default.fileExists(atPath: widget.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: bundled.path))
        XCTAssertTrue(FileManager.default.fileExists(atPath: other.path))
        // A second start finds nothing to remove.
        XCTAssertEqual(LegacyCache.remove(
            clients: ["MnemonicBar"], caches: caches,
            profile: try profile(isolated: false)), [])
    }

    func testAFolderThatIsNotTheCacheOfASessionIsLeft() throws {
        let folder = try folder("MnemonicBar", files: ["shaders.data"])
        XCTAssertEqual(LegacyCache.remove(
            clients: ["MnemonicBar"], caches: caches,
            profile: try profile(isolated: false)), [])
        XCTAssertTrue(FileManager.default.fileExists(atPath: folder.path))
    }

    func testAnIsolatedClientLeavesTheOwnersCacheAlone() throws {
        let folder = try folder("MnemonicBar", files: ["Cache.db"])
        XCTAssertEqual(LegacyCache.remove(
            clients: ["MnemonicBar"], caches: caches,
            profile: try profile(isolated: true)), [])
        XCTAssertTrue(FileManager.default.fileExists(atPath: folder.path))
    }

    func testNoProfileKeepsAResponseOnDisk() throws {
        for isolated in [false, true] {
            let configuration = try profile(isolated: isolated).sessionConfiguration()
            XCTAssertNil(configuration.urlCache)
            XCTAssertEqual(configuration.requestCachePolicy, .reloadIgnoringLocalCacheData)
            // An ephemeral session keeps its cookies and credentials in
            // memory too: it has no store of its own on disk.
            XCTAssertNotEqual(
                configuration.httpCookieStorage, HTTPCookieStorage.shared)
        }
    }

    func testTheNamesOfThisClientHoldItsProcessName() {
        let names = LegacyCache.clientNames()
        XCTAssertTrue(names.contains(ProcessInfo.processInfo.processName))
    }
}
