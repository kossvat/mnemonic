import Foundation

/// Stable Gregorian route keys with day boundaries in the selected timezone.
public enum TimelineDay {
    public static func today(now: Date = Date(), calendar: Calendar = .current) -> String {
        string(now, calendar: calendar)
    }

    public static func string(_ date: Date, calendar: Calendar = .current) -> String {
        let formatter = DateFormatter()
        formatter.calendar = gregorian(calendar)
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = calendar.timeZone
        formatter.dateFormat = "yyyy-MM-dd"
        return formatter.string(from: date)
    }

    public static func date(_ value: String, calendar: Calendar = .current) -> Date? {
        guard let parts = components(value) else { return nil }
        let selected = gregorian(calendar)
        guard let noon = selected.date(from: DateComponents(
            year: parts.year, month: parts.month, day: parts.day, hour: 12)),
              selected.component(.year, from: noon) == parts.year,
              selected.component(.month, from: noon) == parts.month,
              selected.component(.day, from: noon) == parts.day else { return nil }
        return selected.startOfDay(for: noon)
    }

    public static func isValid(_ value: String) -> Bool {
        guard let parts = components(value) else { return false }
        let leap = parts.year.isMultiple(of: 4)
            && (!parts.year.isMultiple(of: 100) || parts.year.isMultiple(of: 400))
        let lengths = [31, leap ? 29 : 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31]
        return parts.day <= lengths[parts.month - 1]
    }

    private static func gregorian(_ calendar: Calendar) -> Calendar {
        var result = Calendar(identifier: .gregorian)
        result.locale = Locale(identifier: "en_US_POSIX")
        result.timeZone = calendar.timeZone
        return result
    }

    private static func components(_ value: String) -> (year: Int, month: Int, day: Int)? {
        let bytes = Array(value.utf8)
        guard bytes.count == 10, bytes[4] == 45, bytes[7] == 45,
              bytes.enumerated().allSatisfy({ index, byte in
                  index == 4 || index == 7 || (48...57).contains(byte)
              }),
              let year = Int(value.prefix(4)), let month = Int(value.dropFirst(5).prefix(2)),
              let day = Int(value.suffix(2)), (1...9999).contains(year),
              (1...12).contains(month), (1...31).contains(day) else { return nil }
        return (year, month, day)
    }
}
