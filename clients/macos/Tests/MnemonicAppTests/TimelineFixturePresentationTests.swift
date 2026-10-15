import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class TimelineFixturePresentationTests: XCTestCase {
    func testTimelineStatesHaveNoDeferredExpectationAndEveryStateHasALiteral() throws {
        let states = try FixtureStates.timelines()
        XCTAssertTrue(states.allSatisfy { state in state.until == nil })
        XCTAssertEqual(Set(states.map(\.id)), Set(FixtureTimelineExpectations.byID.keys))
    }

    func testFixtureViewsExerciseTheNamedScaleEdgeAndCrowdedRows() throws {
        for state in try FixtureStates.timelines() {
            guard let snapshot = state.timeline, let focus = FixtureTimelinePresentation.focus(state) else { continue }
            let scale = FixtureTimelinePresentation.scale(state)
            let drawing = TimelineDrawing(snapshot, span: scale == .focus ? focus : nil,
                width: FixtureTimelinePresentation.width(state), registry: state.board, scale: scale, calendar: state.calendar)
            switch state.id {
            case "timeline-focus":
                XCTAssertEqual(drawing.duration, 7_200)
                XCTAssertEqual(drawing.lanes.first?.tracks, 2)
                XCTAssertEqual(drawing.lanes.first?.blocks.first?.geometry.paint.width, 290)
            case "timeline-short":
                XCTAssertEqual(drawing.duration, 86_400)
                XCTAssertEqual(drawing.width, 1_440)
                XCTAssertEqual(drawing.segment(at: 545.5, in: drawing.lanes[0].blocks)?.id, "segment-2")
            case "timeline-edge":
                XCTAssertTrue(try XCTUnwrap(drawing.lanes.first?.blocks.first).geometry.continuesBefore)
            case "timeline-crowded":
                let lane = try XCTUnwrap(drawing.lanes.first { lane in lane.project == FixtureWire.key(0) })
                XCTAssertEqual(lane.blocks.count, 36)
                XCTAssertEqual(lane.tracks, 12)
                XCTAssertTrue(lane.blocks.allSatisfy { block in block.caption == nil })
                let folded = try XCTUnwrap(drawing.presentedLanes().first { value in value.id == lane.id })
                XCTAssertEqual(folded.rows.count, 3)
                XCTAssertEqual(folded.hiddenCount, 27)
                XCTAssertEqual(folded.toggleLabel, "+27 more segments")
            case "timeline-registry":
                XCTAssertEqual(drawing.lanes.map(\.name), ["Beacon", "Harbor"])
                XCTAssertEqual(drawing.lanes.last?.emptyMessage, "No sessions that day")
            case "timeline-day", "timeline-23h", "timeline-25h":
                XCTAssertEqual(drawing.from, snapshot.from)
                XCTAssertEqual(drawing.to, snapshot.to)
            default: break
            }
        }
    }

    func testFailuresAndLoadingDoNotSummarizeHeldNumbersAsCurrent() throws {
        let snapshot = try FixtureWire.timeline(segments: [.init(from: 540, to: 600)])
        for var state in FixtureStates.failures(.timeline) {
            state.timeline = snapshot
            let summary = FixtureStateSummary.summary(of: state)
            XCTAssertEqual(summary, state.expected)
            XCTAssertFalse(summary.contains("Agents 1h"))
        }
        let state = FixtureState(id: "loading", screen: .timeline, loading: true, expected: "")
        XCTAssertEqual(FixtureStateSummary.summary(of: state), "loading=Loading timeline span=not-shown totals=not-shown")
    }

    func testCrowdedQuietProjectLabelsAreAlsoCheckedWhenTheStripMoves() throws {
        let state = try XCTUnwrap(FixtureStates.timelines().first { state in state.id == "timeline-crowded" })
        let snapshot = try XCTUnwrap(state.timeline)
        let span = TimelineDrawing.Span(from: snapshot.from + 650 * 60, to: snapshot.from + 770 * 60)
        let drawing = TimelineDrawing(snapshot, span: span, width: 1_440, calendar: state.calendar)
        let labels = drawing.lanes.flatMap(\.blocks).compactMap(\.caption)
        XCTAssertFalse(labels.isEmpty)
        for lane in drawing.lanes {
            for block in lane.blocks {
                guard let caption = block.caption else { continue }
                XCTAssertGreaterThanOrEqual(caption.rect.x, block.geometry.paint.x)
                XCTAssertLessThanOrEqual(caption.rect.end, block.geometry.paint.end)
                for neighbour in lane.blocks where neighbour.id != block.id && neighbour.track == block.track {
                    if let other = neighbour.caption { XCTAssertFalse(caption.rect.overlaps(other.rect)) }
                }
            }
        }
    }
}
