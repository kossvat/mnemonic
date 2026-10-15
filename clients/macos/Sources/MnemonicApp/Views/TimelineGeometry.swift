import AppKit
import MnemonicShared

extension TimelineDrawing {
    struct Span: Equatable {
        let from: Double
        let to: Double
        var duration: Double { to - from }
        var valid: Bool { from.isFinite && to.isFinite && duration.isFinite && to > from }
    }

    struct Rect: Equatable {
        let x: Double
        let width: Double
        var end: Double { x + width }
        var mid: Double { x + width / 2 }
        func contains(_ value: Double) -> Bool { value >= x && value <= end }
        func overlaps(_ other: Rect) -> Bool { x < other.end && other.x < end }
    }

    struct Geometry {
        let paint: Rect
        let hit: Rect
        let middle: Double
        let point: Bool
        let continuesBefore: Bool
        let continuesAfter: Bool
        let paintHeight: Double
        let hitHeight: Double
        var paintY: Double { (hitHeight - paintHeight) / 2 }
    }

    struct Caption {
        let text: String
        let rect: Rect
    }

    static let pointWidth = 6.0
    static let labelWidth = 154.0
    static let labelGap = 12.0
    static let rowHeight = 42.0
    static let ownerHeight = 40.0
    static let labelFont = NSFont.systemFont(ofSize: 10)

    static func laneWidth(pageWidth: Double) -> Double { max(1, pageWidth - 56 - 212 - 18 - labelWidth - labelGap) }
    static func laneHeight(_ lane: Lane) -> Double { max(76, Double(lane.tracks) * rowHeight + 14) }

    static func tickLabel(_ tick: Tick, width: Double) -> Rect {
        let size = min(width, textWidth(tick.label))
        return Rect(x: min(width - size, max(0, tick.position * width - size / 2)), width: size)
    }

    var axisTicks: [Tick] {
        let candidates = ticks.filter(\.major)
        guard let last = candidates.last else { return [] }
        let lastRect = Self.tickLabel(last, width: width)
        var result: [Tick] = [], end = -Double.infinity
        for tick in candidates.dropLast() {
            let rect = Self.tickLabel(tick, width: width)
            if rect.x >= end + 4 && rect.end + 4 <= lastRect.x {
                result.append(tick)
                end = rect.end
            }
        }
        result.append(last)
        return result
    }

    static func geometry(from: Double, to: Double, span: Span, width: Double,
                         rowHeight: Double = TimelineDrawing.rowHeight, paintHeight: Double = 34) -> Geometry? {
        guard span.valid, width.isFinite, width > 0, from.isFinite, to.isFinite, to >= from else { return nil }
        let point = from == to
        guard point ? (from >= span.from && from <= span.to) : (to > span.from && from < span.to) else { return nil }
        let start = (max(span.from, from) - span.from) * width / span.duration
        let length = max(0, min(span.to, to) - max(span.from, from)) * width / span.duration
        // A block widened to 2 points ends at the right edge of the lane
        // rather than past it, where the row would cut it to 1 point.
        let paint = point ? Rect(x: start - pointWidth / 2, width: pointWidth)
            : length < 2 ? Rect(x: min(start, width - 2), width: 2) : Rect(x: start, width: length)
        let middle = (from + (to - from) / 2 - span.from) * width / span.duration
        let hitWidth = min(width, max(24, paint.width))
        let hit = Rect(x: min(width - hitWidth, max(0, middle - hitWidth / 2)), width: hitWidth)
        return Geometry(paint: paint, hit: hit, middle: middle, point: point,
                        continuesBefore: from < span.from, continuesAfter: to > span.to,
                        paintHeight: point ? pointWidth : paintHeight, hitHeight: rowHeight)
    }

    static func textWidth(_ text: String) -> Double {
        ceil((text as NSString).size(withAttributes: [.font: labelFont]).width)
    }

    static func caption(_ segment: TimelineSegment, geometry: Geometry, width: Double) -> Caption? {
        guard !geometry.point else { return nil }
        let prefix = mark(segment.agent) == .claude ? "C" : mark(segment.agent) == .codex ? "X" : "?"
        let duration = ProjectPresentation.duration(segment.to - segment.from)
        let candidates = [prefix + " · " + agent(segment.agent) + " · " + duration, prefix + " · " + duration, prefix]
        for text in candidates {
            let inset = text == prefix ? 2.0 : 8.0
            let left = max(0, geometry.paint.x) + (geometry.continuesBefore ? max(8, inset) : inset)
            let right = min(width, geometry.paint.end) - (geometry.continuesAfter ? max(8, inset) : inset)
            let size = textWidth(text)
            if size <= right - left {
                return Caption(text: text, rect: Rect(x: left + (right - left - size) / 2, width: size))
            }
        }
        return nil
    }

    func segment(at x: Double, in blocks: [Block]) -> TimelineSegment? {
        pick(at: x, items: blocks, geometry: { block in block.geometry },
             bounds: { block in Span(from: block.segment.from, to: block.segment.to) }, id: { block in block.id })?.segment
    }

    func ownerRange(at x: Double) -> TimelineOwnerRange? {
        pick(at: x, items: owner, geometry: { block in block.geometry },
             bounds: { block in Span(from: block.range.from, to: block.range.to) }, id: { block in block.id })?.range
    }

    private func pick<Item>(at x: Double, items: [Item], geometry: (Item) -> Geometry,
                            bounds: (Item) -> Span, id: (Item) -> String) -> Item? {
        guard valid, width > 0, x.isFinite, x >= 0, x <= width else { return nil }
        let hits = items.filter { item in geometry(item).hit.contains(x) }
        guard !hits.isEmpty else { return nil }
        let instant = from + x * duration / width
        let inside = items.filter { item in
            let range = bounds(item)
            return range.from == range.to ? instant == range.from : instant >= range.from && instant < range.to
        }
        func later(_ left: Item, _ right: Item) -> Bool {
            bounds(left).from == bounds(right).from ? id(left) < id(right) : bounds(left).from > bounds(right).from
        }
        if !inside.isEmpty { return inside.sorted(by: later).first }
        return hits.sorted { left, right in
            let first = abs(geometry(left).middle - x), second = abs(geometry(right).middle - x)
            return first == second ? later(left, right) : first < second
        }.first
    }

    static func defaultFocus(_ snapshot: ProjectTimeline, now: Date, calendar: Calendar) -> Span {
        let day = Span(from: snapshot.from, to: snapshot.to)
        guard day.valid else { return day }
        // Today follows the clock. A past day follows the latest served work.
        if snapshot.day == TimelineDay.today(now: now, calendar: calendar) {
            return focus(endingAt: now.timeIntervalSince1970, day: day)
        }
        let ownerEnds = snapshot.ownerAvailable ? snapshot.owner.compactMap { range -> Double? in
            guard range.to > range.from,
                  geometry(from: range.from, to: range.to, span: day, width: 1) != nil else { return nil }
            return range.to
        } : []
        guard let end = (daySegments(snapshot).map(\.to) + ownerEnds).max() else { return day }
        return focus(endingAt: end, day: day)
    }

    static func focus(endingAt end: Double, day: Span) -> Span {
        guard day.valid else { return day }
        let length = min(7_200, day.duration)
        let start = min(day.to - length, max(day.from, end - length))
        return Span(from: start, to: start + length)
    }

    static func focus(at x: Double, width: Double, day: Span) -> Span {
        guard day.valid, x.isFinite, width.isFinite, width > 0 else { return focus(endingAt: day.from + 7_200, day: day) }
        let instant = day.from + min(1, max(0, x / width)) * day.duration
        return focus(endingAt: instant + 3_600, day: day)
    }

    static func frame(_ focus: Span, in day: Span, width: Double) -> Rect {
        guard day.valid else { return Rect(x: 0, width: 0) }
        return Rect(x: (focus.from - day.from) * width / day.duration, width: focus.duration * width / day.duration)
    }

    static func spanLabel(_ span: Span, calendar: Calendar) -> String {
        time(span.from, calendar: calendar) + " – " + time(span.to, calendar: calendar)
    }
}
