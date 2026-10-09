import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicApp

@MainActor
final class AppGraphPaintRevisionTests: XCTestCase {
    func testRemovalRevokesPaintEvenWhenTheSameSnapshotReturnsWithoutAnInterveningRender() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        let controller = fixture.controller
        await controller.refresh()
        let snapshot = controller.snapshot
        var revision = controller.paintRevision
        controller.invalidateSelected()
        XCTAssertNil(controller.snapshot)
        XCTAssertGreaterThan(controller.paintRevision, revision)
        revision = controller.paintRevision
        await controller.refresh()
        XCTAssertEqual(controller.snapshot, snapshot)
        XCTAssertEqual(controller.paintRevision, revision)
        controller.setContext(visible: false)
        XCTAssertGreaterThan(controller.paintRevision, revision)
        revision = controller.paintRevision
        controller.setContext(visible: true)
        XCTAssertGreaterThan(controller.paintRevision, revision)
        revision = controller.paintRevision
        await controller.refresh()
        XCTAssertEqual(controller.snapshot, snapshot)
        XCTAssertEqual(controller.paintRevision, revision)
        await fixture.reads.set(.failure(.unavailable))
        await controller.refresh()
        XCTAssertNil(controller.snapshot)
        XCTAssertGreaterThan(controller.paintRevision, revision)
        revision = controller.paintRevision
        await fixture.reads.set(.success(GraphTestValues.snapshot()))
        await controller.refresh()
        XCTAssertEqual(controller.snapshot, snapshot)
        XCTAssertEqual(controller.paintRevision, revision)
        controller.resetEndpoint()
        XCTAssertNil(controller.snapshot)
        XCTAssertGreaterThan(controller.paintRevision, revision)
    }

    func testFocusFallbackRunningInvalidationAndMutationAdvancePaintRevision() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        let controller = fixture.controller
        await controller.refresh()
        let overviewRevision = controller.paintRevision
        await controller.expand("project:1")
        XCTAssertGreaterThan(controller.paintRevision, overviewRevision)
        let focusedRevision = controller.paintRevision
        await fixture.reads.reset { key in
            key == nil ? .success(GraphTestValues.snapshot()) : .failure(.notFound)
        }
        await controller.refresh()
        XCTAssertNil(controller.expandedKey)
        XCTAssertNotNil(controller.snapshot)
        XCTAssertGreaterThan(controller.paintRevision, focusedRevision)
        await fixture.reads.hold()
        let beforeRead = controller.paintRevision
        let reading = Task { await controller.refresh() }
        await graphArrived(fixture.reads, 5)
        XCTAssertTrue(controller.invalidateRunningRead())
        XCTAssertGreaterThan(controller.paintRevision, beforeRead)
        XCTAssertNil(controller.snapshot)
        await fixture.reads.finishAll()
        await reading.value
        let beforeMutation = controller.paintRevision
        await fixture.mutations.hold()
        let applying = Task {
            await controller.setArchived(.entity(id: "phantom", label: "Phantom"), archived: true)
        }
        await graphArrived(fixture.mutations, 1)
        XCTAssertNil(controller.snapshot)
        XCTAssertGreaterThan(controller.paintRevision, beforeMutation)
        await fixture.mutations.finishAll()
        _ = await applying.value
        XCTAssertNotNil(controller.snapshot)
    }
}
