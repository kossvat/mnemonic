import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class TimelineGeometryTests: XCTestCase {
    private let focus = TimelineDrawing.Span(from: 0, to: 7_200)

    private func state(_ id: String) throws -> FixtureState {
        try XCTUnwrap(FixtureStates.timelines().first { state in state.id == id })
    }

    private func geometry(_ from: Double, _ to: Double, width: Double = 600) throws -> TimelineDrawing.Geometry {
        try XCTUnwrap(TimelineDrawing.geometry(from: from, to: to, span: focus, width: width))
    }

    func testAnHourIsHalfTheFocusAndOnlyWidthsUnderTwoPointsAreWidened() throws {
        let hour = try geometry(600, 4_200)
        XCTAssertEqual(hour.paint.x, 50)
        XCTAssertEqual(hour.paint.width, 300)
        XCTAssertEqual(hour.hitHeight, TimelineDrawing.rowHeight)
        XCTAssertEqual(hour.paintHeight, 34)
        XCTAssertEqual(hour.paintY, 4)
        for (seconds, painted) in [(6.0, 2.0), (24, 2), (36, 3), (120, 10)] {
            let block = try geometry(120, 120 + seconds)
            XCTAssertEqual(block.paint.x, 10)
            XCTAssertEqual(block.paint.width, painted, accuracy: 0.000_001)
        }
    }

    func testCutBlocksKeepTheirBoundsAndMarkEachContinuingEdge() throws {
        let left = try geometry(-600, 3_600)
        XCTAssertEqual(left.paint, .init(x: 0, width: 300))
        XCTAssertTrue(left.continuesBefore)
        XCTAssertFalse(left.continuesAfter)
        let right = try geometry(3_600, 7_800)
        XCTAssertEqual(right.paint, .init(x: 300, width: 300))
        XCTAssertFalse(right.continuesBefore)
        XCTAssertTrue(right.continuesAfter)
        let both = try geometry(-600, 7_800)
        XCTAssertTrue(both.continuesBefore && both.continuesAfter)
        XCTAssertEqual(both.paint.width, 600)
        XCTAssertNil(TimelineDrawing.geometry(from: 7_200, to: 7_800, span: focus, width: 600))
    }

    func testPointIsAFixedMarkCentredOnItsInstant() throws {
        for width in [300.0, 600, 1_440] {
            let point = try geometry(3_600, 3_600, width: width)
            XCTAssertTrue(point.point)
            XCTAssertEqual(point.paint.width, TimelineDrawing.pointWidth)
            XCTAssertEqual(point.paint.mid, width / 2)
            XCTAssertEqual(point.paintHeight, TimelineDrawing.pointWidth)
            XCTAssertEqual(point.hitHeight, TimelineDrawing.rowHeight)
            XCTAssertFalse(point.continuesBefore || point.continuesAfter)
        }
    }

    func testHitWidthsAndBothInwardMovesDoNotChangePaint() throws {
        XCTAssertEqual(try geometry(3_600, 3_606).hit.width, 24)
        XCTAssertEqual(try geometry(1_200, 4_800).hit, .init(x: 100, width: 300))
        let left = try geometry(0, 6)
        XCTAssertEqual(left.hit, .init(x: 0, width: 24))
        XCTAssertEqual(left.paint, .init(x: 0, width: 2))
        let right = try geometry(7_194, 7_200)
        XCTAssertEqual(right.hit, .init(x: 576, width: 24))
        // Widened to 2 points, it ends at the right edge instead of being cut there.
        XCTAssertEqual(right.paint, .init(x: 598, width: 2))
        let wide = try geometry(7_100, 7_200)
        XCTAssertEqual(wide.paint.x + wide.paint.width, 600, accuracy: 0.0001)
        XCTAssertEqual(try geometry(3_600, 3_606, width: 12).hit, .init(x: 0, width: 12))
        let cut = try geometry(-7_200, 600)
        XCTAssertEqual(cut.middle, -275)
        XCTAssertEqual(cut.hit, .init(x: 0, width: 50))
    }

    func testEveryNamedClickOfTimelineShort() throws {
        let fixture = try state("timeline-short")
        let snapshot = try XCTUnwrap(fixture.timeline)
        let drawing = TimelineDrawing(snapshot, width: 1_440, calendar: fixture.calendar)
        let lane = try XCTUnwrap(drawing.lanes.first)
        XCTAssertEqual(lane.tracks, 1)
        let expected: [(Double, String?)] = [
            (540.5, "segment-1"), (545, "segment-1"), (545.5, "segment-2"),
            (580, "segment-4"), (20, "segment-5"), (600, nil)
        ]
        for (x, id) in expected {
            XCTAssertEqual(drawing.segment(at: x, in: lane.blocks)?.id, id, "Click at \(x)")
        }
        XCTAssertNil(drawing.segment(at: -1, in: lane.blocks))
        XCTAssertNil(drawing.segment(at: 1_441, in: lane.blocks))
        let a = try XCTUnwrap(lane.blocks.first { block in block.id == "segment-1" })
        let point = try XCTUnwrap(lane.blocks.first { block in block.point })
        XCTAssertEqual(a.geometry.hit, .init(x: 528.5, width: 24))
        XCTAssertEqual(point.geometry.hit, .init(x: 568, width: 24))
    }

    func testTheLatestStartWinsInsideOverlappingIntervalsRegardlessOfHitDistance() throws {
        let snapshot = try FixtureWire.timeline(segments: [.init(from: 10, to: 70), .init(from: 40, to: 50)])
        let drawing = TimelineDrawing(snapshot, width: 1_440, calendar: FixtureWire.calendar())
        let blocks = try XCTUnwrap(drawing.lanes.first?.blocks)
        XCTAssertEqual(drawing.segment(at: 40, in: blocks)?.id, "segment-2")
        XCTAssertEqual(drawing.segment(at: 49, in: blocks)?.id, "segment-2")
        XCTAssertEqual(drawing.segment(at: 50, in: blocks)?.id, "segment-1")
    }

    func testCorrectionOfACutBlockCarriesTheWholeServedBounds() throws {
        let fixture = try state("timeline-edge")
        let snapshot = try XCTUnwrap(fixture.timeline)
        let span = TimelineDrawing.Span(from: snapshot.from + 540 * 60, to: snapshot.from + 660 * 60)
        let drawing = TimelineDrawing(snapshot, span: span, width: 600, calendar: fixture.calendar)
        let block = try XCTUnwrap(drawing.lanes.first?.blocks.first)
        XCTAssertTrue(block.geometry.continuesBefore)
        let selected = try XCTUnwrap(drawing.segment(at: 30, in: [block]))
        let correction = try XCTUnwrap(TimelineSelection.agent(day: snapshot.day, selected)
            .correction(project: nil, transcript: false, day: snapshot.day, snapshot: snapshot))
        XCTAssertEqual(correction.from?.timeIntervalSince1970, snapshot.segments.first?.from)
        XCTAssertEqual(correction.to?.timeIntervalSince1970, snapshot.segments.first?.to)
        XCTAssertLessThan(try XCTUnwrap(correction.from?.timeIntervalSince1970), span.from)
    }

    func testWholeDayUsesTwentyThreeAndTwentyFiveHoursWhileFocusKeepsTwo() throws {
        for (id, hours) in [("timeline-23h", 23.0), ("timeline-25h", 25.0)] {
            let fixture = try state(id)
            let snapshot = try XCTUnwrap(fixture.timeline)
            let day = TimelineDrawing(snapshot, width: 600, calendar: fixture.calendar)
            XCTAssertEqual(day.duration, hours * 3_600)
            XCTAssertEqual(try XCTUnwrap(day.lanes.first?.blocks.first).geometry.paint.width,
                           600 / hours, accuracy: 0.000_001)
            let span = TimelineDrawing.defaultFocus(snapshot, now: fixture.now, calendar: fixture.calendar)
            XCTAssertEqual(span.duration, 7_200)
            XCTAssertGreaterThanOrEqual(span.from, snapshot.from)
            XCTAssertLessThanOrEqual(span.to, snapshot.to)
        }
    }

    func testDefaultFocusForTodayPastDayAndNoSegments() throws {
        let snapshot = try FixtureWire.timeline(segments: [.init(from: 540, to: 600)])
        let now = Date(timeIntervalSince1970: snapshot.from + 720 * 60)
        let today = TimelineDrawing.defaultFocus(snapshot, now: now, calendar: FixtureWire.calendar())
        XCTAssertEqual(today, .init(from: snapshot.from + 600 * 60, to: now.timeIntervalSince1970))
        let past = TimelineDrawing.defaultFocus(snapshot, now: FixtureWire.now, calendar: FixtureWire.calendar())
        XCTAssertEqual(past, .init(from: snapshot.from + 480 * 60, to: snapshot.from + 600 * 60))
        let empty = try FixtureWire.timeline(owner: [.init(from: 540, to: 600)])
        XCTAssertEqual(TimelineDrawing.defaultFocus(empty, now: FixtureWire.now, calendar: FixtureWire.calendar()),
                       .init(from: empty.from + 480 * 60, to: empty.from + 600 * 60))
        let early = TimelineDrawing.defaultFocus(snapshot, now: Date(timeIntervalSince1970: snapshot.from + 60),
                                                calendar: FixtureWire.calendar())
        XCTAssertEqual(early, .init(from: snapshot.from, to: snapshot.from + 7_200))
        let earlyPast = try FixtureWire.timeline(segments: [.init(from: 30, to: 60)])
        let earlyEnd = TimelineDrawing.defaultFocus(earlyPast, now: FixtureWire.now, calendar: FixtureWire.calendar())
        XCTAssertEqual(earlyEnd, .init(from: earlyPast.from, to: earlyPast.from + 7_200))
    }

    func testStripClickCentresFocusAndKeepsItInsideTheDay() {
        let day = TimelineDrawing.Span(from: 0, to: 86_400)
        for (x, start) in [(720.0, 660.0), (0, 0), (1_440, 1_320), (-20, 0), (1_500, 1_320)] {
            let span = TimelineDrawing.focus(at: x, width: 1_440, day: day)
            XCTAssertEqual(span.from, start * 60)
            XCTAssertEqual(span.duration, 7_200)
        }
    }

    func testCrowdedLabelsStayInsideTheirBlocksAndNeverOverlap() throws {
        let fixture = try state("timeline-crowded")
        let snapshot = try XCTUnwrap(fixture.timeline)
        let span = TimelineDrawing.Span(from: snapshot.from + 540 * 60, to: snapshot.from + 660 * 60)
        for width in [240.0, 580, 1_440] {
            let drawing = TimelineDrawing(snapshot, span: span, width: width, calendar: fixture.calendar)
            let crowded = try XCTUnwrap(drawing.lanes.first { lane in lane.project == FixtureWire.key(0) })
            XCTAssertEqual(crowded.tracks, 12, "Twelve simultaneous blocks need twelve rows")
            XCTAssertEqual(crowded.blocks.count, 36)
            if width < 1_440 { XCTAssertTrue(crowded.blocks.allSatisfy { block in block.caption == nil }) }
            else { XCTAssertTrue(crowded.blocks.allSatisfy { block in ["C", "X"].contains(block.caption?.text ?? "") }) }
            for lane in drawing.lanes {
                for block in lane.blocks {
                    if let caption = block.caption {
                        XCTAssertGreaterThanOrEqual(caption.rect.x, block.geometry.paint.x)
                        XCTAssertLessThanOrEqual(caption.rect.end, block.geometry.paint.end)
                        XCTAssertGreaterThanOrEqual(caption.rect.width, TimelineDrawing.textWidth(caption.text))
                    }
                    for other in lane.blocks where other.id != block.id && other.track == block.track {
                        XCTAssertFalse(block.geometry.paint.overlaps(other.geometry.paint))
                        if let label = block.caption, let neighbour = other.caption {
                            XCTAssertFalse(label.rect.overlaps(neighbour.rect))
                        }
                    }
                }
            }
        }
        let wide = TimelineDrawing(try FixtureWire.timeline(segments: [.init(from: 540, to: 600)]),
                                   span: span, width: 580, calendar: fixture.calendar)
        XCTAssertEqual(wide.lanes.first?.blocks.first?.caption?.text, "C · Claude Code · 1h")
    }

    func testSequentialShortBlocksReuseOneRowAndDayLanesKeepTheirAgentNames() throws {
        let snapshot = try FixtureWire.timeline(projectCount: 2, segments: [
            .init(from: 540, to: 541), .init(agent: "codex", from: 542, to: 543), .init(from: 544, to: 545)
        ])
        let span = TimelineDrawing.Span(from: snapshot.from + 480 * 60, to: snapshot.from + 600 * 60)
        let drawing = TimelineDrawing(snapshot, span: span, width: 580, calendar: FixtureWire.calendar())
        XCTAssertEqual(drawing.lanes.count, 1, "A registered project without segments has no lane")
        XCTAssertEqual(drawing.lanes.first?.tracks, 1)
        XCTAssertEqual(drawing.lanes.first?.agents, "Claude Code · Codex")
        XCTAssertEqual(TimelineDrawing.segmentCount(1), "1 recorded segment")
        XCTAssertEqual(TimelineDrawing.segmentCount(3), "3 recorded segments")
    }

    func testLabelsFitAtTheirMeasuredThresholdAndAdjacentLabelsStayApart() throws {
        let snapshot = try FixtureWire.timeline(segments: [
            .init(from: 0, to: 60), .init(agent: "codex", from: 60, to: 120)
        ])
        let span = TimelineDrawing.Span(from: snapshot.from, to: snapshot.from + 7_200)
        let text = "C · Claude Code · 1h"
        let width = (TimelineDrawing.textWidth(text) + 16) * 2
        let fits = TimelineDrawing(snapshot, span: span, width: width, calendar: FixtureWire.calendar())
        let blocks = try XCTUnwrap(fits.lanes.first?.blocks)
        let first = try XCTUnwrap(blocks.first?.caption)
        let second = try XCTUnwrap(blocks.last?.caption)
        XCTAssertEqual(fits.lanes.first?.tracks, 1)
        XCTAssertEqual(first.text, text)
        XCTAssertFalse(first.rect.overlaps(second.rect))
        XCTAssertLessThanOrEqual(first.rect.end, blocks[0].geometry.paint.end)
        XCTAssertGreaterThanOrEqual(second.rect.x, blocks[1].geometry.paint.x)
        let narrow = TimelineDrawing(snapshot, span: span, width: width - 0.1, calendar: FixtureWire.calendar())
        XCTAssertEqual(narrow.lanes.first?.blocks.first?.caption?.text, "C · 1h")
    }

    func testPaintFloorAndPointFootprintsParticipateInMinimalRowPacking() throws {
        let snapshot = try FixtureWire.timeline(segments: [
            .init(from: 540, to: 541), .init(from: 541, to: 542),
            .init(from: 542, to: 543), .init(from: 550, to: 550), .init(from: 550, to: 550)
        ])
        let drawing = TimelineDrawing(snapshot, width: 1_440, calendar: FixtureWire.calendar())
        let lane = try XCTUnwrap(drawing.lanes.first)
        XCTAssertEqual(lane.tracks, 2)
        XCTAssertEqual(lane.blocks.map(\.track), [0, 1, 0, 0, 1])
        let reversed = ProjectTimeline(day: snapshot.day, from: snapshot.from, to: snapshot.to,
            projects: snapshot.projects, segments: Array(snapshot.segments.reversed()), owner: [], ownerAvailable: true)
        let reordered = TimelineDrawing(reversed, width: 1_440, calendar: FixtureWire.calendar())
        XCTAssertEqual(reordered.lanes.first?.blocks.map(\.id), lane.blocks.map(\.id))
        XCTAssertEqual(reordered.lanes.first?.blocks.map(\.track), lane.blocks.map(\.track))
    }

    func testOwnerPickingUsesTheSameInwardHitRuleAndKeepsFractionalBounds() throws {
        let snapshot = try FixtureWire.timeline(owner: [.init(from: 0.25 / 60, to: 60.75 / 60), .init(from: 10, to: 20)])
        let drawing = TimelineDrawing(snapshot, width: 1_440, calendar: FixtureWire.calendar())
        let first = try XCTUnwrap(drawing.owner.first)
        XCTAssertEqual(first.geometry.hit, .init(x: 0, width: 24))
        XCTAssertEqual(first.geometry.hitHeight, TimelineDrawing.ownerHeight)
        XCTAssertEqual(first.geometry.paintHeight, 12)
        XCTAssertEqual(first.range.from, snapshot.from + 0.25)
        XCTAssertEqual(first.range.to, snapshot.from + 60.75)
        XCTAssertEqual(drawing.ownerRange(at: 20)?.id, "owner-2")
        XCTAssertEqual(drawing.ownerRange(at: 2), snapshot.owner.first)
        XCTAssertNil(drawing.ownerRange(at: 40))
    }

    func testStripFrameAndAxisLabelsStayWithinTheirWidth() {
        let day = TimelineDrawing.Span(from: 0, to: 86_400)
        let focus = TimelineDrawing.Span(from: 9 * 3_600, to: 11 * 3_600)
        XCTAssertEqual(TimelineDrawing.frame(focus, in: day, width: 1_440), .init(x: 540, width: 120))
        for width in [12.0, 580, 1_440] {
            for fraction in [0.0, 0.25, 0.5, 0.75, 1] {
                let rect = TimelineDrawing.tickLabel(.init(at: fraction, position: fraction, label: "00:00", major: true), width: width)
                XCTAssertGreaterThanOrEqual(rect.x, 0)
                XCTAssertLessThanOrEqual(rect.end, width)
            }
        }
        XCTAssertEqual(TimelineDrawing.laneWidth(pageWidth: 1_032), 580)
    }

    func testInvalidGeometryNeverProducesAHitAndStripEdgesUseTheRealDay() {
        for width in [0.0, -1, .infinity, .nan] {
            XCTAssertNil(TimelineDrawing.geometry(from: 0, to: 60, span: focus, width: width))
        }
        XCTAssertNil(TimelineDrawing.geometry(from: 60, to: 0, span: focus, width: 600))
        XCTAssertNil(TimelineDrawing.geometry(from: .nan, to: 60, span: focus, width: 600))
        for hours in [23.0, 25] {
            let day = TimelineDrawing.Span(from: 0, to: hours * 3_600)
            let beginning = TimelineDrawing.focus(at: 0, width: 600, day: day)
            let end = TimelineDrawing.focus(at: 600, width: 600, day: day)
            XCTAssertEqual(beginning, .init(from: 0, to: 7_200))
            XCTAssertEqual(end, .init(from: day.to - 7_200, to: day.to))
            XCTAssertEqual(TimelineDrawing.frame(end, in: day, width: 600).width, 1_200 / hours, accuracy: 0.000_001)
        }
    }

    func testAxisLabelsDoNotCollideOnShortOrLongLocalDays() throws {
        for id in ["timeline-23h", "timeline-25h"] {
            let fixture = try state(id)
            let snapshot = try XCTUnwrap(fixture.timeline)
            for width in [12.0, 240, 580] {
                for span in [TimelineDrawing.Span(from: snapshot.from, to: snapshot.to),
                             .init(from: snapshot.from, to: snapshot.from + 7_200)] {
                    let drawing = TimelineDrawing(snapshot, span: span, width: width, calendar: fixture.calendar)
                    let labels = drawing.axisTicks.map { tick in TimelineDrawing.tickLabel(tick, width: width) }
                    for (first, next) in zip(labels, labels.dropFirst()) { XCTAssertLessThan(first.end, next.x) }
                    XCTAssertEqual(drawing.axisTicks.last?.at, span.to)
                }
            }
        }
    }
}
