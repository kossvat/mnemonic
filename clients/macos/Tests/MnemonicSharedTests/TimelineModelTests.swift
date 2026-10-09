import Foundation
import MnemonicShared
import XCTest

final class TimelineModelTests: XCTestCase {
    func testDaemonFixturePreservesConcurrentSharesPointsAndUnknownValues() throws {
        let path = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/project-timeline.json")
        let timeline = try JSONDecoder().decode(ProjectTimeline.self, from: Data(contentsOf: path))
        XCTAssertEqual(timeline.day, "2026-09-29")
        XCTAssertEqual(timeline.to - timeline.from, 86400)
        XCTAssertTrue(timeline.ownerAvailable)
        XCTAssertEqual(timeline.projects.map(\.key), ["demoapp", "notes"])
        XCTAssertEqual(timeline.segments.count, 4)
        let point = timeline.segments[2]
        XCTAssertEqual(point.from, point.to)
        XCTAssertNil(timeline.segments[3].project)
        XCTAssertEqual(timeline.segments[3].agent, "unknown")
        XCTAssertEqual(timeline.segments[3].driver, "program")
        let shared = timeline.owner[1]
        XCTAssertEqual(shared.to - shared.from, 600)
        XCTAssertEqual(shared.assignments.map(\.share), [0.5, 0.5])
        XCTAssertEqual(shared.assignments.map(\.project), ["demoapp", "notes"])
        XCTAssertNil(timeline.owner[3].assignments[0].project)
        XCTAssertNil(timeline.owner[3].assignments[0].confidence)
        XCTAssertEqual(timeline.owner[4].from.truncatingRemainder(dividingBy: 1), 0.25)
        XCTAssertEqual(timeline.owner[4].to.truncatingRemainder(dividingBy: 1), 0.75)
        let encoded = try JSONEncoder().encode(timeline)
        XCTAssertEqual(try JSONDecoder().decode(ProjectTimeline.self, from: encoded), timeline)
    }

    func testUnavailableOwnerIsNotAnEmptyKnownOwnerLane() throws {
        let unavailable = ProjectTimeline(day: "2026-06-17", from: 1, to: 2,
                                          projects: [], segments: [], owner: [], ownerAvailable: false)
        let known = ProjectTimeline(day: unavailable.day, from: 1, to: 2,
                                    projects: [], segments: [], owner: [], ownerAvailable: true)
        XCTAssertNotEqual(unavailable, known)
        let encoded = try JSONEncoder().encode(unavailable)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
        XCTAssertEqual(object["owner_available"] as? Bool, false)
        XCTAssertEqual(try JSONDecoder().decode(ProjectTimeline.self, from: encoded), unavailable)
    }

    func testDayValidationUsesStrictGregorianDates() {
        for valid in ["2024-02-29", "2000-02-29", "2026-06-17", "1582-10-10", "0001-01-01", "9999-12-31"] {
            XCTAssertTrue(TimelineDay.isValid(valid), valid)
        }
        for invalid in ["", "2026-2-03", "2026-02-3", "2026-02-29", "1900-02-29", "2026-04-31",
                        "0000-01-01", "10000-01-01", "2026-00-01", "2026-13-01", "2026-01-00",
                        "2026-06-17 ", "2026-06-17T00:00:00Z", "2026/06/17", "２０２６-06-17"] {
            XCTAssertFalse(TimelineDay.isValid(invalid), invalid)
        }
    }

    func testDayHelpersRespectLocalDSTAndUseGregorianEvenWithAnotherCalendar() throws {
        var calendar = Calendar(identifier: .buddhist)
        calendar.timeZone = try XCTUnwrap(TimeZone(identifier: "America/New_York"))
        let spring = try XCTUnwrap(TimelineDay.date("2026-03-08", calendar: calendar))
        let springEnd = try XCTUnwrap(TimelineDay.date("2026-03-09", calendar: calendar))
        let autumn = try XCTUnwrap(TimelineDay.date("2026-11-01", calendar: calendar))
        let autumnEnd = try XCTUnwrap(TimelineDay.date("2026-11-02", calendar: calendar))
        XCTAssertEqual(springEnd.timeIntervalSince(spring), 23 * 3600)
        XCTAssertEqual(autumnEnd.timeIntervalSince(autumn), 25 * 3600)
        XCTAssertEqual(TimelineDay.string(spring, calendar: calendar), "2026-03-08")
        XCTAssertEqual(TimelineDay.today(now: autumnEnd.addingTimeInterval(-1), calendar: calendar), "2026-11-01")
        XCTAssertNil(TimelineDay.date("2026-02-29", calendar: calendar))
    }

    func testCorrectionEncodesExplicitNoneAndRFC3339WithoutRounding() throws {
        let start = Date(timeIntervalSince1970: 1_790_672_400)
        let correction = AttributionCorrection(stream: "fixture-stream", from: start,
                                               to: start.addingTimeInterval(60), project: nil)
        XCTAssertTrue(correction.isValid)
        let data = try JSONEncoder().encode(correction)
        let object = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
        XCTAssertTrue(object["project"] is NSNull)
        XCTAssertEqual(object["stream"] as? String, "fixture-stream")
        let from = try XCTUnwrap(object["from"] as? String)
        let to = try XCTUnwrap(object["to"] as? String)
        XCTAssertEqual(ISO8601DateFormatter().date(from: from), start)
        XCTAssertEqual(ISO8601DateFormatter().date(from: to), start.addingTimeInterval(60))
        let pin = AttributionCorrection(stream: "fixture-point", project: "demoapp:part-two")
        let pinObject = try XCTUnwrap(JSONSerialization.jsonObject(
            with: JSONEncoder().encode(pin)) as? [String: Any])
        XCTAssertNil(pinObject["from"])
        XCTAssertNil(pinObject["to"])
        XCTAssertEqual(pinObject["project"] as? String, "demoapp:part-two")
    }

    func testDayHelpersResolveMidnightGapsAndRejectAnEntirelySkippedDate() throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try XCTUnwrap(TimeZone(identifier: "America/Santiago"))
        let before = try XCTUnwrap(TimelineDay.date("2024-09-07", calendar: calendar))
        let gap = try XCTUnwrap(TimelineDay.date("2024-09-08", calendar: calendar))
        let after = try XCTUnwrap(TimelineDay.date("2024-09-09", calendar: calendar))
        XCTAssertEqual(calendar.component(.hour, from: before), 0)
        XCTAssertEqual(calendar.component(.hour, from: gap), 1)
        XCTAssertEqual(calendar.component(.hour, from: after), 0)
        XCTAssertEqual(gap.timeIntervalSince(before), 24 * 3600)
        XCTAssertEqual(after.timeIntervalSince(gap), 23 * 3600)
        XCTAssertEqual(calendar.dateInterval(of: .day, for: before)?.end, gap)
        XCTAssertEqual(calendar.dateInterval(of: .day, for: gap)?.end, after)
        XCTAssertEqual(TimelineDay.string(gap, calendar: calendar), "2024-09-08")
        calendar.timeZone = try XCTUnwrap(TimeZone(identifier: "Pacific/Apia"))
        XCTAssertNil(TimelineDay.date("2011-12-30", calendar: calendar))
    }

    func testInvalidCorrectionsCannotEncodeFractionalOrUnpairedRangesOrBadIdentities() {
        let start = Date(timeIntervalSince1970: 1_790_672_400)
        let invalid = [
            AttributionCorrection(project: nil),
            AttributionCorrection(stream: "", project: nil),
            AttributionCorrection(stream: "\nfixture", project: nil),
            AttributionCorrection(stream: String(repeating: "a", count: 4097), project: nil),
            AttributionCorrection(stream: "fixture", project: "../demoapp"),
            AttributionCorrection(from: start, project: "demoapp"),
            AttributionCorrection(to: start, project: "demoapp"),
            AttributionCorrection(from: start, to: start, project: nil),
            AttributionCorrection(from: start, to: start.addingTimeInterval(-1), project: nil),
            AttributionCorrection(from: start.addingTimeInterval(0.25), to: start.addingTimeInterval(1), project: nil),
            AttributionCorrection(from: start, to: start.addingTimeInterval(3660 * 86400), project: nil),
            AttributionCorrection(from: Date(timeIntervalSince1970: .infinity), to: start, project: nil),
        ]
        for correction in invalid {
            XCTAssertFalse(correction.isValid)
            XCTAssertThrowsError(try JSONEncoder().encode(correction))
        }
        XCTAssertTrue(AttributionCorrection(from: start, to: start.addingTimeInterval(1), project: nil).isValid)
        XCTAssertTrue(AttributionCorrection(stream: "[\"conversation\",\"fixture\"]", project: nil).isValid)
    }
}
