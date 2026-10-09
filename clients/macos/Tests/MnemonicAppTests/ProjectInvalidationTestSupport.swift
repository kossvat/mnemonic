import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

actor ProjectPlanRead<Value: Sendable> {
    private var answer: Result<Value, LocalAPIFailure>
    private var held = false
    private(set) var count = 0
    private(set) var active = 0
    private(set) var peakActive = 0
    private var notices: [Int: @Sendable () -> Void] = [:]
    private var replies: [Int: (CheckedContinuation<Result<Value, LocalAPIFailure>, Never>,
                                Result<Value, LocalAPIFailure>)] = [:]

    init(_ value: Value) { answer = .success(value) }
    func hold() { held = true }
    func set(_ value: Result<Value, LocalAPIFailure>) { answer = value }

    func requested(_ number: Int, _ notify: @escaping @Sendable () -> Void) {
        if count >= number { notify() } else { notices[number] = notify }
    }

    func read() async -> Result<Value, LocalAPIFailure> {
        count += 1
        active += 1
        peakActive = max(peakActive, active)
        defer { active -= 1 }
        let number = count, captured = answer
        let notify = notices.removeValue(forKey: number)
        guard held else { notify?(); return captured }
        return await withCheckedContinuation { reply in
            replies[number] = (reply, captured)
            notify?()
        }
    }

    func finish(_ number: Int) {
        guard let (reply, captured) = replies.removeValue(forKey: number) else { return }
        reply.resume(returning: captured)
    }

    func finishAll() {
        held = false
        let numbers = Array(replies.keys)
        for number in numbers { finish(number) }
    }
}

struct ProjectInvalidationValues {
    let board: ProjectBoard
    let detail: ProjectDetail

    init(_ fixture: ProjectTestFixtures, done: Int, key: String = "demoapp") throws {
        func changed<T: Codable>(_ value: T, edit: (inout [String: Any]) throws -> Void) throws -> T {
            var object = try XCTUnwrap(JSONSerialization.jsonObject(
                with: JSONEncoder().encode(value)) as? [String: Any])
            try edit(&object)
            return try JSONDecoder().decode(T.self, from: JSONSerialization.data(withJSONObject: object))
        }
        board = try changed(fixture.board) { object in
            var projects = try XCTUnwrap(object["projects"] as? [[String: Any]])
            var project = try XCTUnwrap(projects.first)
            var plan = try XCTUnwrap(project["plan"] as? [String: Any])
            plan["done"] = done
            project["plan"] = plan
            project["key"] = key
            projects[0] = project
            object["projects"] = projects
        }
        detail = try changed(fixture.detail) { object in
            var plan = try XCTUnwrap(object["plan"] as? [String: Any])
            var units = try XCTUnwrap(plan["units"] as? [[String: Any]])
            var unit = try XCTUnwrap(units.first)
            unit["done"] = done
            units[0] = unit
            plan["units"] = units
            object["plan"] = plan
            object["key"] = key
        }
    }
}

@MainActor
final class ProjectInvalidationFixture {
    let data: ProjectTestFixtures
    let clock = ProjectTestClock()
    let live: ProjectTestSource
    let board: ProjectPlanRead<ProjectBoard>
    let detail: ProjectPlanRead<ProjectDetail>
    let controller: AppProjectController
    let kind: ProjectTestRequest

    init(_ route: AppRoute) throws {
        let data = try ProjectTestFixtures()
        self.data = data
        let first = try ProjectInvalidationValues(data, done: 0)
        let live = ProjectTestSource(data)
        let board = ProjectPlanRead(first.board), detail = ProjectPlanRead(first.detail)
        self.live = live
        self.board = board
        self.detail = detail
        kind = route == .projects ? .board : .detail
        let clock = clock
        controller = AppProjectController(
            clock: clock, date: { clock.now }, ask: { await live.askLive() },
            board: { await board.read() }, detail: { _ in await detail.read() })
        controller.select(route)
    }

    func hold() async {
        if kind == .board { await board.hold() } else { await detail.hold() }
    }

    func answer(done: Int, failure: LocalAPIFailure? = nil, key: String = "demoapp") async throws {
        let values = try ProjectInvalidationValues(data, done: done, key: key)
        await board.set(failure.map { reason in .failure(reason) } ?? .success(values.board))
        await detail.set(failure.map { reason in .failure(reason) } ?? .success(values.detail))
    }

    func requested(_ number: Int, _ notify: @escaping @Sendable () -> Void) async {
        if kind == .board { await board.requested(number, notify) }
        else { await detail.requested(number, notify) }
    }

    func count() async -> Int {
        if kind == .board { return await board.count }
        return await detail.count
    }
    func peak() async -> Int {
        if kind == .board { return await board.peakActive }
        return await detail.peakActive
    }
    func finish(_ number: Int) async {
        if kind == .board { await board.finish(number) } else { await detail.finish(number) }
    }
    func finishAll() async {
        await board.finishAll()
        await detail.finishAll()
        await live.finishLive()
    }
    var shownDone: Int? {
        kind == .board ? controller.board?.projects.first?.plan?.done : controller.detail?.plan.units.first?.done
    }
}
