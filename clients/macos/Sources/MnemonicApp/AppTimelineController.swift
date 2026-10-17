import Foundation
import SwiftUI
import MnemonicShared

/// Timeline reads use the existing app clock; this controller owns no timer.
@MainActor
final class AppTimelineController: ObservableObject {
    @Published private(set) var snapshot: ProjectTimeline?
    @Published private(set) var failure: LocalAPIFailure?
    @Published private(set) var loading = false
    @Published private(set) var mutationInFlight = false
    @Published private(set) var message: String?
    @Published private(set) var canUndo = false
    @Published private(set) var day: String?
    @Published private(set) var visible = false

    private let date: @MainActor () -> Date
    private let calendar: Calendar
    private let read: @Sendable (String) async -> Result<ProjectTimeline, LocalAPIFailure>
    private let add: @Sendable (AttributionCorrection) async -> Result<Int64, LocalAPIFailure>
    private let remove: @Sendable (Int64) async -> Result<Void, LocalAPIFailure>
    private var visit: UInt64 = 0
    private var endpoint: UInt64 = 0
    private var running: Set<UInt64> = []
    private var invalidated: Set<UInt64> = []
    // Only responses requested after their day ended enter this 31-day cache.
    private var cache: [String: ProjectTimeline] = [:]
    private var cacheOrder: [String] = []
    private var undoRecord: (id: Int64, correction: AttributionCorrection)?
    private var mutation: UInt64 = 0

    init(
        date: @escaping @MainActor () -> Date = Date.init, calendar: Calendar = .autoupdatingCurrent,
        read: @escaping @Sendable (String) async -> Result<ProjectTimeline, LocalAPIFailure>,
        add: @escaping @Sendable (AttributionCorrection) async -> Result<Int64, LocalAPIFailure>,
        remove: @escaping @Sendable (Int64) async -> Result<Void, LocalAPIFailure>
    ) {
        self.date = date
        self.calendar = calendar
        self.read = read
        self.add = add
        self.remove = remove
    }

    convenience init(api: LocalAPI) {
        self.init(read: { day in await api.timeline(day: day) },
                  add: { correction in await api.addAttributionOverride(correction) },
                  remove: { id in await api.removeAttributionOverride(id: id) })
    }

    func setContext(day selected: String?, visible shown: Bool) {
        let selected = selected.flatMap { value in TimelineDay.isValid(value) ? value : nil }
        let shown = shown && selected != nil
        guard day != selected || visible != shown else { return }
        visit &+= 1
        day = selected
        visible = shown
        loading = false
        failure = nil
        message = nil
        snapshot = shown ? selected.flatMap { value in cached(value) } : nil
    }

    func refresh() async {
        guard visible, let selected = day, !running.contains(visit) else { return }
        if let value = cached(selected) {
            if snapshot != value { snapshot = value }
            if failure != nil { failure = nil }
            return
        }
        let started = visit
        running.insert(started)
        if !loading { loading = true }
        defer {
            running.remove(started)
            invalidated.remove(started)
            if visit == started, loading { loading = false }
        }
        repeat {
            invalidated.remove(started)
            let readDay = today
            let wasPast = selected < readDay
            let answer = await read(selected)
            guard current(selected, started) else { return }
            if invalidated.contains(started) { continue }
            if selected == readDay && selected < today {
                // Today's reply may be clipped before midnight. Finish the
                // now-historical day before publishing or caching it.
                invalidated.insert(started)
                continue
            }
            switch answer {
            case .success(let value):
                guard value.day == selected, value.from.isFinite, value.to.isFinite,
                      value.from < value.to else {
                    snapshot = nil
                    failure = .undecodable
                    return
                }
                if snapshot != value { snapshot = value }
                if failure != nil { failure = nil }
                if wasPast && selected < today { remember(value) }
            case .failure(let reason):
                if snapshot != nil { snapshot = nil }
                if failure != reason { failure = reason }
            }
        } while current(selected, started) && invalidated.contains(started)
    }

    /// Explicit refresh also invalidates historical cache entries.
    func invalidateSelected() {
        guard let day else { return }
        forget(day)
        invalidateRunningRead(onlyToday: false)
    }

    @discardableResult
    func invalidateRunningRead(onlyToday: Bool = true) -> Bool {
        guard visible, let day, running.contains(visit), !onlyToday || day == today else { return false }
        forget(day)
        invalidated.insert(visit)
        return true
    }

    func resetEndpoint() {
        endpoint &+= 1
        visit &+= 1
        mutation &+= 1
        cache.removeAll()
        cacheOrder.removeAll()
        undoRecord = nil
        canUndo = false
        mutationInFlight = false
        snapshot = nil
        failure = nil
        loading = false
        message = nil
    }

    @discardableResult
    func apply(_ correction: AttributionCorrection, for selected: String) async -> Bool {
        guard visible, day == selected, !mutationInFlight else { return false }
        guard correction.isValid else {
            message = "Choose a project and an exact whole-second range, or an explicit transcript pin."
            return false
        }
        let started = visit, origin = endpoint
        mutation &+= 1
        let operation = mutation
        mutationInFlight = true
        message = nil
        defer { if mutation == operation { mutationInFlight = false } }
        let answer = await add(correction)
        guard endpoint == origin else { return false }
        switch answer {
        case .success(let id) where id > 0:
            undoRecord = (id, correction)
            canUndo = true
            if current(selected, started) { message = "Correction saved. You can undo it." }
            await corrected(correction)
            return endpoint == origin
        case .success:
            if current(selected, started) { message = "The correction was not confirmed." }
        case .failure(let reason):
            if current(selected, started) { message = mutationMessage(reason) }
        }
        return false
    }

    @discardableResult
    func undo() async -> Bool {
        guard visible, let selected = day, !mutationInFlight, let record = undoRecord else { return false }
        let started = visit, origin = endpoint
        mutation &+= 1
        let operation = mutation
        mutationInFlight = true
        message = nil
        defer { if mutation == operation { mutationInFlight = false } }
        let answer = await remove(record.id)
        guard endpoint == origin else { return false }
        switch answer {
        case .success:
            undoRecord = nil
            canUndo = false
            if current(selected, started) { message = "Correction undone." }
            await corrected(record.correction)
            return endpoint == origin
        case .failure(let reason):
            if current(selected, started) { message = mutationMessage(reason) }
            return false
        }
    }

    private var today: String { TimelineDay.today(now: date(), calendar: calendar) }

    private func current(_ selected: String, _ started: UInt64) -> Bool {
        visible && visit == started && day == selected
    }

    private func cached(_ selected: String) -> ProjectTimeline? {
        guard selected < today, let value = cache[selected] else { return nil }
        guard let bounds = bounds(selected), value.from == bounds.from, value.to == bounds.to else {
            forget(selected)
            return nil
        }
        cacheOrder.removeAll { key in key == selected }
        cacheOrder.append(selected)
        return value
    }

    private func remember(_ value: ProjectTimeline) {
        cache[value.day] = value
        cacheOrder.removeAll { key in key == value.day }
        cacheOrder.append(value.day)
        while cacheOrder.count > 31 { cache.removeValue(forKey: cacheOrder.removeFirst()) }
    }

    private func forget(_ selected: String) {
        cache.removeValue(forKey: selected)
        cacheOrder.removeAll { key in key == selected }
    }

    private func corrected(_ correction: AttributionCorrection) async {
        let affected = cache.values.filter { value in
            overlaps(correction, from: value.from, to: value.to)
        }.map(\.day)
        for selected in affected { forget(selected) }
        guard visible, let selected = day, let bounds = bounds(selected),
              overlaps(correction, from: bounds.from, to: bounds.to)
        else { return }
        invalidateSelected()
        await refresh()
    }

    private func overlaps(_ correction: AttributionCorrection, from: Double, to: Double) -> Bool {
        guard let start = correction.from, let end = correction.to else { return true }
        return start.timeIntervalSince1970 < to && end.timeIntervalSince1970 > from
    }

    private func bounds(_ selected: String) -> (from: Double, to: Double)? {
        guard let start = TimelineDay.date(selected, calendar: calendar),
              let end = calendar.dateInterval(of: .day, for: start)?.end else { return nil }
        return (start.timeIntervalSince1970, end.timeIntervalSince1970)
    }

    private func mutationMessage(_ reason: LocalAPIFailure) -> String {
        switch reason {
        case .off: return "The local API is off. The correction was not confirmed."
        case .unreachable: return "The daemon could not confirm the correction."
        case .unauthorised: return "The daemon refused the correction token."
        case .earlierDaemon: return "The daemon could not find this correction route or record."
        case .undecodable: return "The daemon did not return a valid correction confirmation."
        }
    }
}
