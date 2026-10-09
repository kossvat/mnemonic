import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicBar

final class WidgetLiveTests: XCTestCase {
    private func snapshot() throws -> LiveSnapshot {
        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/live.json")
        return try JSONDecoder().decode(LiveSnapshot.self, from: Data(contentsOf: file))
    }

    @MainActor
    final class Clock: LiveClock {
        final class Armed: LiveTimer {
            var fire: (@MainActor () -> Void)?
            init(_ fire: @escaping @MainActor () -> Void) { self.fire = fire }
            func cancel() { fire = nil }
        }
        var timers: [Armed] = []
        func arm(after: TimeInterval, tolerance: TimeInterval,
                 _ fire: @escaping @MainActor () -> Void) -> LiveTimer {
            let timer = Armed(fire)
            timers.append(timer)
            return timer
        }
    }

    actor Answers {
        var script: [LiveAnswer]
        var count = 0
        init(_ script: [LiveAnswer]) { self.script = script }
        func next() -> LiveAnswer {
            count += 1
            return script.count > 1 ? script.removeFirst() : script[0]
        }
    }

    @MainActor
    func testClosedSignalsOnlyChangeTheTitleWhenTheProjectCountChanges() async throws {
        let snapshot = try snapshot()
        let answers = Answers([.snapshot(snapshot), .unchanged, .snapshot(snapshot), .failed(.undecodable)])
        let live = WidgetLiveController(clock: Clock()) { await answers.next() }
        var titles: [StatusTitle] = []
        live.onTitle = { title in titles.append(title) }
        await live.start()
        await live.coordinator.signalled()
        await live.coordinator.signalled()
        XCTAssertEqual(titles.count, 1)
        XCTAssertEqual(titles[0].label, "2 · 128")
        XCTAssertEqual(live.store.publications, 0)
        XCTAssertEqual(live.coordinator.armed, ["safety"])
        await live.coordinator.signalled()
        XCTAssertEqual(titles.count, 2)
        XCTAssertNil(titles[1].label)
        XCTAssertTrue(titles[1].alert)
        XCTAssertNil(live.store.snapshot)
        XCTAssertEqual(live.store.publications, 0)
        XCTAssertEqual(live.coordinator.armed, [])
    }

    /// Live projects first, then the memories of the store. A daemon
    /// that does not count them yet leaves the title as the live count.
    func testTheTitleNamesLiveProjectsAndTheCountOfMemories() throws {
        let counted = try snapshot()
        XCTAssertEqual(counted.memories, 128)
        XCTAssertEqual(StatusTitle(live: counted, failure: nil).label, "2 · 128")

        let file = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/live.json")
        var fields = try XCTUnwrap(
            JSONSerialization.jsonObject(with: Data(contentsOf: file)) as? [String: Any])
        fields.removeValue(forKey: "memories")
        let earlier = try JSONDecoder().decode(
            LiveSnapshot.self, from: JSONSerialization.data(withJSONObject: fields))
        XCTAssertNil(earlier.memories)
        XCTAssertEqual(StatusTitle(live: earlier, failure: nil).label, "2")
    }

    @MainActor
    func testOnlyTheVisibleLivePageHoldsPlanRefreshAndFailureClearsLiveRows() async throws {
        let answers = Answers([.snapshot(try snapshot()), .failed(.unreachable)])
        let live = WidgetLiveController(clock: Clock()) { await answers.next() }
        await live.setVisible(true)
        XCTAssertEqual(live.coordinator.armed, ["safety", "refresh", "labels"])
        live.selectPage(2)
        XCTAssertEqual(live.coordinator.armed, ["safety", "labels"])
        live.selectPage(0)
        XCTAssertEqual(live.coordinator.armed, ["safety", "refresh", "labels"])
        await live.coordinator.signalled()
        XCTAssertNil(live.store.shown)
        XCTAssertEqual(live.store.failure, .unreachable)
        XCTAssertEqual(live.coordinator.armed, ["refresh"])
        live.hide()
        XCTAssertEqual(live.coordinator.armed, [])
    }

    actor Delayed {
        var continuation: CheckedContinuation<LiveAnswer, Never>?
        private var ready: CheckedContinuation<Void, Never>?
        func next() async -> LiveAnswer {
            await withCheckedContinuation { pending in
                continuation = pending
                ready?.resume()
                ready = nil
            }
        }
        func waitForRequest() async {
            guard continuation == nil else { return }
            await withCheckedContinuation { pending in ready = pending }
        }
        func finish(_ answer: LiveAnswer) {
            continuation?.resume(returning: answer)
            continuation = nil
        }
    }

    @MainActor
    func testReplyAfterClosingCannotPublishOrRearmVisibleTimers() async throws {
        let delayed = Delayed()
        let live = WidgetLiveController(clock: Clock()) { await delayed.next() }
        let opening = Task { await live.setVisible(true) }
        await delayed.waitForRequest()
        live.hide()
        let publications = live.store.publications
        await delayed.finish(.snapshot(try snapshot()))
        await opening.value
        XCTAssertFalse(live.visible)
        XCTAssertFalse(live.store.visible)
        XCTAssertNil(live.store.shown)
        XCTAssertEqual(live.store.publications, publications)
        XCTAssertEqual(live.coordinator.armed, ["safety"])
    }

    @MainActor
    func testFixturePresentationKeepsAgentKindsHoursAndPlanTruthful() throws {
        let snapshot = try snapshot()
        let project = snapshot.projects[0]
        let groups = LiveProjectCard.agents(project.sessions)
        XCTAssertEqual(groups.map { group in group.0 }, ["claude", "codex"])
        XCTAssertEqual(groups.map { group in group.1 }, [1, 1])
        XCTAssertEqual(project.today.ownerSeconds, 5400)
        XCTAssertEqual(project.today.agentSeconds, 9000)
        XCTAssertEqual(try XCTUnwrap(LiveProjectCard.progress(try XCTUnwrap(project.plan))), 3.0 / 7)
        XCTAssertNil(snapshot.projects[1].plan)
        let zero = LivePlan(unit: "proposal", done: 0, total: 0, stage: "proposed", task: nil, gate: nil)
        XCTAssertNil(LiveProjectCard.progress(zero))
        let failures: [LocalAPIFailure] = [.off, .unreachable, .unauthorised, .earlierDaemon, .undecodable]
        XCTAssertEqual(Set(failures.map(\.widgetTitle)).count, 5)
        for failure in failures {
            let live = WidgetLiveController(clock: Clock()) { .failed(.off) }
            live.present(.snapshot(snapshot), at: .distantPast)
            live.present(.failed(failure), at: .distantPast)
            XCTAssertNil(live.store.shown)
            XCTAssertFalse(live.store.hasLiveSession)
            XCTAssertFalse(failure.widgetDetail.isEmpty)
            XCTAssertEqual(live.coordinator.armed, [])
        }
    }
}
