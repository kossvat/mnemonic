import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

@MainActor
final class AppTimelineMutationTests: XCTestCase {
    private func arrived<Argument: Sendable, Value: Sendable>(
        _ source: TimelineTestReplies<Argument, Value>, _ count: Int
    ) async {
        let expected = expectation(description: "Mutation fixture request \(count) arrived")
        await source.requested(count) { expected.fulfill() }
        await fulfillment(of: [expected], timeout: 5)
    }

    func testExplicitApplyAndUndoSendOnceAndInvalidateHistoricalCache() async {
        let day = "2026-09-29", fixture = TimelineControllerFixture(day: "2026-09-29")
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.adds.hold()
        let correction = TimelineTestValues.correction(day)
        let applying = Task { await fixture.controller.apply(correction, for: day) }
        await arrived(fixture.adds, 1)
        XCTAssertTrue(fixture.controller.mutationInFlight)
        let duplicate = await fixture.controller.apply(correction, for: day)
        XCTAssertFalse(duplicate)
        let submissions = await fixture.adds.count
        XCTAssertEqual(submissions, 1)
        await fixture.adds.finishAll()
        let applied = await applying.value
        XCTAssertTrue(applied)
        XCTAssertTrue(fixture.controller.canUndo)
        XCTAssertFalse(fixture.controller.mutationInFlight)
        let afterApply = await fixture.reads.count
        XCTAssertEqual(afterApply, 2)

        await fixture.removes.hold()
        let undoing = Task { await fixture.controller.undo() }
        await arrived(fixture.removes, 1)
        let repeatedUndo = await fixture.controller.undo()
        XCTAssertFalse(repeatedUndo)
        await fixture.removes.finishAll()
        let undone = await undoing.value
        XCTAssertTrue(undone)
        XCTAssertFalse(fixture.controller.canUndo)
        XCTAssertFalse(fixture.controller.mutationInFlight)
        let removed = await fixture.removes.arguments(), afterUndo = await fixture.reads.count
        XCTAssertEqual(removed, [7])
        XCTAssertEqual(afterUndo, 3)
    }

    func testHiddenStaleDayAndInvalidCorrectionsCannotSubmit() async {
        let fixture = TimelineControllerFixture()
        defer { fixture.close() }
        let correction = TimelineTestValues.correction("2026-09-30")
        fixture.controller.setContext(day: "2026-09-29", visible: true)
        let stale = await fixture.controller.apply(correction, for: "2026-09-30")
        fixture.controller.setContext(day: "2026-09-30", visible: false)
        let hidden = await fixture.controller.apply(correction, for: "2026-09-30")
        fixture.controller.setContext(day: "2026-09-30", visible: true)
        let invalid = await fixture.controller.apply(AttributionCorrection(project: nil), for: "2026-09-30")
        XCTAssertFalse(stale)
        XCTAssertFalse(hidden)
        XCTAssertFalse(invalid)
        let requests = await fixture.adds.count
        XCTAssertEqual(requests, 0)
        XCTAssertFalse(fixture.controller.canUndo)
    }

    func testConfirmedCorrectionWhileHiddenKeepsUndoWithoutReadingOrPublishing() async {
        let day = "2026-09-29", fixture = TimelineControllerFixture(day: "2026-09-29")
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.adds.hold()
        let applying = Task { await fixture.controller.apply(TimelineTestValues.correction(day), for: day) }
        await arrived(fixture.adds, 1)
        fixture.controller.setContext(day: day, visible: false)
        await fixture.adds.finishAll()
        let success = await applying.value
        XCTAssertTrue(success)
        XCTAssertTrue(fixture.controller.canUndo)
        XCTAssertNil(fixture.controller.message)
        XCTAssertNil(fixture.controller.snapshot)
        let hiddenReads = await fixture.reads.count
        XCTAssertEqual(hiddenReads, 1)
        fixture.controller.setContext(day: day, visible: true)
        XCTAssertNil(fixture.controller.snapshot, "The corrected historical cache must be gone")
        await fixture.controller.refresh()
        let reopenedReads = await fixture.reads.count
        XCTAssertEqual(reopenedReads, 2)
        let undone = await fixture.controller.undo()
        XCTAssertTrue(undone)
        let removed = await fixture.removes.arguments()
        XCTAssertEqual(removed, [7])
    }

    func testLateRangeCorrectionDoesNotReplaceAnotherDaysPresentation() async {
        let day = "2026-09-29", fixture = TimelineControllerFixture(day: "2026-09-29")
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.adds.hold()
        let applying = Task { await fixture.controller.apply(TimelineTestValues.correction(day), for: day) }
        await arrived(fixture.adds, 1)
        fixture.controller.setContext(day: "2026-09-28", visible: true)
        await fixture.controller.refresh()
        await fixture.adds.finishAll()
        let success = await applying.value
        XCTAssertTrue(success)
        XCTAssertEqual(fixture.controller.snapshot?.day, "2026-09-28")
        XCTAssertNil(fixture.controller.message)
        XCTAssertTrue(fixture.controller.canUndo)
        let count = await fixture.reads.count
        XCTAssertEqual(count, 2, "The unrelated selected day must not be refetched")
        fixture.controller.setContext(day: day, visible: true)
        XCTAssertNil(fixture.controller.snapshot)
        await fixture.controller.refresh()
        let refreshed = await fixture.reads.count
        XCTAssertEqual(refreshed, 3)
    }

    func testWholeTranscriptPinAndUndoInvalidateEveryCachedDay() async {
        let fixture = TimelineControllerFixture(day: "2026-09-29")
        defer { fixture.close() }
        await fixture.controller.refresh()
        fixture.controller.setContext(day: "2026-09-28", visible: true)
        await fixture.controller.refresh()
        fixture.controller.setContext(day: "2026-09-29", visible: true)
        let applied = await fixture.controller.apply(
            AttributionCorrection(stream: "fixture-stream", project: nil), for: "2026-09-29")
        XCTAssertTrue(applied)
        fixture.controller.setContext(day: "2026-09-28", visible: true)
        XCTAssertNil(fixture.controller.snapshot)
        await fixture.controller.refresh()
        let beforeUndo = await fixture.reads.count
        XCTAssertEqual(beforeUndo, 4)
        let undone = await fixture.controller.undo()
        XCTAssertTrue(undone)
        fixture.controller.setContext(day: "2026-09-29", visible: true)
        XCTAssertNil(fixture.controller.snapshot)
        await fixture.controller.refresh()
        let afterUndo = await fixture.reads.count
        XCTAssertEqual(afterUndo, 6)
    }

    func testFailuresRetainAnEarlierUndoAndNeverRetryAutomatically() async {
        let fixture = TimelineControllerFixture()
        defer { fixture.close() }
        let correction = TimelineTestValues.correction("2026-09-30")
        let first = await fixture.controller.apply(correction, for: "2026-09-30")
        XCTAssertTrue(first)
        await fixture.adds.set(.failure(.unreachable))
        let failed = await fixture.controller.apply(correction, for: "2026-09-30")
        XCTAssertFalse(failed)
        XCTAssertTrue(fixture.controller.canUndo)
        let attempts = await fixture.adds.count
        XCTAssertEqual(attempts, 2)
        await fixture.removes.set(.failure(.earlierDaemon))
        let undoFailed = await fixture.controller.undo()
        XCTAssertFalse(undoFailed)
        XCTAssertTrue(fixture.controller.canUndo)
        XCTAssertFalse(fixture.controller.message?.contains("earlier") ?? true)
        let removes = await fixture.removes.arguments()
        XCTAssertEqual(removes, [7])
        await fixture.removes.set(.success(()))
        let recovered = await fixture.controller.undo()
        XCTAssertTrue(recovered)
        XCTAssertFalse(fixture.controller.canUndo)
    }

    func testEndpointResetDiscardsOldMutationWithoutFinishingANewSubmit() async {
        let fixture = TimelineControllerFixture()
        defer { fixture.close() }
        let correction = TimelineTestValues.correction("2026-09-30")
        await fixture.adds.hold()
        let old = Task { await fixture.controller.apply(correction, for: "2026-09-30") }
        await arrived(fixture.adds, 1)
        fixture.controller.resetEndpoint()
        XCTAssertFalse(fixture.controller.mutationInFlight)
        await fixture.adds.set(.success(8))
        let newer = Task { await fixture.controller.apply(correction, for: "2026-09-30") }
        await arrived(fixture.adds, 2)
        await fixture.adds.finish(1)
        let oldSuccess = await old.value
        XCTAssertFalse(oldSuccess)
        XCTAssertTrue(fixture.controller.mutationInFlight)
        XCTAssertFalse(fixture.controller.canUndo)
        XCTAssertNil(fixture.controller.message)
        await fixture.adds.finishAll()
        let newSuccess = await newer.value
        XCTAssertTrue(newSuccess)
        XCTAssertTrue(fixture.controller.canUndo)
        let undone = await fixture.controller.undo()
        XCTAssertTrue(undone)
        let removed = await fixture.removes.arguments()
        XCTAssertEqual(removed, [8], "Undo must never target the prior endpoint's record")
    }

    func testEndpointResetDuringPostSaveReadDiscardsLateCompletion() async {
        let fixture = TimelineControllerFixture()
        defer { fixture.close() }
        await fixture.reads.hold()
        let applying = Task {
            await fixture.controller.apply(TimelineTestValues.correction("2026-09-30"), for: "2026-09-30")
        }
        await arrived(fixture.reads, 1)
        XCTAssertTrue(fixture.controller.canUndo)
        fixture.controller.resetEndpoint()
        await fixture.reads.finishAll()
        let success = await applying.value
        XCTAssertFalse(success)
        XCTAssertFalse(fixture.controller.canUndo)
        XCTAssertNil(fixture.controller.snapshot)
        XCTAssertNil(fixture.controller.message)
    }

    func testCorrectionInvalidatesAnAlreadyOwnedTimelineReadSerially() async {
        let fixture = TimelineControllerFixture()
        defer { fixture.close() }
        await fixture.controller.refresh()
        await fixture.reads.set(.success(TimelineTestValues.snapshot("2026-09-30", version: 2)))
        await fixture.reads.hold()
        let reading = Task { await fixture.controller.refresh() }
        await arrived(fixture.reads, 2)
        let applied = await fixture.controller.apply(
            TimelineTestValues.correction("2026-09-30"), for: "2026-09-30")
        XCTAssertTrue(applied)
        await fixture.reads.set(.success(TimelineTestValues.snapshot("2026-09-30", version: 3)))
        await fixture.reads.finish(2)
        await arrived(fixture.reads, 3)
        XCTAssertEqual(fixture.controller.snapshot?.projects.first?.name, "Version 1")
        await fixture.reads.finishAll()
        await reading.value
        let count = await fixture.reads.count, peak = await fixture.reads.peakActive
        XCTAssertEqual(count, 3)
        XCTAssertEqual(peak, 1)
        XCTAssertEqual(fixture.controller.snapshot?.projects.first?.name, "Version 3")
    }
}
