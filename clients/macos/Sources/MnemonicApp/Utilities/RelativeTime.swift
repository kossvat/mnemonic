import Foundation

enum RelativeTime {
    private static let parser = ISO8601DateFormatter()
    private static let formatter: RelativeDateTimeFormatter = {
        let formatter = RelativeDateTimeFormatter()
        formatter.unitsStyle = .abbreviated
        return formatter
    }()

    static func string(from isoString: String?) -> String {
        guard let isoString, let date = parse(isoString) else {
            return "never"
        }
        return formatter.localizedString(for: date, relativeTo: Date())
    }

    static func parse(_ isoString: String) -> Date? {
        if let date = parser.date(from: isoString) {
            return date
        }
        let fallback = ISO8601DateFormatter()
        fallback.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        return fallback.date(from: isoString)
    }
}
