import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class AppSearchRecoveryTests: XCTestCase {
    @MainActor
    func testSearchRecoversFromAnotherLegacyRouteFailureAndThenStopsProbingStatus() async throws {
        let fixture = try AppLegacyFixture(.memories)
        defer { fixture.close() }
        fixture.memories.answer = .failure(AppLegacyReadFailure.unavailable)
        await fixture.model.refreshMemories()
        XCTAssertNotNil(fixture.model.setupMessage)
        XCTAssertNil(fixture.model.status)
        fixture.status.held = true
        let requested = expectation(description: "Search verifies the active endpoint")
        fixture.status.requested(1) { requested.fulfill() }
        let displayed = expectation(description: "Search can replace the setup page after recovery")
        fixture.model.projects.onPresentationChange = {
            if fixture.model.status != nil && fixture.model.setupMessage == nil { displayed.fulfill() }
        }
        fixture.model.route = .search
        await fulfillment(of: [requested], timeout: 2)
        XCTAssertNotNil(fixture.model.setupMessage, "Pending recovery cannot hide a real error")
        fixture.status.finishAll()
        await fulfillment(of: [displayed], timeout: 2)
        fixture.model.projects.onPresentationChange = nil
        XCTAssertNil(fixture.model.setupMessage)
        XCTAssertNotNil(fixture.model.status)
        await fixture.model.refreshAll()
        XCTAssertEqual(fixture.status.count, 1, "Healthy Search has no automatic status polling")
    }

    @MainActor
    func testSameRouteRetryAfterRejectedEndpointKeepsFailuresUntilActualRecovery() async throws {
        let fixture = try AppLegacyFixture(.search)
        defer { fixture.close() }
        let endpoint = fixture.model.endpoint
        await fixture.model.applyEndpoint("not-an-endpoint")
        XCTAssertEqual(fixture.model.endpoint, endpoint)
        XCTAssertNotNil(fixture.model.setupMessage)
        XCTAssertNil(fixture.model.status)
        fixture.status.answer = .failure(AppLegacyReadFailure.unavailable)
        await fixture.model.refreshAll()
        XCTAssertEqual(fixture.status.count, 1)
        XCTAssertNotNil(fixture.model.setupMessage)
        XCTAssertNil(fixture.model.status)
        XCTAssertFalse(fixture.model.isOnline)

        fixture.status.answer = .success(StatusResponse(
            totalMemories: 0, byType: [], entities: 0, edges: 0, lastActivity: nil))
        await fixture.model.refreshAll()
        XCTAssertEqual(fixture.status.count, 2)
        XCTAssertNil(fixture.model.setupMessage)
        XCTAssertNotNil(fixture.model.status)
        XCTAssertTrue(fixture.model.isOnline)
        XCTAssertEqual(fixture.model.endpoint, endpoint)
    }

    @MainActor
    func testSearchRecoveryCannotPublishAfterHideOrNavigation() async throws {
        for navigate in [false, true] {
            let fixture = try AppLegacyFixture(.search)
            defer { fixture.close() }
            fixture.model.setupMessage = "Synthetic setup failure"
            fixture.status.held = true
            let requested = expectation(description: "Search recovery is held")
            fixture.status.requested(1) { requested.fulfill() }
            let reading = Task { await fixture.model.refreshAll() }
            await fulfillment(of: [requested], timeout: 2)
            if navigate { fixture.model.route = .projects }
            else {
                let window = NSObject()
                fixture.model.window(ObjectIdentifier(window), visible: false)
            }
            fixture.status.finishAll()
            await reading.value
            XCTAssertNil(fixture.model.status)
            XCTAssertEqual(fixture.model.setupMessage, "Synthetic setup failure")
            XCTAssertEqual(fixture.status.count, 1)
        }
    }
}
