import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicApp

@MainActor
final class AppGraphReadTests: XCTestCase {
    func testContextAndHiddenRefreshDoNotReadAndTheControllerOwnsNoClock() async {
        let fixture = GraphControllerFixture(visible: false)
        defer { fixture.close() }
        await fixture.controller.refresh()
        fixture.controller.setContext(visible: true)
        fixture.controller.setContext(visible: true)
        let before = await fixture.reads.count
        XCTAssertEqual(before, 0)
        await fixture.controller.refresh()
        XCTAssertEqual(fixture.controller.snapshot, GraphTestValues.snapshot())
        fixture.controller.setContext(visible: false)
        await fixture.controller.refresh()
        let count = await fixture.reads.count
        XCTAssertEqual(count, 1)
        XCTAssertNil(fixture.controller.snapshot)
    }

    func testInvalidationSuppressesOldRepliesAndUsesSerialReplacementReads() async {
        for failure in [false, true] {
            let fixture = GraphControllerFixture()
            defer { fixture.close() }
            var presented: [String] = []
            fixture.controller.onPresentationChange = {
                if let label = fixture.controller.snapshot?.nodes.first?.label { presented.append(label) }
            }
            await fixture.reads.hold()
            await fixture.reads.set(failure ? .failure(.unavailable) : .success(GraphTestValues.snapshot(2)))
            let reading = Task { await fixture.controller.refresh() }
            await graphArrived(fixture.reads, 1)
            XCTAssertTrue(fixture.controller.invalidateRunningRead())
            await fixture.reads.set(.success(GraphTestValues.snapshot(3)))
            await fixture.reads.finish(1)
            await graphArrived(fixture.reads, 2)
            XCTAssertNil(fixture.controller.snapshot)
            XCTAssertNil(fixture.controller.failure)
            XCTAssertTrue(presented.isEmpty)
            XCTAssertTrue(fixture.controller.invalidateRunningRead())
            await fixture.reads.set(.success(GraphTestValues.snapshot(4)))
            await fixture.reads.finish(2)
            await graphArrived(fixture.reads, 3)
            await fixture.controller.refresh()
            await fixture.reads.finishAll()
            await reading.value
            let count = await fixture.reads.count, peak = await fixture.reads.peakActive
            XCTAssertEqual(count, 3)
            XCTAssertEqual(peak, 1)
            XCTAssertTrue(presented.allSatisfy { value in value == "Version 4" })
            XCTAssertEqual(fixture.controller.snapshot?.nodes.first?.label, "Version 4")
            XCTAssertFalse(fixture.controller.loading)
        }
    }

    func testHiddenEndpointAndFocusRepliesCannotPublishOrClearANewerLoader() async {
        for change in ["hide", "endpoint", "focus"] {
            let fixture = GraphControllerFixture()
            defer { fixture.close() }
            await fixture.controller.refresh()
            await fixture.reads.hold()
            await fixture.reads.set(.failure(.unavailable))
            let old = Task { await fixture.controller.refresh() }
            await graphArrived(fixture.reads, 2)
            await fixture.reads.set(.success(GraphTestValues.snapshot(2)))
            let newer: Task<Void, Never>
            if change == "focus" { newer = Task { await fixture.controller.expand("project:1") } }
            else {
                if change == "hide" { fixture.controller.setContext(visible: false) }
                else { fixture.controller.resetEndpoint() }
                newer = Task { await fixture.controller.refresh() }
            }
            if change != "hide" { await graphArrived(fixture.reads, 3) }
            await fixture.reads.finish(2)
            await old.value
            XCTAssertNil(fixture.controller.failure)
            XCTAssertNil(fixture.controller.snapshot)
            XCTAssertEqual(fixture.controller.loading, change != "hide")
            await fixture.reads.finishAll()
            await newer.value
            XCTAssertEqual(fixture.controller.snapshot?.nodes.first?.label, change == "hide" ? nil : "Version 2")
        }
    }

    func testMissingExpansionFallsBackOnceButUnavailableDoesNotClaimMissingConfiguration() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.reads.reset { key in key == nil ? .success(GraphTestValues.snapshot(2)) : .failure(.notFound) }
        await fixture.controller.expand("project:1")
        XCTAssertNil(fixture.controller.expandedKey)
        XCTAssertEqual(fixture.controller.snapshot?.nodes.first?.label, "Version 2")
        let arguments = await fixture.reads.arguments()
        XCTAssertEqual(arguments, [nil, "project:1", nil])
        await fixture.reads.set(.failure(.unavailable))
        await fixture.controller.expand("project:1")
        XCTAssertEqual(fixture.controller.expandedKey, "project:1")
        XCTAssertEqual(fixture.controller.failure, .unavailable)
        let count = await fixture.reads.count
        XCTAssertEqual(count, 4)
    }

    func testSelectionReadsStaleShelfProjectsAndDropsLateProjectAndEvidenceReplies() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.projects.hold()
        let selecting = Task { await fixture.controller.selectNode("project:2") }
        await graphArrived(fixture.projects, 1)
        await fixture.controller.selectNode("entity:rust")
        await fixture.projects.finishAll()
        await selecting.value
        let keys = await fixture.projects.arguments()
        XCTAssertEqual(keys, ["ledger"])
        XCTAssertNil(fixture.controller.project)
        XCTAssertFalse(fixture.controller.projectLoading)
        await fixture.edges.hold()
        let edge = Task { await fixture.controller.selectEdge(0) }
        await graphArrived(fixture.edges, 1)
        await fixture.controller.selectNode("entity:phantom")
        await fixture.edges.finishAll()
        await edge.value
        let memoryReads = await fixture.memories.count
        XCTAssertEqual(memoryReads, 0)
        XCTAssertTrue(fixture.controller.evidence.isEmpty)
        await fixture.controller.selectNode("unknown")
        XCTAssertNil(fixture.controller.selectedKey)
    }

    func testMemoryReadStopsAtFirstHiddenResponseAndKeepsEveryFollowupBounded() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.memories.hold()
        let loading = Task { await fixture.controller.selectEdge(0) }
        await graphArrived(fixture.memories, 1)
        fixture.controller.setContext(visible: false)
        await fixture.memories.finishAll()
        await loading.value
        let hiddenReads = await fixture.memories.count
        XCTAssertEqual(hiddenReads, 1)
        XCTAssertTrue(fixture.controller.evidence.isEmpty)
        fixture.controller.setContext(visible: true)
        await fixture.controller.refresh()
        await fixture.controller.selectEdge(0)
        for _ in 0..<3 { await fixture.controller.loadMoreEvidence() }
        XCTAssertEqual(fixture.controller.evidence.count, 30)
        XCTAssertEqual(fixture.controller.evidenceDiscardedCount, 10)
        XCTAssertEqual(fixture.controller.evidence.first?.id, "memory-10")
        XCTAssertEqual(fixture.controller.evidence.last?.memory?.content, "Fixture content")
        let pages = await fixture.edges.arguments()
        XCTAssertEqual(pages.map(\.offset), [0, 0, 10, 20, 30])
        XCTAssertTrue(pages.allSatisfy { page in page.limit == 10 })
    }

    func testSignalRefreshRevokesHeldSelectionAndPreviouslyShownEvidence() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.projects.hold()
        let selection = Task { await fixture.controller.selectNode("project:1") }
        await graphArrived(fixture.projects, 1)
        await fixture.controller.refresh()
        await fixture.projects.finishAll()
        await selection.value
        XCTAssertNil(fixture.controller.selectedKey)
        XCTAssertNil(fixture.controller.project)
        XCTAssertFalse(fixture.controller.projectLoading)
        await fixture.controller.selectEdge(0)
        XCTAssertFalse(fixture.controller.evidence.isEmpty)
        await fixture.controller.refresh()
        XCTAssertNil(fixture.controller.selectedEdge)
        XCTAssertTrue(fixture.controller.evidence.isEmpty)
    }

    func testArchivePagingStartsAtZeroRetainsOneHundredAndCanExplicitlyReset() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        for _ in 0..<6 { await fixture.controller.loadOutside() }
        XCTAssertEqual(fixture.controller.outside.entities.count, 100)
        XCTAssertEqual(fixture.controller.outside.discardedCount, 20)
        XCTAssertEqual(fixture.controller.outside.entities.first?.id, "outside-20")
        await fixture.controller.selectNode("entity:outside-20")
        XCTAssertEqual(fixture.controller.selectedKey, "entity:outside-20")
        let projectCalls = await fixture.projects.count
        XCTAssertEqual(projectCalls, 0)
        await fixture.controller.loadOutside(reset: true)
        XCTAssertEqual(fixture.controller.outside.entities.count, 20)
        XCTAssertEqual(fixture.controller.outside.discardedCount, 0)
        let calls = await fixture.archives.arguments()
        XCTAssertEqual(calls.map(\.offset), [0, 20, 40, 60, 80, 100, 0])
        XCTAssertTrue(calls.allSatisfy { page in page.limit == 20 && !page.archived })
    }
}
