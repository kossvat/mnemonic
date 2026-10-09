import Foundation
import MnemonicShared
import XCTest

/// These tests inspect the readiness protocol without system notifications.
final class AppLinkReadinessTests: XCTestCase {
    private let identity = "0123456789abcdef0123456789abcdef"
    private let otherIdentity = "fedcba9876543210fedcba9876543210"
    private let nonce = "11111111111111111111111111111111"
    private let otherNonce = "22222222222222222222222222222222"

    func testReadinessRequiresBothExpectedProfileAndLaunchNonce() throws {
        let expected = try XCTUnwrap(AppLinkReadiness(identity: identity, nonce: nonce))
        let wrongProfile = try XCTUnwrap(AppLinkReadiness(identity: otherIdentity, nonce: nonce))
        let wrongLaunch = try XCTUnwrap(AppLinkReadiness(identity: identity, nonce: otherNonce))
        XCTAssertTrue(expected.accepts(Notification(name: expected.notificationName, object: nonce)))
        XCTAssertFalse(expected.accepts(Notification(name: wrongProfile.notificationName, object: nonce)))
        XCTAssertFalse(expected.accepts(Notification(name: wrongLaunch.notificationName, object: otherNonce)))
        XCTAssertFalse(expected.accepts(Notification(name: expected.notificationName, object: otherNonce)))
        XCTAssertFalse(expected.accepts(Notification(name: expected.notificationName)))
        XCTAssertNotEqual(expected.notificationName, wrongProfile.notificationName)
        XCTAssertNotEqual(expected.notificationName, wrongLaunch.notificationName)
        XCTAssertEqual(expected.notificationName.rawValue, "mnemonic.route-ready.\(identity).\(nonce)")
    }

    func testExactlyOneSeparateReadinessArgumentSelectsTheLaunch() throws {
        let expected = try XCTUnwrap(AppLinkReadiness(identity: identity, nonce: nonce))
        let link = try XCTUnwrap(AppLink.project("alpha").url(identity: identity))
        let arguments = ["MnemonicApp", "--open-url", link.absoluteString, "--route-ready", nonce]
        XCTAssertEqual(AppLinkReadiness(arguments: arguments, identity: identity), expected)
        for invalid in [
            [], ["MnemonicApp"], ["--route-ready"], ["--route-ready", ""],
            ["--route-ready", "invalid"], ["--route-ready", identity.uppercased()],
            ["--route-ready=\(nonce)"], ["--route-ready", nonce, "--route-ready", nonce],
            ["--route-ready", nonce, "--route-ready=\(otherNonce)"],
            ["--route-ready", "--open-url", link.absoluteString],
        ] {
            XCTAssertNil(AppLinkReadiness(arguments: invalid, identity: identity))
        }
    }

    func testInvalidIdentifiersCannotCreateAReadinessSignal() {
        for invalid in ["", "invalid", String(repeating: "1", count: 31),
                        String(repeating: "1", count: 33), String(repeating: "z", count: 32),
                        identity.uppercased(), "../profile"] {
            XCTAssertNil(AppLinkReadiness(identity: invalid, nonce: nonce))
            XCTAssertNil(AppLinkReadiness(identity: identity, nonce: invalid))
            XCTAssertNil(AppLinkReadiness(arguments: ["--route-ready", nonce], identity: invalid))
        }
    }
}
