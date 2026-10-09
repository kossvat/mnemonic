import MnemonicShared
import XCTest
@testable import MnemonicBar

/// No process is started. Explicit lifecycle events stand in for launch,
/// readiness and termination, including events from an older process.
final class DashboardLaunchStateTests: XCTestCase {
    private let identity = "0123456789abcdef0123456789abcdef"
    private let otherIdentity = "fedcba9876543210fedcba9876543210"

    private func launch(_ digit: String = "1", identity: String? = nil) throws -> AppLinkReadiness {
        try XCTUnwrap(AppLinkReadiness(
            identity: identity ?? self.identity, nonce: String(repeating: digit, count: 32)))
    }

    func testTheLatestClickBeforeReadinessIsDeliveredOnce() throws {
        let launch = try launch()
        var state = DashboardLaunchState()
        XCTAssertTrue(state.begin(launch, link: .project("alpha")))
        XCTAssertFalse(state.isReady)
        XCTAssertEqual(state.pending, .project("alpha"))
        XCTAssertNil(state.request(.project("beta"), identity: identity))
        XCTAssertEqual(state.pending, .project("beta"))
        XCTAssertEqual(state.received(launch), .project("beta"))
        XCTAssertTrue(state.isReady)
        XCTAssertNil(state.pending)
        XCTAssertNil(state.received(launch))
        XCTAssertEqual(state.request(.search, identity: identity), .search)
        XCTAssertNil(state.pending)
    }

    func testWrongProfileOrLaunchCannotReleaseOrReplaceThePendingRoute() throws {
        let launch = try launch()
        var state = DashboardLaunchState()
        state.begin(launch, link: .project("alpha"))
        XCTAssertNil(state.received(try self.launch("2")))
        XCTAssertNil(state.received(try self.launch(identity: otherIdentity)))
        XCTAssertNil(state.request(.project("other"), identity: otherIdentity))
        XCTAssertNil(state.request(.project("../other"), identity: identity))
        XCTAssertFalse(state.isReady)
        XCTAssertEqual(state.pending, .project("alpha"))
        XCTAssertEqual(state.readiness, launch)
        XCTAssertEqual(state.received(launch), .project("alpha"))
    }

    func testFailedLaunchAndRetryResetPendingAndReadiness() throws {
        let failed = try launch()
        let retry = try launch("2")
        var state = DashboardLaunchState()
        state.begin(failed, link: .project("alpha"))
        XCTAssertNil(state.request(.project("beta"), identity: identity))
        XCTAssertTrue(state.ended(failed))
        XCTAssertNil(state.readiness)
        XCTAssertNil(state.pending)
        XCTAssertFalse(state.isReady)
        XCTAssertNil(state.received(failed))
        XCTAssertTrue(state.begin(retry, link: .projects))
        XCTAssertFalse(state.isReady)
        XCTAssertEqual(state.pending, .projects)
        XCTAssertNil(state.received(failed))
        XCTAssertEqual(state.received(retry), .projects)
    }

    func testLateExitFromAnOlderProcessCannotClearItsReplacement() throws {
        let old = try launch()
        let replacement = try launch("2")
        var state = DashboardLaunchState()
        state.begin(old, link: .project("alpha"))
        XCTAssertEqual(state.received(old), .project("alpha"))
        state.begin(replacement, link: .project("beta"))
        XCTAssertFalse(state.isReady)
        XCTAssertFalse(state.ended(old))
        XCTAssertNil(state.received(old))
        XCTAssertEqual(state.readiness, replacement)
        XCTAssertEqual(state.pending, .project("beta"))
        XCTAssertEqual(state.received(replacement), .project("beta"))
        XCTAssertFalse(state.ended(old))
        XCTAssertTrue(state.isReady)
        XCTAssertEqual(state.request(.search, identity: identity), .search)
        XCTAssertTrue(state.ended(replacement))
        XCTAssertNil(state.request(.projects, identity: identity))
        XCTAssertNil(state.readiness)
        XCTAssertNil(state.pending)
        XCTAssertFalse(state.isReady)
    }

    func testExitBeforeReadinessDiscardsTheUndeliverableRoute() throws {
        let launch = try launch()
        var state = DashboardLaunchState()
        state.begin(launch, link: .project("alpha"))
        _ = state.request(.project("beta"), identity: identity)
        XCTAssertTrue(state.ended(launch))
        XCTAssertNil(state.received(launch))
        XCTAssertFalse(state.ended(launch))
        XCTAssertNil(state.pending)
    }

    func testLaterClickExpiresStartupWithoutLosingTheOwnedProcessIdentity() throws {
        let launch = try launch()
        var state = DashboardLaunchState()
        XCTAssertTrue(state.begin(launch, link: .project("alpha"), now: 100))
        XCTAssertNil(state.request(.project("beta"), identity: identity))
        XCTAssertFalse(state.rejectExpired(now: 129.9))
        XCTAssertEqual(state.pending, .project("beta"))
        XCTAssertTrue(state.rejectExpired(now: 130))
        XCTAssertTrue(state.failed)
        XCTAssertNil(state.pending)
        XCTAssertEqual(state.readiness, launch)
        XCTAssertNil(state.request(.search, identity: identity))
        XCTAssertNil(state.received(launch))
        XCTAssertTrue(state.rejectExpired(now: 140))
        XCTAssertTrue(state.ended(launch))
        XCTAssertFalse(state.failed)
        XCTAssertNil(state.readiness)
        let retry = try self.launch("2")
        XCTAssertTrue(state.begin(retry, link: .projects, now: 200))
        XCTAssertFalse(state.rejectExpired(now: 201))
        XCTAssertEqual(state.received(retry), .projects)
    }

    func testReadyLaunchNeverExpiresAndNonAdvancingClockDoesNotExpireStartup() throws {
        let launch = try launch()
        var state = DashboardLaunchState()
        XCTAssertTrue(state.begin(launch, link: .projects, now: 100))
        XCTAssertFalse(state.rejectExpired(now: 99))
        XCTAssertFalse(state.rejectExpired(now: 100))
        XCTAssertEqual(state.received(launch), .projects)
        XCTAssertFalse(state.rejectExpired(now: 10_000))
        XCTAssertEqual(state.request(.search, identity: identity), .search)
        XCTAssertFalse(state.failed)
    }
}
