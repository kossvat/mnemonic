import Foundation

/// Fixed POSIX date patterns, each formatter built once. Building a
/// DateFormatter loads its ICU data, which cost more than the rest of a board
/// redraw; formatting with one already built is safe from any thread.
public enum FixedDateFormat {
    private struct Key: Hashable {
        let format: String
        let calendar: Calendar.Identifier
        let timeZone: TimeZone
        let firstWeekday: Int
        let minimumDaysInFirstWeek: Int
    }

    /// The formatters built so far, behind one lock.
    private final class Cache: @unchecked Sendable {
        private let lock = NSLock()
        private var formatters: [Key: DateFormatter] = [:]

        func formatter(_ key: Key, calendar: Calendar) -> DateFormatter {
            lock.lock()
            defer { lock.unlock() }
            if let built = formatters[key] { return built }
            // Patterns are few; zones change only with the system's setting.
            if formatters.count >= 64 { formatters.removeAll() }
            let built = DateFormatter()
            built.locale = Locale(identifier: "en_US_POSIX")
            built.calendar = calendar
            built.timeZone = calendar.timeZone
            built.dateFormat = key.format
            formatters[key] = built
            return built
        }
    }

    private static let cache = Cache()

    public static func string(_ date: Date, format: String, calendar: Calendar) -> String {
        formatter(format: format, calendar: calendar).string(from: date)
    }

    static func formatter(format: String, calendar: Calendar) -> DateFormatter {
        cache.formatter(Key(format: format, calendar: calendar.identifier, timeZone: calendar.timeZone,
                            firstWeekday: calendar.firstWeekday,
                            minimumDaysInFirstWeek: calendar.minimumDaysInFirstWeek), calendar: calendar)
    }
}
