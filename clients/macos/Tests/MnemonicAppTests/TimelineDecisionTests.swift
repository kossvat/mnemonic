import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class TimelineDecisionTests: XCTestCase {
    private let calendar = FixtureWire.calendar()

    func testPastDayFocusUsesTheLatestAgentOrOwnerEndOrTheWholeDay() throws {
        let cases: [([FixtureWire.Segment], [FixtureWire.Owner], Double?)] = [
            ([], [], nil), ([], [.init(from: 540, to: 600)], 600),
            ([.init(from: 540, to: 610)], [], 610),
            ([.init(from: 540, to: 610)], [.init(from: 550, to: 620)], 620),
            ([.init(from: 540, to: 630)], [.init(from: 550, to: 620)], 630)
        ]
        for (segments, owner, end) in cases {
            let snapshot = try FixtureWire.timeline(segments: segments, owner: owner)
            let focus = TimelineDrawing.defaultFocus(snapshot, now: FixtureWire.now, calendar: calendar)
            let expected = end.map { minute in
                TimelineDrawing.Span(from: snapshot.from + (minute - 120) * 60, to: snapshot.from + minute * 60)
            } ?? TimelineDrawing.Span(from: snapshot.from, to: snapshot.to)
            XCTAssertEqual(focus, expected)
            XCTAssertEqual(TimelineDrawing.displayScale(.focus, focus: focus, snapshot: snapshot), end == nil ? .day : .focus)
        }
    }

    func testFocusIgnoresUnavailableInvalidAndOffDayOwnerRanges() throws {
        for owner in [[FixtureWire.Owner(from: 540, to: 600)], []] {
            let snapshot = try FixtureWire.timeline(owner: owner, available: false)
            XCTAssertEqual(TimelineDrawing.defaultFocus(snapshot, now: FixtureWire.now, calendar: calendar),
                           .init(from: snapshot.from, to: snapshot.to))
        }
        let invalid = try FixtureWire.timeline(owner: [.init(from: -30, to: -10), .init(from: 700, to: 650)])
        XCTAssertEqual(TimelineDrawing.defaultFocus(invalid, now: FixtureWire.now, calendar: calendar),
                       .init(from: invalid.from, to: invalid.to))
        let today = try FixtureWire.timeline(owner: [.init(from: 540, to: 600)])
        let now = Date(timeIntervalSince1970: today.from + 720 * 60)
        XCTAssertEqual(TimelineDrawing.defaultFocus(today, now: now, calendar: calendar),
                       .init(from: today.from + 600 * 60, to: now.timeIntervalSince1970))
    }

    func testEmptyLanesRequireBothTheDayAnswerAndAnActiveUnmergedRegistryRow() throws {
        let snapshot = try FixtureWire.timeline(projectCount: 5, segments: [.init(project: 3, from: 540, to: 600)])
        let registry = try FixtureWire.board([
            FixtureWire.row(0), FixtureWire.row(1, state: "archived"), FixtureWire.row(2, state: "ignored"),
            FixtureWire.row(3, state: "archived"), FixtureWire.row(4, mergedInto: FixtureWire.key(0)), FixtureWire.row(5)
        ])
        let drawing = TimelineDrawing(snapshot, registry: registry, calendar: calendar)
        XCTAssertEqual(Set(drawing.lanes.compactMap(\.project)), Set([FixtureWire.key(0), FixtureWire.key(3)]))
        let empty = try XCTUnwrap(drawing.lanes.first { lane in lane.project == FixtureWire.key(0) })
        XCTAssertEqual(empty.emptyMessage, "No sessions that day")
        XCTAssertEqual(empty.count, 0)
        XCTAssertTrue(empty.blocks.isEmpty)
        XCTAssertEqual(TimelineDrawing(snapshot, calendar: calendar).lanes.compactMap(\.project), [FixtureWire.key(3)])
        XCTAssertEqual(TimelineDrawing(snapshot, registry: try FixtureWire.board(), calendar: calendar)
            .lanes.compactMap(\.project), [FixtureWire.key(3)])
        let focus = TimelineDrawing.Span(from: snapshot.from + 720 * 60, to: snapshot.from + 840 * 60)
        let focused = TimelineDrawing(snapshot, span: focus, registry: registry, scale: .focus, calendar: calendar)
        XCTAssertEqual(focused.lanes.first { lane in lane.project == FixtureWire.key(3) }?.emptyMessage,
                       "No sessions in this focus")
        XCTAssertEqual(focused.lanes.first { lane in lane.project == FixtureWire.key(0) }?.emptyMessage,
                       "No sessions that day")
    }

    func testThreePackedRowsFoldOnlyHiddenSegmentsInFocusAndKeepTheirExactBounds() throws {
        let state = try XCTUnwrap(FixtureStates.timelines().first { state in state.id == "timeline-crowded" })
        let snapshot = try XCTUnwrap(state.timeline)
        let drawing = TimelineDrawing(snapshot, span: FixtureTimelinePresentation.focus(state), scale: .focus, calendar: calendar)
        let folded = try XCTUnwrap(drawing.presentedLanes().first { value in value.lane.project == FixtureWire.key(0) })
        XCTAssertEqual(folded.rows.count, 3)
        XCTAssertEqual(folded.rows.flatMap { row in row }.count, 9)
        XCTAssertEqual(folded.hiddenCount, 27)
        XCTAssertEqual(folded.toggleLabel, "+27 more segments")
        let expanded = try XCTUnwrap(drawing.presentedLanes(expanded: [folded.id]).first { value in value.id == folded.id })
        XCTAssertEqual(expanded.rows.count, 12)
        XCTAssertEqual(expanded.hiddenCount, 0)
        XCTAssertEqual(expanded.toggleLabel, "Show fewer segments")
        let segments = expanded.rows.flatMap { row in row }.map(\.segment)
        XCTAssertEqual(Set(segments.map(\.id)), Set(snapshot.segments.filter { segment in
            segment.project == FixtureWire.key(0)
        }.map(\.id)))
        for segment in segments {
            XCTAssertEqual(segment, snapshot.segments.first { value in value.id == segment.id })
            let correction = TimelineSelection.agent(day: snapshot.day, segment)
                .correction(project: nil, transcript: false, day: snapshot.day, snapshot: snapshot)
            XCTAssertEqual(correction?.from?.timeIntervalSince1970, segment.from)
            XCTAssertEqual(correction?.to?.timeIntervalSince1970, segment.to)
        }
        XCTAssertTrue(drawing.presentedLanes().filter { value in value.id != folded.id }.allSatisfy { value in
            value.hiddenCount == 0 && value.toggleLabel == nil
        })
    }

    func testFoldCountsSegmentsRatherThanRowsAndKnowsTheSingular() throws {
        for (count, label) in [(3, nil), (4, "+1 more segment"), (5, "+2 more segments")] as [(Int, String?)] {
            let segments = Array(repeating: FixtureWire.Segment(from: 540, to: 600), count: count)
                + [.init(from: 800, to: 810)]
            let snapshot = try FixtureWire.timeline(segments: segments)
            let focus = TimelineDrawing.Span(from: snapshot.from + 480 * 60, to: snapshot.from + 600 * 60)
            let drawing = TimelineDrawing(snapshot, span: focus, scale: .focus, calendar: calendar)
            let lane = try XCTUnwrap(drawing.presentedLanes().first)
            XCTAssertEqual(lane.rows.count, 3)
            XCTAssertEqual(lane.hiddenCount, max(0, count - 3))
            XCTAssertEqual(lane.toggleLabel, label)
            let full = TimelineDrawing(snapshot, calendar: calendar)
            XCTAssertNil(full.presentedLanes().first?.toggleLabel)
            XCTAssertEqual(full.presentedLanes().first?.rows.flatMap { row in row }.count, count + 1)
        }
    }

    func testCaptionFallsBackThroughMeasuredLabelsAndNeverLeavesTheBlock() throws {
        let snapshot = try FixtureWire.timeline(segments: [.init(from: 0, to: 60)])
        let segment = try XCTUnwrap(snapshot.segments.first)
        let span = TimelineDrawing.Span(from: segment.from, to: segment.to)
        for (label, padding) in [("C · Claude Code · 1h", 16.0), ("C · 1h", 16.0), ("C", 4.0)] {
            let width = TimelineDrawing.textWidth(label) + padding
            let geometry = try XCTUnwrap(TimelineDrawing.geometry(from: segment.from, to: segment.to, span: span, width: width))
            let caption = try XCTUnwrap(TimelineDrawing.caption(segment, geometry: geometry, width: width))
            XCTAssertEqual(caption.text, label)
            XCTAssertGreaterThanOrEqual(caption.rect.x, geometry.paint.x)
            XCTAssertLessThanOrEqual(caption.rect.end, geometry.paint.end)
        }
        let width = TimelineDrawing.textWidth("C") + 3.9
        let geometry = try XCTUnwrap(TimelineDrawing.geometry(from: segment.from, to: segment.to, span: span, width: width))
        XCTAssertNil(TimelineDrawing.caption(segment, geometry: geometry, width: width))
    }

    func testCodexCaptionAndContinuationInsetsStayInsideTheVisibleBlock() throws {
        let snapshot = try FixtureWire.timeline(segments: [.init(agent: "codex", from: 0, to: 60)])
        let segment = try XCTUnwrap(snapshot.segments.first)
        let span = TimelineDrawing.Span(from: segment.from + 600, to: segment.to - 600)
        for (width, label) in [(160.0, "X · Codex · 1h"), (65, "X · 1h"), (25, "X")] {
            let geometry = try XCTUnwrap(TimelineDrawing.geometry(from: segment.from, to: segment.to, span: span, width: width))
            let caption = try XCTUnwrap(TimelineDrawing.caption(segment, geometry: geometry, width: width))
            XCTAssertEqual(caption.text, label)
            XCTAssertGreaterThanOrEqual(caption.rect.x, geometry.paint.x + 8)
            XCTAssertLessThanOrEqual(caption.rect.end, geometry.paint.end - 8)
        }
        let point = try XCTUnwrap(TimelineDrawing.geometry(from: segment.from, to: segment.from,
            span: .init(from: segment.from, to: segment.to), width: 580))
        XCTAssertNil(TimelineDrawing.caption(segment, geometry: point, width: 580))
    }

    func testRecordedIntervalsFoldByDefaultAndCountBothKindsWithOriginalBounds() throws {
        let snapshot = try FixtureWire.timeline(segments: [.init(from: 540, to: 600), .init(from: 610, to: 610)],
            owner: [.init(from: 540.25, to: 560.75)], available: false)
        let records = TimelineDrawing.recordedIntervals(snapshot)
        XCTAssertFalse(records.initiallyExpanded)
        XCTAssertEqual(records.title, "Recorded intervals (3)")
        XCTAssertEqual(records.selections.count, 3)
        XCTAssertEqual(records.selections.first, .agent(day: snapshot.day, snapshot.segments[0]))
        XCTAssertEqual(records.selections.last, .owner(day: snapshot.day, snapshot.owner[0]))
        XCTAssertEqual(records.selections.last?.from, snapshot.owner[0].from)
        XCTAssertEqual(records.selections.last?.to, snapshot.owner[0].to)
        XCTAssertEqual(TimelineDrawing.recordedIntervals(try FixtureWire.timeline()).title, "Recorded intervals (0)")
        XCTAssertEqual(TimelineDrawing.recordedIntervals(try FixtureWire.timeline(segments: [.init(from: 0, to: 1)])).title,
                       "Recorded intervals (1)")
    }
}
