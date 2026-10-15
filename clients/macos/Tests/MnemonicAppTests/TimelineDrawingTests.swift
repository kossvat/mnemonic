import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class TimelineDrawingTests: XCTestCase {
    private func calendar(_ zone: String) throws -> Calendar {
        var value = Calendar(identifier: .gregorian)
        value.timeZone = try XCTUnwrap(TimeZone(identifier: zone))
        return value
    }

    private func snapshot(day: String, calendar: Calendar, segments: [TimelineSegment] = [],
                          owner: [TimelineOwnerRange] = [], available: Bool = true) throws -> ProjectTimeline {
        let start = try XCTUnwrap(TimelineDay.date(day, calendar: calendar))
        let end = try XCTUnwrap(calendar.dateInterval(of: .day, for: start)?.end)
        return ProjectTimeline(day: day, from: start.timeIntervalSince1970, to: end.timeIntervalSince1970,
                               projects: [TimelineProject(key: "demoapp", name: "Demo app")],
                               segments: segments, owner: owner, ownerAvailable: available)
    }

    private func segment(_ id: String, from: Double, to: Double,
                         project: String? = "demoapp", agent: String = "claude") -> TimelineSegment {
        TimelineSegment(id: id, stream: "fixture-" + id, project: project, agent: agent,
                        driver: "owner", from: from, to: to)
    }

    func testPositionsUseActualTwentyThreeTwentyFourAndTwentyFiveHourDays() throws {
        let local = try calendar("America/New_York")
        for (day, hours) in [("2026-03-08", 23.0), ("2026-06-17", 24.0), ("2026-11-01", 25.0)] {
            let empty = try snapshot(day: day, calendar: local)
            let recorded = segment("first", from: empty.from + 3_600, to: empty.from + 7_200)
            let value = try snapshot(day: day, calendar: local, segments: [recorded])
            let drawing = TimelineDrawing(value, calendar: local)
            XCTAssertEqual(drawing.duration, hours * 3_600)
            let block = try XCTUnwrap(drawing.lanes.first?.blocks.first)
            XCTAssertEqual(block.position, 1 / hours, accuracy: 0.000_001)
            XCTAssertEqual(block.length, 1 / hours, accuracy: 0.000_001)
            XCTAssertEqual(drawing.ticks.first?.position, 0)
            XCTAssertEqual(drawing.ticks.last?.position, 1)
            XCTAssertEqual(drawing.to, empty.to, "A partial day's evidence must not shorten the axis")
            if hours == 23 { XCTAssertFalse(drawing.ticks.contains { tick in tick.label == "02:00" }) }
            if hours == 25 {
                let repeated = drawing.ticks.filter { tick in tick.label.hasPrefix("01:00") }
                XCTAssertEqual(repeated.count, 2)
                XCTAssertNotEqual(repeated.first?.label, repeated.last?.label)
                XCTAssertEqual(repeated.first?.position ?? -1, 1 / 25, accuracy: 0.000_001)
                XCTAssertEqual(repeated.last?.position ?? -1, 2 / 25, accuracy: 0.000_001)
            }
        }
    }

    func testOverlappingWorkAndCoincidentPointsHaveStableSeparateTracks() throws {
        let utc = try calendar("UTC")
        let empty = try snapshot(day: "2026-06-17", calendar: utc)
        let start = empty.from
        let values = [
            segment("long", from: start + 3_600, to: start + 10_800),
            segment("overlap", from: start + 7_200, to: start + 14_400, agent: "codex"),
            segment("point-a", from: start + 9_000, to: start + 9_000, agent: "codex"),
            segment("point-b", from: start + 9_000, to: start + 9_000),
            segment("later", from: start + 14_400, to: start + 18_000),
            segment("unknown", from: start + 20_000, to: start + 21_000, project: nil, agent: "other")
        ]
        let drawing = TimelineDrawing(try snapshot(day: empty.day, calendar: utc, segments: values), calendar: utc)
        let reversed = TimelineDrawing(try snapshot(day: empty.day, calendar: utc, segments: Array(values.reversed())), calendar: utc)
        let lane = try XCTUnwrap(drawing.lanes.first { lane in lane.project == "demoapp" })
        XCTAssertEqual(lane.tracks, 4)
        let blocks = Dictionary(uniqueKeysWithValues: lane.blocks.map { block in (block.id, block) })
        XCTAssertEqual(blocks["long"]?.track, blocks["later"]?.track)
        XCTAssertNotEqual(blocks["long"]?.track, blocks["overlap"]?.track)
        XCTAssertNotEqual(blocks["point-a"]?.track, blocks["point-b"]?.track)
        XCTAssertEqual(blocks["point-a"]?.length, 0)
        XCTAssertEqual(blocks["point-b"]?.length, 0)
        XCTAssertTrue(blocks["point-a"]?.point == true)
        XCTAssertNotEqual(blocks["long"]?.mark, blocks["overlap"]?.mark)
        XCTAssertEqual(blocks["long"]?.label, "Claude Code")
        XCTAssertEqual(blocks["overlap"]?.label, "Codex")
        XCTAssertEqual(drawing.lanes.map(\.id), reversed.lanes.map(\.id))
        XCTAssertEqual(drawing.lanes.flatMap(\.blocks).map(\.track), reversed.lanes.flatMap(\.blocks).map(\.track))
        let unknown = try XCTUnwrap(drawing.lanes.first { lane in lane.project == nil })
        XCTAssertEqual(unknown.name, "Unassigned")
        XCTAssertTrue(unknown.blocks.first?.label.hasPrefix("Unknown agent") == true)
        XCTAssertEqual(unknown.blocks.first?.mark, .unknown)
    }

    func testMidnightGapAxisBeginsAtTheFirstRepresentableHour() throws {
        let local = try calendar("America/Santiago")
        let previous = try snapshot(day: "2024-09-07", calendar: local)
        let value = try snapshot(day: "2024-09-08", calendar: local)
        let next = try snapshot(day: "2024-09-09", calendar: local)
        let drawing = TimelineDrawing(value, calendar: local)
        XCTAssertEqual(previous.to, value.from)
        XCTAssertEqual(value.to, next.from)
        XCTAssertEqual(previous.to - previous.from, 24 * 3600)
        XCTAssertEqual(drawing.duration, 23 * 3600)
        XCTAssertEqual(drawing.ticks.first?.label, "01:00")
        XCTAssertEqual(drawing.ticks.first?.position, 0)
        XCTAssertEqual(drawing.ticks.last?.label, "00:00")
        XCTAssertEqual(drawing.ticks.last?.position, 1)
    }

    func testOwnerOnlySharesKeepTheirConcurrentFractionalBounds() throws {
        let utc = try calendar("UTC")
        let empty = try snapshot(day: "2026-06-17", calendar: utc)
        let range = TimelineOwnerRange(id: "owner-fractional", from: empty.from + 600.25, to: empty.from + 1_200.75,
            assignments: [TimelineAssignment(project: "demoapp", share: 0.5, confidence: "high"),
                          TimelineAssignment(project: nil, share: 0.5, confidence: nil)])
        let drawing = TimelineDrawing(try snapshot(day: empty.day, calendar: utc, owner: [range]), calendar: utc)
        XCTAssertTrue(drawing.lanes.isEmpty)
        XCTAssertTrue(drawing.ownerAvailable)
        XCTAssertEqual(drawing.owner.count, 1, "Concurrent attribution must not become sequential blocks")
        let owner = try XCTUnwrap(drawing.owner.first)
        XCTAssertEqual(owner.range, range)
        XCTAssertEqual(owner.position, 600.25 / 86_400, accuracy: 0.000_001)
        XCTAssertEqual(owner.length, 600.5 / 86_400, accuracy: 0.000_001)
        XCTAssertTrue(TimelineDrawing.time(range.from, calendar: utc).contains(".250"))
        XCTAssertTrue(TimelineDrawing.time(range.to, calendar: utc).contains(".750"))
        let unavailable = TimelineDrawing(try snapshot(day: empty.day, calendar: utc, available: false), calendar: utc)
        XCTAssertFalse(unavailable.ownerAvailable)
        XCTAssertTrue(unavailable.owner.isEmpty)
        XCTAssertFalse(unavailable.ticks.isEmpty, "Unavailable owner data does not erase the day axis")
    }

    func testTheSharedWireFixtureIsDrawableAndRetainsPointsAndFractionalOwnerEvidence() throws {
        let directory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures")
        let value = try JSONDecoder().decode(ProjectTimeline.self,
            from: Data(contentsOf: directory.appendingPathComponent("project-timeline.json")))
        let drawing = TimelineDrawing(value, calendar: try calendar("UTC"))
        XCTAssertTrue(drawing.valid)
        XCTAssertEqual(drawing.duration, 86_400)
        XCTAssertEqual(drawing.lanes.flatMap(\.blocks).count, value.segments.count)
        XCTAssertEqual(drawing.owner.count, value.owner.count)
        XCTAssertTrue(drawing.lanes.flatMap(\.blocks).contains { block in block.point && block.length == 0 })
        XCTAssertTrue(value.owner.contains { range in range.from.rounded(.towardZero) != range.from })
    }
}
