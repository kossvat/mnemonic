import CryptoKit
import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicBar

/// Synthetic byte files stand in for applications. Selection only reads
/// their metadata and never starts a process or touches a real profile.
final class DashboardCapabilityTests: XCTestCase {
    private func folder() throws -> URL {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-capability-\(UUID().uuidString)")
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        addTeardownBlock { try? FileManager.default.removeItem(at: root) }
        return root
    }

    @discardableResult
    private func executable(at url: URL, content: String = "synthetic executable") throws -> URL {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                               withIntermediateDirectories: true)
        try Data(content.utf8).write(to: url)
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: url.path)
        return url
    }

    private func plist(_ fields: [String: Any], at url: URL) throws {
        try FileManager.default.createDirectory(at: url.deletingLastPathComponent(),
                                               withIntermediateDirectories: true)
        try PropertyListSerialization.data(fromPropertyList: fields, format: .xml, options: 0).write(to: url)
    }

    private func receipt(
        for executable: URL, version: Any = AppLinkReadiness.protocolVersion, digest: String? = nil
    ) throws {
        let bytes = try Data(contentsOf: executable)
        let expected = digest ?? SHA256.hash(data: bytes).map { byte in String(format: "%02x", byte) }.joined()
        try plist([AppLinkReadiness.capabilityKey: version,
                   AppLinkReadiness.executableDigestKey: expected],
                  at: DashboardCapability.receiptURL(for: executable))
    }

    private func bundle(at root: URL, name: String, version: Any?) throws -> URL {
        let bundle = root.appendingPathComponent(name)
        try executable(at: bundle.appendingPathComponent("Contents/MacOS/MnemonicApp"))
        let fields = version.map { value in [AppLinkReadiness.capabilityKey: value] } ?? [:]
        try plist(fields, at: bundle.appendingPathComponent("Contents/Info.plist"))
        return bundle
    }

    func testVerifiedCurrentExecutablePrecedesAnOlderInstalledBundle() throws {
        let root = try folder()
        let built = try executable(at: root.appendingPathComponent("release/MnemonicApp"))
        let old = try bundle(at: root, name: "older", version: nil)
        let current = try bundle(at: root, name: "current", version: AppLinkReadiness.protocolVersion)
        try receipt(for: built)
        XCTAssertEqual(DashboardCapability.firstCompatible(built: [built], bundles: [old, current]), built)
        try FileManager.default.removeItem(at: DashboardCapability.receiptURL(for: built))
        XCTAssertEqual(DashboardCapability.firstCompatible(built: [built], bundles: [old, current]),
                       current.appendingPathComponent("Contents/MacOS/MnemonicApp"))
        XCTAssertNil(DashboardCapability.firstCompatible(built: [built], bundles: [old]))
    }

    func testSiblingExecutableIsPreferredOnlyWithItsOwnMatchingReceipt() throws {
        let root = try folder()
        let sibling = try executable(at: root.appendingPathComponent("sibling/MnemonicApp"), content: "sibling")
        let configured = try executable(at: root.appendingPathComponent("configured/MnemonicApp"), content: "configured")
        try receipt(for: configured)
        XCTAssertEqual(DashboardCapability.firstCompatible(built: [sibling, configured], bundles: []), configured)
        try receipt(for: sibling)
        XCTAssertEqual(DashboardCapability.firstCompatible(built: [sibling, configured], bundles: []), sibling)
        try executable(at: sibling, content: "rebuilt sibling")
        XCTAssertEqual(DashboardCapability.firstCompatible(built: [sibling, configured], bundles: []), configured)
    }

    func testAReceiptCannotSurviveReplacingItsExecutableOrUseAnotherFilesDigest() throws {
        let root = try folder()
        let built = try executable(at: root.appendingPathComponent("MnemonicApp"))
        try receipt(for: built)
        XCTAssertEqual(DashboardCapability.firstCompatible(built: [built], bundles: []), built)
        try executable(at: built, content: "replacement executable")
        XCTAssertNil(DashboardCapability.firstCompatible(built: [built], bundles: []))
        try receipt(for: built, digest: String(repeating: "0", count: 64))
        XCTAssertNil(DashboardCapability.firstCompatible(built: [built], bundles: []))
        try receipt(for: built)
        XCTAssertEqual(DashboardCapability.firstCompatible(built: [built], bundles: []), built)
    }

    func testUnsupportedVersionsMalformedMetadataAndNonExecutablesAreRejected() throws {
        let root = try folder()
        let built = try executable(at: root.appendingPathComponent("MnemonicApp"))
        let versions: [Any] = [0, 2, "1", true]
        for version in versions {
            try receipt(for: built, version: version)
            XCTAssertNil(DashboardCapability.firstCompatible(built: [built], bundles: []))
            let invalid = try bundle(at: root, name: "invalid", version: version)
            XCTAssertNil(DashboardCapability.firstCompatible(built: [], bundles: [invalid]))
        }
        let receiptURL = DashboardCapability.receiptURL(for: built)
        try Data("not a property list".utf8).write(to: receiptURL)
        XCTAssertNil(DashboardCapability.firstCompatible(built: [built], bundles: []))
        try Data(repeating: 65, count: 16_385).write(to: receiptURL)
        XCTAssertNil(DashboardCapability.firstCompatible(built: [built], bundles: []))
        try receipt(for: built)
        try FileManager.default.setAttributes([.posixPermissions: 0o600], ofItemAtPath: built.path)
        XCTAssertNil(DashboardCapability.firstCompatible(built: [built], bundles: []))
    }
}
