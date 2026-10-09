import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicBar

/// Argument construction cannot start a process or register a URL scheme.
final class WidgetAppLinkTests: XCTestCase {
    private let identity = "0123456789abcdef0123456789abcdef"

    private func profile(isolated: Bool) throws -> MnemonicProfile {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-widget-route-\(UUID().uuidString)")
        var environment = [MnemonicProfile.endpointVariable: "http://localhost:3737"]
        if isolated { environment[MnemonicProfile.homeVariable] = root.appendingPathComponent("store").path }
        return try MnemonicProfile.resolve(
            environment: environment, arguments: [], userHome: root.appendingPathComponent("user"))
    }

    func testEveryRouteKeepsProfileArgumentsAndAddsOnlyAnOpaqueProfileURL() throws {
        for isolated in [false, true] {
            let selected = try profile(isolated: isolated)
            for route in [AppLink.projects, .search, .project("demoapp:part-two"), .project("café")] {
                let arguments = try XCTUnwrap(MnemonicService.dashboardArguments(
                    profile: selected, link: route, identity: identity))
                XCTAssertEqual(Array(arguments.dropLast(2)), selected.commandArguments)
                XCTAssertEqual(arguments.suffix(2).first, "--open-url")
                let text = try XCTUnwrap(arguments.last)
                let url = try XCTUnwrap(URL(string: text))
                XCTAssertEqual(AppLink(url: url, expectedIdentity: identity), route)
                let parts = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
                XCTAssertEqual(parts.queryItems, [URLQueryItem(name: "profile", value: identity)])
                XCTAssertFalse(text.contains(selected.home.path))
                XCTAssertFalse(text.contains(selected.endpoint.absoluteString))
                XCTAssertFalse(text.contains("token"))
                XCTAssertNil(parts.user)
                XCTAssertNil(parts.password)
                XCTAssertNil(parts.port)
            }
        }
    }

    func testInvalidIdentityOrProjectCannotProduceLaunchArguments() throws {
        let selected = try profile(isolated: true)
        XCTAssertNil(MnemonicService.dashboardArguments(
            profile: selected, link: .projects, identity: "invalid"))
        XCTAssertNil(MnemonicService.dashboardArguments(
            profile: selected, link: .project("../other"), identity: identity))
        XCTAssertFalse(FileManager.default.fileExists(atPath: selected.identityURL.path))
    }

    func testLaunchReadinessNonceStaysOutsideTheRouteURL() throws {
        let nonce = "11111111111111111111111111111111"
        let selected = try profile(isolated: true)
        let arguments = try XCTUnwrap(MnemonicService.dashboardArguments(
            profile: selected, link: .project("alpha"), identity: identity, readinessNonce: nonce))
        XCTAssertEqual(Array(arguments.prefix(selected.commandArguments.count)), selected.commandArguments)
        XCTAssertEqual(Array(arguments.suffix(2)), ["--route-ready", nonce])
        XCTAssertEqual(AppLinkReadiness(arguments: arguments, identity: identity),
                       AppLinkReadiness(identity: identity, nonce: nonce))
        let index = try XCTUnwrap(arguments.firstIndex(of: "--open-url"))
        let text = arguments[index + 1]
        let url = try XCTUnwrap(URL(string: text))
        XCTAssertEqual(AppLink(url: url, expectedIdentity: identity), .project("alpha"))
        XCTAssertFalse(text.contains(nonce))
        XCTAssertFalse(text.contains(selected.home.path))
        XCTAssertNil(MnemonicService.dashboardArguments(
            profile: selected, link: .projects, identity: identity, readinessNonce: "invalid"))
    }
}
