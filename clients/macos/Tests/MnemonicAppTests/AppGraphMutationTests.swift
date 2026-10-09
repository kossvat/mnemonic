import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicApp

@MainActor
final class AppGraphMutationTests: XCTestCase {
    let target = GraphArchiveTarget.entity(id: "phantom", label: "Phantom")

    func testFailedWriteRetainsRetryAndUndoWithoutAutomaticMutationRetries() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.controller.loadOutside()
        await fixture.controller.loadHistory()
        await fixture.mutations.set(.failure(.local(.undecodable)))
        let saved = await fixture.controller.setArchived(target, archived: true)
        XCTAssertFalse(saved)
        XCTAssertTrue(fixture.controller.canUndo)
        XCTAssertTrue(fixture.controller.canRetryMutation)
        XCTAssertEqual(fixture.controller.recovery?.target, target)
        XCTAssertTrue(fixture.controller.message?.contains("may have been saved") == true)
        let attempts = await fixture.mutations.count, reads = await fixture.reads.count
        let lists = await fixture.archives.arguments()
        XCTAssertEqual(attempts, 1)
        XCTAssertEqual(reads, 2)
        XCTAssertEqual(lists.map(\.offset), [0, 0, 0, 0])
        await fixture.mutations.set(.success(()))
        let retry = await fixture.controller.retryMutation()
        XCTAssertTrue(retry)
        XCTAssertFalse(fixture.controller.canRetryMutation)
        XCTAssertTrue(fixture.controller.canUndo)
        let undo = await fixture.controller.undo()
        XCTAssertTrue(undo)
        let calls = await fixture.mutations.arguments()
        XCTAssertEqual(calls.map(\.target), [target, target, target])
        XCTAssertEqual(calls.map(\.archived), [true, true, false])
    }

    func testConfirmedPostWithFailedFollowupGraphKeepsRetryAndUndo() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.reads.set(.failure(.unavailable))
        let confirmed = await fixture.controller.setArchived(target, archived: true)
        XCTAssertFalse(confirmed)
        XCTAssertTrue(fixture.controller.canRetryMutation)
        XCTAssertTrue(fixture.controller.canUndo)
        XCTAssertTrue(fixture.controller.message?.contains("change was saved") == true)
        let writes = await fixture.mutations.count
        XCTAssertEqual(writes, 1)
        await fixture.reads.set(.success(GraphTestValues.snapshot(2)))
        let retry = await fixture.controller.retryMutation()
        XCTAssertTrue(retry)
        XCTAssertFalse(fixture.controller.canRetryMutation)
        XCTAssertEqual(fixture.controller.snapshot?.nodes.first?.label, "Version 2")
    }

    func testHiddenUncertainResponseRecordsRecoveryWithoutGraphListOrMemoryReads() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.controller.loadOutside()
        await fixture.mutations.hold()
        await fixture.mutations.set(.failure(.local(.unreachable)))
        let applying = Task { await fixture.controller.setArchived(target, archived: true) }
        await graphArrived(fixture.mutations, 1)
        fixture.controller.setContext(visible: false)
        await fixture.mutations.finishAll()
        let confirmed = await applying.value
        XCTAssertFalse(confirmed)
        XCTAssertTrue(fixture.controller.canUndo)
        XCTAssertTrue(fixture.controller.canRetryMutation)
        XCTAssertNil(fixture.controller.snapshot)
        XCTAssertFalse(fixture.controller.mutationInFlight)
        let reads = await fixture.reads.count, lists = await fixture.archives.count
        let memories = await fixture.memories.count
        XCTAssertEqual(reads, 1)
        XCTAssertEqual(lists, 1)
        XCTAssertEqual(memories, 0)
        let hiddenUndo = await fixture.controller.undo()
        XCTAssertFalse(hiddenUndo)
        fixture.controller.setContext(visible: true)
        await fixture.mutations.set(.success(()))
        let undo = await fixture.controller.undo()
        XCTAssertTrue(undo)
        let writes = await fixture.mutations.arguments()
        XCTAssertEqual(writes.map(\.archived), [true, false])
    }

    func testMutationRevokesOlderSnapshotBeforePostAndSuppressesDuplicateSubmissions() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.reads.hold()
        await fixture.reads.set(.success(GraphTestValues.snapshot(2)))
        let reading = Task { await fixture.controller.refresh() }
        await graphArrived(fixture.reads, 2)
        await fixture.mutations.hold()
        let applying = Task { await fixture.controller.setArchived(target, archived: true) }
        await graphArrived(fixture.mutations, 1)
        XCTAssertNil(fixture.controller.snapshot)
        XCTAssertTrue(fixture.controller.mutationInFlight)
        let duplicate = await fixture.controller.setArchived(target, archived: true)
        XCTAssertFalse(duplicate)
        await fixture.reads.finish(2)
        await reading.value
        XCTAssertNil(fixture.controller.snapshot)
        let writes = await fixture.mutations.count
        XCTAssertEqual(writes, 1)
        await fixture.reads.set(.success(GraphTestValues.snapshot(3)))
        await fixture.mutations.finishAll()
        await graphArrived(fixture.reads, 3)
        await fixture.reads.finishAll()
        let confirmed = await applying.value
        XCTAssertTrue(confirmed)
        XCTAssertEqual(fixture.controller.snapshot?.nodes.first?.label, "Version 3")
    }

    func testEndpointResetDiscardsOldRecoveryAndCannotFinishNewMutation() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.mutations.hold()
        let old = Task { await fixture.controller.setArchived(target, archived: true) }
        await graphArrived(fixture.mutations, 1)
        fixture.controller.resetEndpoint()
        let newTarget = GraphArchiveTarget.project(key: "harbor", label: "Harbor")
        let newer = Task { await fixture.controller.setArchived(newTarget, archived: true) }
        await graphArrived(fixture.mutations, 2)
        await fixture.mutations.finish(1)
        let oldConfirmed = await old.value
        XCTAssertFalse(oldConfirmed)
        XCTAssertTrue(fixture.controller.mutationInFlight)
        XCTAssertNil(fixture.controller.recovery)
        XCTAssertNil(fixture.controller.message)
        await fixture.mutations.finishAll()
        let confirmed = await newer.value
        XCTAssertTrue(confirmed)
        XCTAssertEqual(fixture.controller.recovery?.target, newTarget)
        XCTAssertFalse(fixture.controller.mutationInFlight)
    }

    func testEndpointResetDuringFollowupReadDoesNotReportTheOldEndpointAsCurrent() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.reads.hold()
        let applying = Task { await fixture.controller.setArchived(target, archived: true) }
        await graphArrived(fixture.reads, 1)
        XCTAssertTrue(fixture.controller.canUndo)
        fixture.controller.resetEndpoint()
        await fixture.reads.finishAll()
        let confirmed = await applying.value
        XCTAssertFalse(confirmed)
        XCTAssertNil(fixture.controller.recovery)
        XCTAssertNil(fixture.controller.snapshot)
    }

    func testArchiveListResetAndHideDiscardLatePagesWithoutReplacingCurrentResults() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.archives.hold()
        let old = Task { await fixture.controller.loadHistory() }
        await graphArrived(fixture.archives, 1)
        let reset = Task { await fixture.controller.loadHistory(reset: true) }
        await graphArrived(fixture.archives, 2)
        await fixture.archives.finish(1)
        await old.value
        XCTAssertTrue(fixture.controller.history.loading)
        XCTAssertTrue(fixture.controller.history.entities.isEmpty)
        await fixture.archives.finish(2)
        await reset.value
        XCTAssertEqual(fixture.controller.history.entities.count, 20)
        let next = Task { await fixture.controller.loadHistory() }
        await graphArrived(fixture.archives, 3)
        fixture.controller.setContext(visible: false)
        await fixture.archives.finishAll()
        await next.value
        XCTAssertEqual(fixture.controller.history.entities.count, 20)
        XCTAssertFalse(fixture.controller.history.loading)
    }
    func testReopeningWhilePostIsHeldRefreshesCurrentFocusAfterSuccessOrFailure() async {
        for postFailed in [false, true] {
            let fixture = GraphControllerFixture()
            defer { fixture.close() }
            await fixture.controller.refresh()
            await fixture.controller.expand("project:1")
            await fixture.controller.loadOutside()
            await fixture.controller.loadHistory()
            await fixture.mutations.set(postFailed ? .failure(.local(.unreachable)) : .success(()))
            await fixture.mutations.hold()
            let applying = Task { await fixture.controller.setArchived(target, archived: true) }
            await graphArrived(fixture.mutations, 1)
            fixture.controller.setContext(visible: false)
            fixture.controller.setContext(visible: true)
            await fixture.controller.refresh()
            XCTAssertNil(fixture.controller.snapshot)
            XCTAssertTrue(fixture.controller.mutationInFlight)
            let beforeReply = await fixture.reads.count
            XCTAssertEqual(beforeReply, 2, "Reopening cannot read before the pending write replies")
            await fixture.reads.set(.success(GraphTestValues.snapshot(2)))
            await fixture.mutations.finishAll()
            let confirmed = await applying.value
            XCTAssertEqual(confirmed, !postFailed)
            XCTAssertEqual(fixture.controller.snapshot, GraphTestValues.snapshot(2))
            XCTAssertEqual(fixture.controller.expandedKey, "project:1")
            XCTAssertFalse(fixture.controller.loading)
            XCTAssertFalse(fixture.controller.mutationInFlight)
            XCTAssertTrue(fixture.controller.canUndo)
            XCTAssertEqual(fixture.controller.canRetryMutation, postFailed)
            let reads = await fixture.reads.arguments(), lists = await fixture.archives.arguments()
            let writes = await fixture.mutations.count
            XCTAssertEqual(reads, [nil, "project:1", "project:1"])
            XCTAssertEqual(lists.map(\.offset), [0, 0, 0, 0])
            XCTAssertEqual(writes, 1)
        }
    }

    func testReopenedPostFollowupStopsWhenThatVisitHidesAgain() async {
        let fixture = GraphControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.controller.loadOutside()
        await fixture.controller.loadHistory()
        await fixture.mutations.hold()
        let applying = Task { await fixture.controller.setArchived(target, archived: true) }
        await graphArrived(fixture.mutations, 1)
        fixture.controller.setContext(visible: false)
        fixture.controller.setContext(visible: true)
        await fixture.reads.hold()
        await fixture.mutations.finishAll()
        await graphArrived(fixture.reads, 2)
        fixture.controller.setContext(visible: false)
        await fixture.reads.finishAll()
        _ = await applying.value
        XCTAssertNil(fixture.controller.snapshot)
        XCTAssertTrue(fixture.controller.canUndo)
        let lists = await fixture.archives.count, writes = await fixture.mutations.count
        XCTAssertEqual(lists, 2, "A newly hidden visit cannot continue archive-list reads")
        XCTAssertEqual(writes, 1)
    }

}
