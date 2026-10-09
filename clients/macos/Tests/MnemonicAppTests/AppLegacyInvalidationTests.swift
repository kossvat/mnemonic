import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class AppLegacyInvalidationTests: XCTestCase {
    @MainActor
    func testForgetInvalidationRevokesOldAnswerAndKeepsTheLatestSerialRead() async throws {
        let fixture = try AppLegacyFixture(.memories)
        defer { fixture.close() }
        let model = fixture.model, source = fixture.memories
        let kept = AppLegacyFixture.memory("fixture-kept")
        let removed = AppLegacyFixture.memory("fixture-removed")
        model.memories = [kept]
        source.answer = .success(MemoryListResponse(results: [kept, removed], count: 2))
        source.held = true
        let first = expectation(description: "The pre-delete list is held")
        source.requested(1) { first.fulfill() }
        let reading = Task { await model.refreshMemories() }
        await fulfillment(of: [first], timeout: 2)
        await model.refreshMemories()
        XCTAssertEqual(source.count, 1, "Appearance still coalesces")

        // The successful delete callback asks for a newer server snapshot.
        source.answer = .success(MemoryListResponse(results: [kept], count: 1))
        await model.refreshMemories(invalidate: true)
        await model.refreshMemories(invalidate: true)
        XCTAssertEqual(source.count, 1, "Invalidations wait for the current read")
        let second = expectation(description: "A single post-delete list follows")
        source.requested(2) { second.fulfill() }
        source.finishNext()
        await fulfillment(of: [second], timeout: 2)
        XCTAssertEqual(model.memories, [kept], "The revoked pre-delete list must never publish")
        XCTAssertTrue(model.isLoading)

        // A later mutation also revokes the already-started replacement.
        source.answer = .success(MemoryListResponse(results: [], count: 0))
        await model.refreshMemories(invalidate: true)
        let third = expectation(description: "The latest invalidation is retained")
        source.requested(3) { third.fulfill() }
        source.finishNext()
        await fulfillment(of: [third], timeout: 2)
        XCTAssertEqual(model.memories, [kept])
        source.finishAll()
        await reading.value
        XCTAssertTrue(model.memories.isEmpty)
        XCTAssertEqual(source.count, 3)
        XCTAssertEqual(source.peakActive, 1)
        XCTAssertFalse(model.isLoading)
        XCTAssertNil(model.setupMessage)
    }

    @MainActor
    func testMergeInvalidationCoalescesAndDoesNotPublishAnObsoleteFailure() async throws {
        let fixture = try AppLegacyFixture(.entities)
        defer { fixture.close() }
        let model = fixture.model, source = fixture.entities
        let canonical = AppLegacyFixture.entity("fixture-canonical")
        let alias = AppLegacyFixture.entity("fixture-alias")
        model.entities = [canonical, alias]
        source.answer = .failure(AppLegacyReadFailure.unavailable)
        source.held = true
        let first = expectation(description: "The pre-merge list is held")
        source.requested(1) { first.fulfill() }
        let reading = Task { await model.refreshEntities() }
        await fulfillment(of: [first], timeout: 2)
        await model.refreshEntities()
        source.answer = .success(EntitiesResponse(results: [canonical]))
        await model.refreshEntities(invalidate: true)
        await model.refreshEntities(invalidate: true)
        let second = expectation(description: "One post-merge list follows")
        source.requested(2) { second.fulfill() }
        source.finishNext()
        await fulfillment(of: [second], timeout: 2)
        XCTAssertNil(model.setupMessage, "An invalidated failure cannot replace the current route")
        XCTAssertEqual(model.entities, [canonical, alias])
        source.finishAll()
        await reading.value
        XCTAssertEqual(model.entities, [canonical])
        XCTAssertEqual(source.count, 2)
        XCTAssertEqual(source.peakActive, 1)
        XCTAssertFalse(model.isLoading)
    }

    @MainActor
    func testQueuedInvalidationsCannotReadOrPublishAfterHideOrNavigation() async throws {
        for route in [AppRoute.memories, .entities] {
            for navigate in [false, true] {
                let fixture = try AppLegacyFixture(route)
                defer { fixture.close() }
                fixture.memories.held = true
                fixture.entities.held = true
                fixture.memories.answer = .success(MemoryListResponse(
                    results: [AppLegacyFixture.memory("fixture-old")], count: 1))
                fixture.entities.answer = .success(EntitiesResponse(
                    results: [AppLegacyFixture.entity("fixture-old")]))
                let first = expectation(description: "The legacy response is held")
                if route == .memories { fixture.memories.requested(1) { first.fulfill() } }
                else { fixture.entities.requested(1) { first.fulfill() } }
                let reading = Task { await refreshRows(fixture.model, route: route) }
                await fulfillment(of: [first], timeout: 2)
                await refreshRows(fixture.model, route: route, invalidate: true)
                if navigate { fixture.model.route = .search }
                else {
                    let window = NSObject()
                    fixture.model.window(ObjectIdentifier(window), visible: false)
                }
                fixture.memories.finishAll()
                fixture.entities.finishAll()
                await reading.value
                XCTAssertTrue(fixture.model.memories.isEmpty)
                XCTAssertTrue(fixture.model.entities.isEmpty)
                XCTAssertEqual(fixture.memories.count + fixture.entities.count, 1)
                XCTAssertFalse(fixture.model.isLoading)
                XCTAssertNil(fixture.model.setupMessage)
            }
        }
    }

    @MainActor
    func testExplicitRefreshRevokesHeldStatusBeforeWaitingForLive() async throws {
        let fixture = try AppLegacyFixture(.manage)
        defer { fixture.close() }
        await fixture.model.projects.coordinator.setSurfaceVisible(true)
        fixture.status.held = true
        let first = expectation(description: "The pre-mutation status is held")
        fixture.status.requested(1) { first.fulfill() }
        let reading = Task { await fixture.model.projects.refresh() }
        await fulfillment(of: [first], timeout: 2)
        await fixture.source.holdNext(.live)
        let live = expectation(description: "Explicit refresh waits for live health")
        let notice = await fixture.source.notifyWhenRequested(.live, count: 2) { live.fulfill() }
        let refreshing = Task { await fixture.model.refreshAll() }
        await fulfillment(of: [live], timeout: 2)
        let second = expectation(description: "The invalidated status is replaced before live finishes")
        fixture.status.requested(2) { second.fulfill() }
        fixture.status.finishNext()
        await fulfillment(of: [second], timeout: 2)
        XCTAssertNil(fixture.model.status)
        XCTAssertEqual(fixture.status.peakActive, 1)
        fixture.status.finishAll()
        await reading.value
        await fixture.source.finishLive()
        await refreshing.value
        await fixture.source.cancelNotice(notice)
        XCTAssertNotNil(fixture.model.status)
        XCTAssertEqual(fixture.status.count, 2, "Late live health must not duplicate the replacement")
    }

    @MainActor
    private func refreshRows(_ model: MnemonicAppModel, route: AppRoute, invalidate: Bool = false) async {
        if route == .memories { await model.refreshMemories(invalidate: invalidate) }
        else { await model.refreshEntities(invalidate: invalidate) }
    }
}
