import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class TimelineTotalsTests: XCTestCase {
    func testAgentTotalIsAUnionAcrossProjectsAndIncludesUnassignedWork() throws {
        let snapshot = try FixtureWire.timeline(projectCount: 2, segments: [
            .init(from: 10, to: 70), .init(project: 1, agent: "codex", from: 30, to: 90),
            .init(project: nil, from: 100, to: 110), .init(from: 110, to: 110)
        ])
        let totals = TimelineDrawing.totals(snapshot)
        XCTAssertEqual(totals.agents, 90 * 60)
        XCTAssertEqual(totals.projects.first { project in project.key == FixtureWire.key(0) }?.agents, 60 * 60)
        XCTAssertEqual(totals.projects.first { project in project.key == FixtureWire.key(1) }?.agents, 60 * 60)
        XCTAssertEqual(totals.projects.first { project in project.key == nil }?.agents, 10 * 60)
        XCTAssertGreaterThan(totals.projects.reduce(0) { total, project in total + project.agents }, totals.agents)
    }

    func testUnionClipsToTheDayAndRejectsInvalidIntervals() {
        let day = TimelineDrawing.Span(from: 0, to: 100)
        let intervals = [TimelineDrawing.Span(from: -20, to: 20), .init(from: 10, to: 30),
                         .init(from: 30, to: 40), .init(from: 80, to: 120), .init(from: 50, to: 50),
                         .init(from: 60, to: 20), .init(from: .nan, to: 10)]
        XCTAssertEqual(TimelineDrawing.union(intervals, within: day), 60)
        XCTAssertEqual(TimelineDrawing.union(Array(intervals.reversed()), within: day), 60)
    }

    func testOwnerIsCountedOnceAndAttributionSharesRemainConcurrent() throws {
        let snapshot = try FixtureWire.timeline(projectCount: 2, owner: [
            .init(from: 10, to: 30, projects: [0, 1]), .init(from: 30, to: 40, projects: [nil])
        ])
        let totals = TimelineDrawing.totals(snapshot)
        XCTAssertEqual(totals.owner, 30 * 60)
        XCTAssertEqual(totals.projects.first { project in project.key == FixtureWire.key(0) }?.owner, 10 * 60)
        XCTAssertEqual(totals.projects.first { project in project.key == FixtureWire.key(1) }?.owner, 10 * 60)
        XCTAssertEqual(totals.projects.first { project in project.key == nil }?.owner, 10 * 60)
    }

    func testUnavailableOwnerIsDifferentFromServedZeroAndEmptyWords() throws {
        let unavailable = try FixtureWire.timeline(available: false)
        let empty = try FixtureWire.timeline()
        XCTAssertNil(TimelineDrawing.totals(unavailable).owner)
        XCTAssertNil(TimelineDrawing.totals(unavailable).projects.first?.owner)
        XCTAssertEqual(TimelineDrawing.ownerTime(nil), "Unavailable")
        XCTAssertEqual(TimelineDrawing.ownerTime(TimelineDrawing.totals(empty).owner), "0m")
        XCTAssertEqual(TimelineDrawing.emptyMessage(empty), "No activity recorded for this day.")
        XCTAssertEqual(TimelineDrawing.emptyMessage(unavailable), "No agent sessions recorded for this day.")
        let activity = try FixtureWire.timeline(owner: [.init(from: 10, to: 20)])
        XCTAssertEqual(TimelineDrawing.emptyMessage(activity), "No agent sessions recorded for this day.")
    }

    func testLiveCountComesOnlyFromTheHeldSnapshotAndNamesTruncation() throws {
        XCTAssertNil(TimelineDrawing.liveCount(nil))
        XCTAssertEqual(TimelineDrawing.liveCount(try FixtureWire.live()), "0 sessions live")
        XCTAssertEqual(TimelineDrawing.liveCount(try FixtureWire.live(count: 1)), "1 session live")
        let snapshot = try FixtureWire.live(count: 1, moreProjects: 2, moreSessions: [0: 3],
                                            unknown: FixtureWire.sessions(["codex"]), moreUnknown: 2)
        XCTAssertEqual(TimelineDrawing.liveCount(snapshot), "At least 7 sessions live · snapshot incomplete")
    }
}
