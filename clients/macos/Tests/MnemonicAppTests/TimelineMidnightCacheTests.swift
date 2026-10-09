import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class TimelineMidnightCacheTests: XCTestCase {
    @MainActor
    func testPastMidnightGapDayUsesCalendarEndAndCachesAcrossVisits() async throws {
        var calendar = Calendar(identifier: .gregorian)
        calendar.timeZone = try XCTUnwrap(TimeZone(identifier: "America/Santiago"))
        let day = "2024-09-08"
        let start = try XCTUnwrap(TimelineDay.date(day, calendar: calendar))
        let end = try XCTUnwrap(TimelineDay.date("2024-09-09", calendar: calendar))
        let now = try XCTUnwrap(TimelineDay.date("2024-09-10", calendar: calendar))
        XCTAssertEqual(end.timeIntervalSince(start), 23 * 3600)
        let value = ProjectTimeline(day: day, from: start.timeIntervalSince1970,
                                    to: end.timeIntervalSince1970, projects: [],
                                    segments: [], owner: [], ownerAvailable: true)
        let reads = TimelineTestReplies<String, ProjectTimeline> { _ in .success(value) }
        let controller = AppTimelineController(
            date: { now }, calendar: calendar, read: { requested in await reads.ask(requested) },
            add: { _ in .failure(.off) }, remove: { _ in .failure(.off) })
        controller.setContext(day: day, visible: true)
        await controller.refresh()
        await controller.refresh()
        controller.setContext(day: day, visible: false)
        controller.setContext(day: day, visible: true)
        XCTAssertEqual(controller.snapshot, value)
        await controller.refresh()
        let arguments = await reads.arguments()
        XCTAssertEqual(arguments, [day], "The 23-hour day must remain a valid historical cache entry")
        controller.setContext(day: nil, visible: false)
    }
}
