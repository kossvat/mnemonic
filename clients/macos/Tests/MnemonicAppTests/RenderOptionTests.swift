import Darwin
import Foundation
import XCTest

final class RenderOptionTests: XCTestCase {
    private var package: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
    }

    private var crate: URL {
        package.deletingLastPathComponent().deletingLastPathComponent()
    }

    private func documentedOptions() throws -> [String] {
        let readme = try String(contentsOf: package.appendingPathComponent("README.md"), encoding: .utf8)
        let heading = "## MnemonicApp render options\n"
        let start = try XCTUnwrap(readme.range(of: heading), "The README needs its app render option table.")
        let section = readme[start.upperBound...].components(separatedBy: "\n## ")[0]
        let options = section.components(separatedBy: .newlines).compactMap { line -> String? in
            guard line.hasPrefix("| `--render-") else { return nil }
            return line.components(separatedBy: "`").dropFirst().first
        }
        XCTAssertFalse(options.isEmpty, "The app render option table must not be empty.")
        return options
    }

    func testDocumentedRenderOptionsEqualTheAppParser() throws {
        let source = try String(contentsOf: package.appendingPathComponent("Sources/MnemonicApp/App.swift"),
                                encoding: .utf8)
        let pattern = #"arguments\.contains\("(--render-[a-z-]+)"\)"#
        let expression = try NSRegularExpression(pattern: pattern)
        let parsed = expression.matches(in: source, range: NSRange(source.startIndex..., in: source)).compactMap {
            match -> String? in
            guard let range = Range(match.range(at: 1), in: source) else { return nil }
            return String(source[range])
        }
        let documented = try documentedOptions().compactMap { syntax in
            syntax.split(separator: " ").first.map(String.init)
        }
        XCTAssertFalse(parsed.isEmpty, "No render dispatch was found in App.swift.")
        XCTAssertEqual(documented.count, Set(documented).count, "Document each app option once in the table.")
        XCTAssertEqual(Set(documented), Set(parsed), "The README and the app parser must name the same options.")
    }

    func testEveryDocumentedRenderOptionExitsAndWritesAPicture() async throws {
        let executable = try builtApp()
        for syntax in try documentedOptions() {
            try await render(syntax, executable: executable)
        }
    }

    private func builtApp() throws -> URL {
        // Use the product beside this test bundle, never another configuration
        // or an installed app that might predate the source under test.
        let bundle = Bundle(for: RenderOptionTests.self).bundleURL.resolvingSymlinksInPath()
        let executable = bundle.deletingLastPathComponent().appendingPathComponent("MnemonicApp")
        guard FileManager.default.isExecutableFile(atPath: executable.path) else {
            throw XCTSkip("MnemonicApp is absent beside the test bundle at \(executable.path). Build the app product in the same Swift package configuration, then rerun this test.")
        }
        return executable
    }

    private func arguments(for syntax: String, output: URL) throws -> [String] {
        let tokens = syntax.replacingOccurrences(of: "[", with: "")
            .replacingOccurrences(of: "]", with: "").split(separator: " ").map(String.init)
        let option = try XCTUnwrap(tokens.first)
        let input = option == "--render-graph-fixtures"
            ? package.appendingPathComponent("Tests/Fixtures/Graph")
            : crate.appendingPathComponent("docs/screenshots/demo")
        return try tokens.map { token in
            switch token {
            case "DIRECTORY": return input.path
            case "OUTPUT": return output.path
            // One state covers the selector without drawing the whole catalog.
            case "ID": return "projects-enriched"
            default:
                guard token.hasPrefix("--") else {
                    throw NSError(domain: "RenderOptionTests", code: 1,
                                  userInfo: [NSLocalizedDescriptionKey: "Unknown README argument: \(token)"])
                }
                return token
            }
        }
    }

    private func render(_ syntax: String, executable: URL) async throws {
        let manager = FileManager.default
        let root = manager.temporaryDirectory.appendingPathComponent("mnemonic-render-\(UUID().uuidString)",
                                                                     isDirectory: true)
        try manager.createDirectory(at: root, withIntermediateDirectories: true)
        defer { try? manager.removeItem(at: root) }
        let profile = root.appendingPathComponent("profile", isDirectory: true)
        let temporary = root.appendingPathComponent("tmp", isDirectory: true)
        let userDirectory = root.appendingPathComponent("user", isDirectory: true)
        for directory in [profile, temporary, userDirectory] {
            try manager.createDirectory(at: directory, withIntermediateDirectories: true)
        }
        try "[ui]\nenabled = false\n".write(to: profile.appendingPathComponent("config.toml"),
                                          atomically: true, encoding: .utf8)
        let output = root.appendingPathComponent("output", isDirectory: true)
        let logURL = root.appendingPathComponent("render.log")
        try Data().write(to: logURL)
        let log = try FileHandle(forWritingTo: logURL)
        defer { try? log.close() }
        let process = Process()
        process.executableURL = executable
        process.currentDirectoryURL = root
        process.arguments = try arguments(for: syntax, output: output) + ["--home", profile.path]
        // No inherited profile, endpoint, command override or provider settings.
        // Render dispatch exits before the live model; the isolated API is off.
        process.environment = [
            "PATH": "/usr/bin:/bin", "TMPDIR": temporary.path,
            "CFFIXED_USER_HOME": userDirectory.path, "MNEMONIC_HOME": profile.path,
            "MNEMONIC_BIN": "/usr/bin/false", "TZ": "UTC", "LANG": "en_US.UTF-8"
        ]
        process.standardInput = FileHandle.nullDevice
        process.standardOutput = log
        process.standardError = log
        let exited = expectation(description: "Render exits: \(syntax)")
        process.terminationHandler = { _ in exited.fulfill() }
        try process.run()
        defer {
            if process.isRunning {
                kill(process.processIdentifier, SIGKILL)
                process.waitUntilExit()
            }
        }
        let result = await XCTWaiter.fulfillment(of: [exited], timeout: 30)
        guard result == .completed else {
            XCTFail("Render exceeded 30 seconds: \(syntax)")
            return
        }
        let diagnostic = try String(contentsOf: logURL, encoding: .utf8)
        XCTAssertEqual(process.terminationReason, .exit, "\(syntax): \(diagnostic)")
        XCTAssertEqual(process.terminationStatus, 0, "\(syntax): \(diagnostic)")
        let files = try manager.contentsOfDirectory(at: output,
            includingPropertiesForKeys: [.isRegularFileKey, .fileSizeKey])
        let pictures = try files.filter { file in
            let values = try file.resourceValues(forKeys: [.isRegularFileKey, .fileSizeKey])
            return file.pathExtension == "png" && values.isRegularFile == true && (values.fileSize ?? 0) > 0
        }
        XCTAssertFalse(pictures.isEmpty, "Render wrote no picture: \(syntax)")
    }
}
