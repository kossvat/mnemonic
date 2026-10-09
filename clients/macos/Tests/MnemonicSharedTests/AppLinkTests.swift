import Foundation
import MnemonicShared
import XCTest

final class AppLinkTests: XCTestCase {
    private let identity = "0123456789abcdef0123456789abcdef"
    private let other = "fedcba9876543210fedcba9876543210"

    private func parse(_ text: String, expected: String? = nil) -> AppLink? {
        guard let url = URL(string: text) else { return nil }
        return AppLink(url: url, expectedIdentity: expected ?? identity)
    }

    func testRoutesRoundTripWithinOneExpectedProfile() throws {
        for route in [AppLink.projects, .search, .project("alpha"),
                      .project("alpha:part-two"), .project("café:étude-2")] {
            let url = try XCTUnwrap(route.url(identity: identity))
            XCTAssertEqual(AppLink(url: url, expectedIdentity: identity), route)
            XCTAssertNil(AppLink(url: url, expectedIdentity: other))
            let parts = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
            XCTAssertEqual(parts.queryItems, [URLQueryItem(name: "profile", value: identity)])
            XCTAssertNil(parts.user)
            XCTAssertNil(parts.password)
            XCTAssertNil(parts.port)
            XCTAssertNil(parts.fragment)
        }
    }

    func testProjectKeyIsOneEncodedPathComponentAndIsNeverTruncated() throws {
        let key = "café:étude-2"
        let url = try XCTUnwrap(AppLink.project(key).url(identity: identity))
        let parts = try XCTUnwrap(URLComponents(url: url, resolvingAgainstBaseURL: false))
        XCTAssertTrue(parts.percentEncodedPath.contains("%3A"))
        XCTAssertTrue(parts.percentEncodedPath.contains("%C3%A9"))
        XCTAssertEqual(parts.percentEncodedPath.split(separator: "/").count, 1)
        XCTAssertEqual(parts.path, "/\(key)")
        let long = String(repeating: "a", count: 160) + ":" + String(repeating: "b", count: 160)
        let longURL = try XCTUnwrap(AppLink.project(long).url(identity: identity))
        XCTAssertEqual(AppLink(url: longURL, expectedIdentity: identity), .project(long))
    }

    func testMalformedProjectKeysCannotBeBuiltOrParsed() {
        for key in ["", ".", "..", "../alpha", "alpha/beta", "alpha\\beta", "alpha beta",
                    "alpha?home=fixture", "alpha#fragment", "alpha%2Fbeta", ":alpha", "alpha:",
                    "alpha::beta", "-alpha", "alpha-", "alpha--beta", "alpha_1", "alpha.1",
                    "alpha\u{0000}", "alpha\n", "alpha😀"] {
            XCTAssertNil(AppLink.project(key).url(identity: identity), key)
        }
        for path in ["", "/", "/alpha/beta", "/../alpha", "/%2e%2e", "/alpha%2Fbeta",
                     "/alpha%252Fbeta", "/alpha%5Cbeta", "/alpha%00", "/alpha%20beta",
                     "/alpha%3Abeta%3A", "/%FF", "/%"] {
            XCTAssertNil(parse("mnemonic://project\(path)?profile=\(identity)"), path)
        }
    }

    func testAProfileIsRequiredAndCannotBeSelectedByTheURL() {
        for query in ["", "profile", "profile=", "profile=\(other)",
                      "profile=\(identity)&profile=\(identity)",
                      "profile=\(identity)&profile=\(other)",
                      "profile=\(identity)&endpoint=http%3A%2F%2Flocalhost%3A9999",
                      "profile=\(identity)&home=%2Ffixture", "profile=\(identity)&token=fixture",
                      "profile=\(identity)&port=9999", "profile=\(identity)&unknown=value",
                      "profile=\(identity)&", "Profile=\(identity)"] {
            XCTAssertNil(parse("mnemonic://project/alpha?\(query)"), query)
        }
        XCTAssertNil(parse("mnemonic://project/alpha"))
        XCTAssertNil(parse("mnemonic://project/alpha?profile=\(identity)", expected: ""))
        XCTAssertNil(parse("mnemonic://project/alpha?profile=\(identity)", expected: "fixture"))
    }

    func testOnlyKnownSchemeRoutesAndUnadornedAuthorityAreAccepted() {
        for text in [
            "https://remote/alpha?profile=\(identity)",
            "mnemonic:project/alpha?profile=\(identity)",
            "mnemonic:///project/alpha?profile=\(identity)",
            "mnemonic://unknown?profile=\(identity)",
            "mnemonic://profile/\(identity)",
            "mnemonic://project:3737/alpha?profile=\(identity)",
            "mnemonic://fixture@project/alpha?profile=\(identity)",
            "mnemonic://fixture:secret@project/alpha?profile=\(identity)",
            "mnemonic://project/alpha?profile=\(identity)#fragment",
            "mnemonic://project/alpha?profile=\(identity)#",
            "mnemonic://search/alpha?profile=\(identity)",
            "mnemonic://projects/alpha?profile=\(identity)",
        ] {
            XCTAssertNil(parse(text), text)
        }
        XCTAssertEqual(parse("mnemonic://search/?profile=\(identity)"), .search)
        XCTAssertEqual(parse("mnemonic://projects/?profile=\(identity)"), .projects)
    }

    func testNotificationNamesAreOpaqueAndProfileScoped() throws {
        let name = try XCTUnwrap(AppLink.notificationName(identity: identity))
        XCTAssertEqual(name.rawValue, "mnemonic.route.\(identity)")
        XCTAssertNotEqual(name, AppLink.notificationName(identity: other))
        for invalid in ["", "fixture", String(repeating: "a", count: 31),
                        String(repeating: "a", count: 33), String(repeating: "g", count: 32),
                        identity.uppercased(), "../../fixture"] {
            XCTAssertNil(AppLink.notificationName(identity: invalid))
            XCTAssertNil(AppLink.projects.url(identity: invalid))
        }
    }
}
