import Foundation
import MnemonicShared
@testable import MnemonicApp

actor TimelineTestReplies<Argument: Sendable, Value: Sendable> {
    private var answer: @Sendable (Argument) -> Result<Value, LocalAPIFailure>
    private var held = false
    private var calls: [Argument] = []
    private var replies: [Int: (CheckedContinuation<Result<Value, LocalAPIFailure>, Never>,
                                Result<Value, LocalAPIFailure>)] = [:]
    private var notices: [Int: @Sendable () -> Void] = [:]
    private var active = 0
    private(set) var peakActive = 0

    init(_ answer: @escaping @Sendable (Argument) -> Result<Value, LocalAPIFailure>) {
        self.answer = answer
    }

    var count: Int { calls.count }
    func arguments() -> [Argument] { calls }
    func hold() { held = true }
    func set(_ result: Result<Value, LocalAPIFailure>) { answer = { _ in result } }
    func reset(_ result: @escaping @Sendable (Argument) -> Result<Value, LocalAPIFailure>) { answer = result }

    func requested(_ number: Int, _ notify: @escaping @Sendable () -> Void) {
        if calls.count >= number { notify() } else { notices[number] = notify }
    }

    func ask(_ argument: Argument) async -> Result<Value, LocalAPIFailure> {
        calls.append(argument)
        let number = calls.count, captured = answer(argument)
        active += 1
        peakActive = max(peakActive, active)
        defer { active -= 1 }
        let notify = notices.removeValue(forKey: number)
        guard held else { notify?(); return captured }
        return await withCheckedContinuation { reply in
            replies[number] = (reply, captured)
            notify?()
        }
    }

    func finish(_ number: Int) {
        guard let (reply, value) = replies.removeValue(forKey: number) else { return }
        reply.resume(returning: value)
    }

    func finishAll() {
        held = false
        for number in Array(replies.keys) { finish(number) }
    }
}

enum TimelineTestValues {
    static var calendar: Calendar {
        var result = Calendar(identifier: .gregorian)
        result.timeZone = TimeZone(secondsFromGMT: 0)!
        return result
    }

    static func date(_ day: String, hour: Double = 12) -> Date {
        TimelineDay.date(day, calendar: calendar)!.addingTimeInterval(hour * 3600)
    }

    static func snapshot(_ day: String, version: Int = 1) -> ProjectTimeline {
        let start = date(day, hour: 0)
        return ProjectTimeline(day: day, from: start.timeIntervalSince1970,
                               to: start.addingTimeInterval(86400).timeIntervalSince1970,
                               projects: [TimelineProject(key: "demoapp", name: "Version \(version)")],
                               segments: [], owner: [], ownerAvailable: true)
    }

    static func correction(_ day: String, stream: String? = nil) -> AttributionCorrection {
        let start = date(day, hour: 9)
        return AttributionCorrection(stream: stream, from: start,
                                     to: start.addingTimeInterval(3600), project: "demoapp")
    }
}

@MainActor
final class TimelineTestClock {
    var now: Date
    init(_ now: Date) { self.now = now }
}

@MainActor
final class TimelineControllerFixture {
    let clock = TimelineTestClock(TimelineTestValues.date("2026-09-30"))
    let reads = TimelineTestReplies<String, ProjectTimeline> { day in
        .success(TimelineTestValues.snapshot(day))
    }
    let adds = TimelineTestReplies<AttributionCorrection, Int64> { _ in .success(7) }
    let removes = TimelineTestReplies<Int64, Void> { _ in .success(()) }
    let controller: AppTimelineController

    init(day: String = "2026-09-30") {
        let clock = clock, reads = reads, adds = adds, removes = removes
        controller = AppTimelineController(
            date: { clock.now }, calendar: TimelineTestValues.calendar,
            read: { day in await reads.ask(day) }, add: { correction in await adds.ask(correction) },
            remove: { id in await removes.ask(id) })
        controller.setContext(day: day, visible: true)
    }

    func close() {
        controller.setContext(day: nil, visible: false)
        let reads = reads, adds = adds, removes = removes
        Task {
            await reads.finishAll()
            await adds.finishAll()
            await removes.finishAll()
        }
    }
}
