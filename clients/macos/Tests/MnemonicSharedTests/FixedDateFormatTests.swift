import Foundation
import XCTest
@testable import MnemonicShared

final class FixedDateFormatTests: XCTestCase {
    /// Every pattern the clients pass, and week patterns that read the week settings.
    private let formats = ["EEEE, d MMMM", "EEEE", "d MMM", "EEE, d MMM yyyy", "HH:mm", "yyyy-MM-dd",
                           "HH:mm:ss z", "HH:mm:ss.SSS z", "d MMM yyyy", "d MMM yyyy, HH:mm",
                           "EEEE, d MMMM yyyy", "w YYYY", "e c"]

    private func calendar(_ identifier: Calendar.Identifier, _ zone: String, firstWeekday: Int = 1,
                          minimumDays: Int = 1, locale: String? = nil) throws -> Calendar {
        var result = Calendar(identifier: identifier)
        if let locale { result.locale = Locale(identifier: locale) }
        result.timeZone = try XCTUnwrap(TimeZone(identifier: zone))
        result.firstWeekday = firstWeekday
        result.minimumDaysInFirstWeek = minimumDays
        return result
    }

    func testEachPatternWritesWhatAFormatterBuiltForTheCallWrites() throws {
        // A change of daylight time in New York, Lord Howe's half hour, and the turn of a year.
        let instants: [TimeInterval] = [0, 1_800_000_000.25, 1_793_515_500, 1_793_519_100, 1_806_217_200,
                                        1_798_759_800, 1_798_763_400, 1_798_977_600]
        for identifier in [Calendar.Identifier.gregorian, .buddhist, .iso8601] {
            for zone in ["UTC", "America/New_York", "Asia/Kathmandu", "Australia/Lord_Howe"] {
                for (firstWeekday, minimumDays, locale) in [(1, 1, nil), (2, 4, nil), (2, 1, "de_DE"), (1, 4, "ar_SA")] {
                    let selected = try calendar(identifier, zone, firstWeekday: firstWeekday,
                                                minimumDays: minimumDays, locale: locale)
                    for format in formats {
                        let fresh = DateFormatter()
                        fresh.locale = Locale(identifier: "en_US_POSIX")
                        fresh.calendar = selected
                        fresh.timeZone = selected.timeZone
                        fresh.dateFormat = format
                        for instant in instants {
                            let date = Date(timeIntervalSince1970: instant)
                            XCTAssertEqual(FixedDateFormat.string(date, format: format, calendar: selected),
                                           fresh.string(from: date),
                                           "\(identifier) \(zone) \(firstWeekday) \(minimumDays) \(locale ?? "none") \(format)")
                        }
                    }
                }
            }
        }
    }

    func testOneFormatterServesEachPatternAndCalendar() throws {
        let york = try calendar(.gregorian, "America/New_York")
        let first = FixedDateFormat.formatter(format: "d MMM", calendar: york)
        XCTAssertTrue(first === FixedDateFormat.formatter(format: "d MMM", calendar: york))
        let others = [
            FixedDateFormat.formatter(format: "HH:mm", calendar: york),
            FixedDateFormat.formatter(format: "d MMM", calendar: try calendar(.gregorian, "UTC")),
            FixedDateFormat.formatter(format: "d MMM", calendar: try calendar(.buddhist, "America/New_York")),
            FixedDateFormat.formatter(format: "d MMM", calendar: try calendar(.gregorian, "America/New_York", firstWeekday: 2)),
            FixedDateFormat.formatter(format: "d MMM", calendar: try calendar(.gregorian, "America/New_York", minimumDays: 4)),
        ]
        for other in others { XCTAssertFalse(first === other) }
    }
}
