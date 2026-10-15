import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class FixtureStateDigestTests: XCTestCase {
    private func state(_ id: String) throws -> FixtureState {
        try XCTUnwrap(FixtureStates.all().first { candidate in candidate.id == id })
    }

    private func replacing<Value: Codable>(_ value: Value, change: (inout FixtureWire.Object) throws -> Void) throws -> Value {
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(value)) as? FixtureWire.Object)
        try change(&object)
        return try FixtureWire.decode(object, as: Value.self)
    }

    func testEveryStateHasTheWrittenDigestOfItsInputs() throws {
        let states = try FixtureStates.all()
        XCTAssertEqual(Set(FixtureStateDigests.byID.keys), Set(states.map(\.id)))
        for state in states {
            XCTAssertEqual(try FixtureStateDigest.of(state), FixtureStateDigests.byID[state.id], "digest of \(state.id)")
        }
    }

    func testTheDigestChangesWithInputsTheSummaryDoesNotName() throws {
        let board = try state("projects-full")
        var week = board
        week.board = try replacing(XCTUnwrap(board.board)) { object in
            var rows = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
            var weekly = try XCTUnwrap(rows[0]["week"] as? FixtureWire.Object)
            weekly["owner_s"] = NSNull()
            rows[0]["week"] = weekly
            object["projects"] = rows
        }
        XCTAssertNotEqual(try FixtureStateDigest.of(board), try FixtureStateDigest.of(week))

        let page = try state("project-full")
        var task = page
        task.detail = try replacing(XCTUnwrap(page.detail)) { object in
            var plan = try XCTUnwrap(object["plan"] as? FixtureWire.Object)
            var units = try XCTUnwrap(plan["units"] as? [FixtureWire.Object])
            units[1]["current_task"] = ["id": "T02", "title": "Verify the result", "source": "first_open"]
            plan["units"] = units
            object["plan"] = plan
        }
        XCTAssertNotEqual(try FixtureStateDigest.of(page), try FixtureStateDigest.of(task))
        var sessions = page
        sessions.detail = try replacing(XCTUnwrap(page.detail)) { object in
            object["sessions"] = Array(try XCTUnwrap(object["sessions"] as? [FixtureWire.Object]).reversed())
        }
        XCTAssertNotEqual(try FixtureStateDigest.of(page), try FixtureStateDigest.of(sessions))

        let day = try state("timeline-short")
        var reversed = day
        reversed.timeline = try replacing(XCTUnwrap(day.timeline)) { object in
            object["segments"] = Array(try XCTUnwrap(object["segments"] as? [FixtureWire.Object]).reversed())
        }
        XCTAssertNotEqual(try FixtureStateDigest.of(day), try FixtureStateDigest.of(reversed))
    }
}
