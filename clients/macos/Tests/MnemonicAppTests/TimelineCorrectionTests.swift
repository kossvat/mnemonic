import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class TimelineCorrectionTests: XCTestCase {
    private let day = "2026-06-17"
    private let base = 1_781_654_400.0

    private func snapshot(segments: [TimelineSegment] = [], owner: [TimelineOwnerRange] = []) -> ProjectTimeline {
        ProjectTimeline(day: day, from: base, to: base + 86_400,
                        projects: [TimelineProject(key: "demoapp", name: "Demo app")],
                        segments: segments, owner: owner, ownerAvailable: true)
    }
    private func segment(from: Double, to: Double) -> TimelineSegment {
        TimelineSegment(id: "segment", stream: "fixture-stream", project: "demoapp", agent: "codex",
                        driver: "owner", from: from, to: to)
    }

    func testWholeSecondRangeKeepsExactBoundsAndExplicitUnassignedProject() throws {
        let item = segment(from: base + 10, to: base + 70)
        let value = snapshot(segments: [item])
        let selection = TimelineSelection.agent(day: day, item)
        let correction = try XCTUnwrap(selection.correction(project: nil, transcript: false, day: day, snapshot: value))
        XCTAssertEqual(correction.stream, "fixture-stream")
        XCTAssertEqual(correction.from?.timeIntervalSince1970, item.from)
        XCTAssertEqual(correction.to?.timeIntervalSince1970, item.to)
        XCTAssertNil(correction.project)
        XCTAssertTrue(correction.isValid)
    }

    func testPointCanOnlyProduceAnExplicitWholeTranscriptPin() throws {
        let point = segment(from: base + 10, to: base + 10)
        let value = snapshot(segments: [point])
        let selection = TimelineSelection.agent(day: day, point)
        XCTAssertTrue(selection.point)
        XCTAssertFalse(selection.exactRange)
        XCTAssertNil(selection.correction(project: "demoapp", transcript: false, day: day, snapshot: value))
        let pin = try XCTUnwrap(selection.correction(project: "demoapp", transcript: true, day: day, snapshot: value))
        XCTAssertEqual(pin.stream, point.stream)
        XCTAssertNil(pin.from)
        XCTAssertNil(pin.to)
    }

    func testFractionalOwnerRangesAreViewOnlyWithoutRoundingOrExpansion() {
        let range = TimelineOwnerRange(id: "owner", from: base + 0.25, to: base + 60.75,
            assignments: [TimelineAssignment(project: "demoapp", share: 0.5, confidence: "low"),
                          TimelineAssignment(project: nil, share: 0.5, confidence: nil)])
        let value = snapshot(owner: [range])
        let selection = TimelineSelection.owner(day: day, range)
        XCTAssertFalse(selection.exactRange)
        XCTAssertNil(selection.correction(project: "demoapp", transcript: false, day: day, snapshot: value))
        XCTAssertNil(selection.correction(project: "demoapp", transcript: true, day: day, snapshot: value))
        XCTAssertEqual(selection.from, base + 0.25)
        XCTAssertEqual(selection.to, base + 60.75)
    }

    func testOwnerCorrectionUsesExactIntervalWithoutInventingAStream() throws {
        let range = TimelineOwnerRange(id: "owner", from: base + 10, to: base + 70,
                                       assignments: [TimelineAssignment(project: nil, share: 1, confidence: nil)])
        let selection = TimelineSelection.owner(day: day, range)
        let value = try XCTUnwrap(selection.correction(project: "demoapp", transcript: false,
                                                       day: day, snapshot: snapshot(owner: [range])))
        XCTAssertNil(value.stream)
        XCTAssertEqual(value.from?.timeIntervalSince1970, range.from)
        XCTAssertEqual(value.to?.timeIntervalSince1970, range.to)
    }

    func testOldDayRemovedSelectionAndChangedSameIdentityCannotApply() {
        let item = segment(from: base + 10, to: base + 70)
        let value = snapshot(segments: [item])
        let selection = TimelineSelection.agent(day: day, item)
        XCTAssertTrue(selection.current(day: day, snapshot: value))
        XCTAssertNil(selection.correction(project: "demoapp", transcript: false, day: "2026-06-18", snapshot: value))
        XCTAssertNil(selection.correction(project: "demoapp", transcript: false, day: day, snapshot: nil))
        XCTAssertNil(selection.correction(project: "demoapp", transcript: false, day: day, snapshot: snapshot()))
        let changed = snapshot(segments: [segment(from: item.from, to: item.to + 1)])
        XCTAssertNil(selection.correction(project: "demoapp", transcript: false, day: day, snapshot: changed))
        XCTAssertNil(selection.correction(project: "demoapp", transcript: true, day: day, snapshot: changed))
    }
}
