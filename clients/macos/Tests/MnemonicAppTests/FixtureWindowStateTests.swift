import Darwin
import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class FixtureWindowStateTests: XCTestCase {
    private struct Marker: Decodable, Equatable {
        let visible: Bool
        let route: String
        let ready: Bool
    }

    private struct Stamp: Equatable {
        let values: [Int64]

        init(_ url: URL) throws {
            var info = stat()
            let result = url.withUnsafeFileSystemRepresentation { path in
                guard let path else { return Int32(-1) }
                return lstat(path, &info)
            }
            guard result == 0 else { throw POSIXError(.ENOENT) }
            values = [Int64(info.st_dev), Int64(info.st_ino),
                      Int64(info.st_mtimespec.tv_sec), Int64(info.st_mtimespec.tv_nsec),
                      Int64(info.st_ctimespec.tv_sec), Int64(info.st_ctimespec.tv_nsec)]
        }
    }

    private struct ProfileFixture {
        let root: URL
        let profile: MnemonicProfile
        let usesCanonicalAlias: Bool

        init() throws {
            let scratch = FileManager.default.temporaryDirectory
                .appendingPathComponent("mn-window-state-\(UUID().uuidString)")
            root = scratch
            let home = scratch.appendingPathComponent("profile")
            let user = scratch.appendingPathComponent("user")
            do {
                try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
                try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
                try "[ui]\nenabled = false\nport = 9\n".write(
                    to: home.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
                let resolved = home.resolvingSymlinksInPath()
                let privateAlias = URL(fileURLWithPath: "/private" + resolved.path, isDirectory: true)
                let available = resolved.path.hasPrefix("/var/")
                    && FileManager.default.fileExists(atPath: privateAlias.path)
                usesCanonicalAlias = available
                profile = try MnemonicProfile.resolve(
                    environment: [MnemonicProfile.homeVariable: available ? privateAlias.path : home.path],
                    arguments: ["fixture-client"], userHome: user)
            } catch {
                try? FileManager.default.removeItem(at: scratch)
                throw error
            }
        }

        var marker: URL {
            profile.home.resolvingSymlinksInPath().appendingPathComponent("fixture-window.state")
        }

        func read() throws -> Marker { try JSONDecoder().decode(Marker.self, from: Data(contentsOf: marker)) }
        func remove() { try? FileManager.default.removeItem(at: root) }
    }

    private func readyValues() throws -> (ProjectBoard, LiveSnapshot) {
        let fixture = try ProjectTestFixtures()
        func object<T: Encodable>(_ value: T) throws -> [String: Any] {
            try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(value)) as? [String: Any])
        }
        var board = try object(fixture.board)
        var live = try object(fixture.live)
        let boardRow = try XCTUnwrap((board["projects"] as? [[String: Any]])?.first)
        let liveRow = try XCTUnwrap((live["projects"] as? [[String: Any]])?.first)
        let keys = ["fixture-00", "fixture-01"]
        board["projects"] = keys.map { key in
            var row = boardRow
            row["key"] = key
            row["name"] = key
            return row
        }
        live["projects"] = keys.map { key in
            var row = liveRow
            row["key"] = key
            row["name"] = key
            row["plan"] = boardRow["plan"]
            return row
        }
        let decoder = JSONDecoder()
        return (try decoder.decode(ProjectBoard.self, from: JSONSerialization.data(withJSONObject: board)),
                try decoder.decode(LiveSnapshot.self, from: JSONSerialization.data(withJSONObject: live)))
    }

    @MainActor
    private func makeModel(_ profile: MnemonicProfile, clock: ProjectTestClock) -> MnemonicAppModel {
        let controller = AppProjectController(clock: clock, ask: {
            XCTFail("Writing a fixture marker must not request live state")
            return .failed(.off)
        }, board: {
            XCTFail("Writing a fixture marker must not request a project board")
            return .failure(.off)
        }, detail: { _ in
            XCTFail("Writing a fixture marker must not request project detail")
            return .failure(.off)
        })
        let model = MnemonicAppModel(profile: profile, controller: controller)
        model.activateWindow = {}
        return model
    }

    @MainActor
    func testNewFalseMarkerIsCreatedThroughTheTemporaryPathAliasAndEqualStateIsUntouched() throws {
        let fixture = try ProfileFixture()
        defer { fixture.remove() }
        if fixture.usesCanonicalAlias {
            XCTAssertNotEqual(fixture.profile.home.path, fixture.profile.home.resolvingSymlinksInPath().path)
        }
        let clock = ProjectTestClock()
        let model = makeModel(fixture.profile, clock: clock)
        let writer = FixtureWindowState(profile: fixture.profile)
        XCTAssertFalse(FileManager.default.fileExists(atPath: fixture.marker.path))

        writer.update(model: model)
        XCTAssertEqual(try fixture.read(), Marker(visible: false, route: "projects", ready: false))
        let original = try Stamp(fixture.marker)
        writer.update(model: model)
        writer.update(model: model)
        XCTAssertEqual(try Stamp(fixture.marker), original)
        XCTAssertEqual(clock.starts, 0)
    }

    @MainActor
    func testReadinessNeedsBothLoadedRoutesAndEqualReadyStateDoesNotRewrite() throws {
        let fixture = try ProfileFixture()
        defer { fixture.remove() }
        let clock = ProjectTestClock()
        let model = makeModel(fixture.profile, clock: clock)
        defer { model.projects.hide() }
        let writer = FixtureWindowState(profile: fixture.profile)
        let (board, live) = try readyValues()

        model.projects.present(board: board, detail: nil, live: .failed(.off), route: .projects)
        writer.update(model: model)
        XCTAssertEqual(try fixture.read(), Marker(visible: true, route: "projects", ready: false))
        model.projects.present(board: nil, detail: nil, live: .snapshot(live), route: .projects)
        writer.update(model: model)
        XCTAssertFalse(try fixture.read().ready)

        model.projects.present(board: board, detail: nil, live: .snapshot(live), route: .projects)
        writer.update(model: model)
        XCTAssertEqual(try fixture.read(), Marker(visible: true, route: "projects", ready: true))
        let ready = try Stamp(fixture.marker)
        writer.update(model: model)
        XCTAssertEqual(try Stamp(fixture.marker), ready)
        XCTAssertEqual(clock.starts, 0)
    }

    @MainActor
    func testHideAndReopenChangeTheMarkerStampWithoutStartingAClock() throws {
        let fixture = try ProfileFixture()
        defer { fixture.remove() }
        let clock = ProjectTestClock()
        let model = makeModel(fixture.profile, clock: clock)
        defer { model.projects.hide() }
        let writer = FixtureWindowState(profile: fixture.profile)
        let (board, live) = try readyValues()
        model.projects.present(board: board, detail: nil, live: .snapshot(live), route: .projects)
        writer.update(model: model)
        let open = try Stamp(fixture.marker)

        model.projects.hide()
        writer.update(model: model)
        XCTAssertEqual(try fixture.read(), Marker(visible: false, route: "projects", ready: false))
        let hidden = try Stamp(fixture.marker)
        XCTAssertNotEqual(hidden, open)
        model.projects.present(board: board, detail: nil, live: .snapshot(live), route: .projects)
        writer.update(model: model)
        XCTAssertEqual(try fixture.read(), Marker(visible: true, route: "projects", ready: true))
        XCTAssertNotEqual(try Stamp(fixture.marker), hidden)
        XCTAssertNotEqual(try Stamp(fixture.marker), open)
        XCTAssertEqual(clock.starts, 0)
    }

    @MainActor
    func testOutsideAndDanglingMarkerSymlinksAreLeftUntouched() throws {
        let fixture = try ProfileFixture()
        defer { fixture.remove() }
        let clock = ProjectTestClock()
        let model = makeModel(fixture.profile, clock: clock)
        let writer = FixtureWindowState(profile: fixture.profile)
        let outside = fixture.root.appendingPathComponent("outside-state")
        let original = Data("synthetic outside state".utf8)
        try original.write(to: outside)
        let missing = fixture.root.appendingPathComponent("missing-state")

        for target in [outside, missing] {
            try FileManager.default.createSymbolicLink(at: fixture.marker, withDestinationURL: target)
            writer.update(model: model)
            XCTAssertEqual(try FileManager.default.destinationOfSymbolicLink(atPath: fixture.marker.path), target.path)
            XCTAssertEqual(try Data(contentsOf: outside), original)
            XCTAssertFalse(FileManager.default.fileExists(atPath: missing.path))
            try FileManager.default.removeItem(at: fixture.marker)
        }
        XCTAssertEqual(clock.starts, 0)
    }
}
