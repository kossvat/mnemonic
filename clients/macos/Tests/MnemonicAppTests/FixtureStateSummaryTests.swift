import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class FixtureStateSummaryTests: XCTestCase {
    private func state(_ id: String) throws -> FixtureState {
        try XCTUnwrap(FixtureStates.all().first { candidate in candidate.id == id })
    }

    private func replacing<Value: Codable>(_ value: Value, change: (inout FixtureWire.Object) throws -> Void) throws -> Value {
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(value)) as? FixtureWire.Object)
        try change(&object)
        return try FixtureWire.decode(object, as: Value.self)
    }

    func testSummaryChangesWhenOnlyLiveCardNamesAreReplaced() throws {
        let original = try state("projects-long")
        let donor = try XCTUnwrap(state("projects-full").live)
        var changed = original
        changed.live = try replacing(XCTUnwrap(original.live)) { object in
            var projects = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
            for index in projects.indices { projects[index]["name"] = donor.projects[index].name }
            object["projects"] = projects
        }
        XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed))
    }

    func testSummaryChangesWhenOnlyOtherRowNamesAreReplaced() throws {
        let original = try state("projects-none-live")
        let donor = try XCTUnwrap(state("projects-long").board)
        var changed = original
        changed.board = try replacing(XCTUnwrap(original.board)) { object in
            var rows = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
            for index in rows.indices { rows[index]["name"] = donor.projects[index].name }
            object["projects"] = rows
        }
        XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed))
    }

    /// The kind of a plan is not drawn; changing it where the board looks the same keeps the summary.
    func testSummaryIgnoresAKindOfPlanTheBoardDoesNotDraw() throws {
        let original = try state("projects-enriched")
        var changed = original
        changed.board = try replacing(XCTUnwrap(original.board)) { object in
            var rows = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
            for index in rows.indices where rows[index]["plan_provider"] as? String == "checkbox" {
                rows[index]["plan_provider"] = "registry"
            }
            object["projects"] = rows
        }
        XCTAssertNotEqual(original.board, changed.board)
        XCTAssertEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed))
        XCTAssertFalse(FixtureStateSummary.summary(of: original).contains("provider="))
    }

    func testSummaryChangesWhenOnlyTheCurrentTaskTextIsReplaced() throws {
        let original = try state("projects-long")
        let donor = try XCTUnwrap(state("projects-full").board?.projects.first?.plan?.task)
        var changed = original
        changed.board = try replacing(XCTUnwrap(original.board)) { object in
            var projects = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
            var plan = try XCTUnwrap(projects[0]["plan"] as? FixtureWire.Object)
            plan["task"] = donor
            projects[0]["plan"] = plan
            object["projects"] = projects
        }
        XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed))
    }

    func testBoardSummaryNamesHeaderAndCardTimesSeparately() throws {
        let original = try state("projects-full")
        for field in ["owner_s", "agent_s"] {
            var header = original
            header.live = try replacing(XCTUnwrap(original.live)) { object in
                var today = try XCTUnwrap(object["today"] as? FixtureWire.Object)
                today[field] = 0
                object["today"] = today
            }
            XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: header), field)
            var card = original
            card.live = try replacing(XCTUnwrap(original.live)) { object in
                var projects = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
                var today = try XCTUnwrap(projects[0]["today"] as? FixtureWire.Object)
                today[field] = 0
                projects[0]["today"] = today
                object["projects"] = projects
            }
            XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: card), field)
            var week = original
            week.board = try replacing(XCTUnwrap(original.board)) { object in
                var projects = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
                var value = try XCTUnwrap(projects[0]["week"] as? FixtureWire.Object)
                value[field] = 0
                projects[0]["week"] = value
                object["projects"] = projects
            }
            XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: week), field)
        }
    }

    func testBoardSummaryNamesProgressAbsenceGateAndLongestSession() throws {
        let original = try state("projects-full")
        for (field, value) in [("done", 0 as Any), ("total", 0 as Any), ("gate", "stale" as Any)] {
            var changed = original
            changed.board = try replacing(XCTUnwrap(original.board)) { object in
                var projects = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
                var plan = try XCTUnwrap(projects[0]["plan"] as? FixtureWire.Object)
                plan[field] = value
                projects[0]["plan"] = plan
                object["projects"] = projects
            }
            XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed), field)
        }
        var changed = original
        changed.live = try replacing(XCTUnwrap(original.live)) { object in
            var projects = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
            var sessions = try XCTUnwrap(projects[0]["sessions"] as? [FixtureWire.Object])
            sessions[0]["started_at"] = Int(original.now.timeIntervalSince1970) - 7_200
            projects[0]["sessions"] = sessions
            object["projects"] = projects
        }
        XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed))
        XCTAssertTrue(FixtureStateSummary.summary(of: changed).contains("longest=2h"))
    }

    func testBoardSummaryActiveLineUsesAllKindsAndSuppressesThemForACut() throws {
        let original = try state("projects-full")
        var changed = original
        changed.live = try replacing(XCTUnwrap(original.live)) { object in
            object["unknown"] = ["sessions": FixtureWire.sessions(["codex"]), "more_sessions": 0]
        }
        XCTAssertTrue(FixtureStateSummary.summary(of: changed).contains("active=[3 projects; 2 Claude Code · 3 Codex]"))
        let cut = FixtureStateSummary.summary(of: try state("projects-cut"))
        XCTAssertTrue(cut.contains("active=[3 projects; Snapshot incomplete]"))
        XCTAssertTrue(cut.contains("agents=absent"))
        XCTAssertTrue(cut.contains("longest=absent"))
    }

    func testBoardSummaryNamesEveryVisibleAddition() throws {
        let original = try state("projects-enriched")
        let summary = FixtureStateSummary.summary(of: original)
        for (field, value) in [("owner_s", 0 as Any), ("agent_s", 0 as Any), ("owner_s", NSNull() as Any)] {
            var changed = original
            changed.board = try replacing(XCTUnwrap(original.board)) { object in
                var week = try XCTUnwrap(object["week"] as? FixtureWire.Object)
                week[field] = value
                object["week"] = week
            }
            XCTAssertNotEqual(summary, FixtureStateSummary.summary(of: changed), field)
        }
        let replacements: [(String, Any)] = [
            ("gate_at", Int(original.now.timeIntervalSince1970) - 172_800), ("plan_provider", "checkbox"),
            ("plan_counts", ["contacted": 0]), ("plan_weekly_count", 0),
            ("waiting_on_owner", ["Keep the local cache"])
        ]
        for (field, value) in replacements {
            var changed = original
            changed.board = try replacing(XCTUnwrap(original.board)) { object in
                var projects = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
                projects[1][field] = value
                object["projects"] = projects
            }
            XCTAssertNotEqual(summary, FixtureStateSummary.summary(of: changed), field)
        }
        for field in ["plan_counts", "plan_weekly_count"] {
            var changed = original
            changed.board = try replacing(XCTUnwrap(original.board)) { object in
                var projects = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
                projects[3][field] = NSNull()
                object["projects"] = projects
            }
            XCTAssertNotEqual(summary, FixtureStateSummary.summary(of: changed), "row " + field)
        }
    }

    func testBoardSummaryNamesOnlyTheChosenFooterFactAndNoCardProvider() throws {
        let original = try state("projects-enriched")
        let summary = FixtureStateSummary.summary(of: original)
        XCTAssertTrue(summary.contains("footer=Waiting for you: Verify the result}"))
        XCTAssertFalse(summary.contains("longest=1h; provider="))
        for (field, value) in [("open_units", 0 as Any),
                                ("last_closed", ["title": "Review the guide", "closed_at": NSNull()] as Any)] {
            var changed = original
            changed.board = try replacing(XCTUnwrap(original.board)) { object in
                var projects = try XCTUnwrap(object["projects"] as? [FixtureWire.Object])
                projects[1][field] = value
                object["projects"] = projects
            }
            XCTAssertEqual(summary, FixtureStateSummary.summary(of: changed), field)
        }
    }

    func testSummaryChangesWhenEitherBlockBoundChangesAndOverlapRemains() throws {
        let donor = FixtureState(id: "bounds", screen: .timeline, timeline: try FixtureWire.timeline(segments: [
            .init(from: 535, to: 605), .init(agent: "codex", from: 550, to: 590)
        ]), expected: "")
        let donorBlock = try XCTUnwrap(donor.timeline?.segments.first)
        for id in ["timeline-focus", "timeline-day"] {
            let original = try state(id)
            for (key, value) in [("from", donorBlock.from), ("to", donorBlock.to)] {
                var changed = original
                changed.timeline = try replacing(XCTUnwrap(original.timeline)) { object in
                    var segments = try XCTUnwrap(object["segments"] as? [FixtureWire.Object])
                    segments[0][key] = value
                    object["segments"] = segments
                }
                let drawing = TimelineDrawing(try XCTUnwrap(changed.timeline), calendar: changed.calendar)
                XCTAssertEqual(drawing.lanes.first?.tracks, 2)
                XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed), "\(id): \(key)")
            }
        }
    }

    func testSummaryChangesWhenOnlyAnErrorTextIsReplaced() throws {
        let original = try state("project-plan-error")
        let donor = FixtureState(id: "error", screen: .project,
            detail: try FixtureWire.detail(units: [FixtureWire.unit(error: "Verify the local cache")]), expected: "")
        let error = try XCTUnwrap(donor.detail?.plan.units.first?.error)
        var changed = original
        changed.detail = try replacing(XCTUnwrap(original.detail)) { object in
            var plan = try XCTUnwrap(object["plan"] as? FixtureWire.Object)
            var units = try XCTUnwrap(plan["units"] as? [FixtureWire.Object])
            units[0]["error"] = error
            plan["units"] = units
            object["plan"] = plan
        }
        XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed))
    }

    func testSummaryKeepsUnitOrderIndependentOfTheFocusRule() throws {
        let original = try state("project-full")
        var changed = original
        changed.detail = try replacing(XCTUnwrap(original.detail)) { object in
            var plan = try XCTUnwrap(object["plan"] as? FixtureWire.Object)
            let units = try XCTUnwrap(plan["units"] as? [FixtureWire.Object])
            plan["units"] = Array(units.reversed())
            object["plan"] = plan
        }
        XCTAssertEqual(ProjectPresentation.focusUnit(original.detail?.plan.units ?? []),
                       ProjectPresentation.focusUnit(changed.detail?.plan.units ?? []))
        XCTAssertNotEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed))
    }

    func testFrameFailureKeepsTheNavigationEntries() throws {
        let running = FixtureStateSummary.summary(of: try state("frame-running"))
        let failed = FixtureStateSummary.summary(of: try state("frame-off"))
        XCTAssertEqual(failed, running.replacingOccurrences(of: "daemon=daemon running", with: "daemon=local API is off"))
    }

    func testHistorySummaryFollowsDisplayedOrderInsteadOfPayloadOrder() throws {
        let original = try state("project-history-cut")
        var changed = original
        changed.detail = try replacing(XCTUnwrap(original.detail)) { object in
            let history = try XCTUnwrap(object["history"] as? [FixtureWire.Object])
            object["history"] = Array(history.reversed())
        }
        XCTAssertEqual(FixtureStateSummary.summary(of: original), FixtureStateSummary.summary(of: changed))
        changed.detail = try FixtureWire.detail(history: [
            ["id": "decision-2", "title": "Review the guide", "at": 1],
            ["id": "decision-1", "title": "Keep the local cache", "at": 1]
        ])
        XCTAssertTrue(FixtureStateSummary.summary(of: changed).contains(
            "1970-01-01{not-served; history=[decision-1:Keep the local cache | decision-2:Review the guide]}"))
    }

    func testProjectSummaryNamesFocusCurrentTaskAndFoldedTasks() throws {
        let original = try state("project-full")
        let summary = FixtureStateSummary.summary(of: original)
        XCTAssertTrue(summary.contains("focus=[unit-2; scale=Proposed+>Planned+>Building*>Review>Done"))
        XCTAssertTrue(summary.contains("folded=1 completed task folded[T01:Draft the guide]"))
        var changed = original
        changed.detail = try replacing(XCTUnwrap(original.detail)) { object in
            var plan = try XCTUnwrap(object["plan"] as? FixtureWire.Object)
            var units = try XCTUnwrap(plan["units"] as? [FixtureWire.Object])
            units[1]["current_task"] = ["id": "T02", "title": "Verify the result", "source": "first_open"]
            plan["units"] = units
            object["plan"] = plan
        }
        XCTAssertNotEqual(summary, FixtureStateSummary.summary(of: changed))
    }

    func testProjectSummaryNamesEveryNewFieldAndItsRecordedAbsence() throws {
        let original = try state("project-enriched")
        let summary = FixtureStateSummary.summary(of: original)
        for field in ["closed", "closed_at", "result", "last_record_at", "kind"] {
            var changed = original
            changed.detail = try replacing(XCTUnwrap(original.detail)) { object in
                if field == "last_record_at" {
                    var sessions = try XCTUnwrap(object["sessions"] as? [FixtureWire.Object])
                    sessions[0][field] = NSNull()
                    object["sessions"] = sessions
                } else if field == "kind" {
                    var history = try XCTUnwrap(object["history"] as? [FixtureWire.Object])
                    history[0][field] = NSNull()
                    object["history"] = history
                } else {
                    var plan = try XCTUnwrap(object["plan"] as? FixtureWire.Object)
                    var units = try XCTUnwrap(plan["units"] as? [FixtureWire.Object])
                    if field == "result" {
                        var gate = try XCTUnwrap(units[0]["gate"] as? FixtureWire.Object)
                        gate[field] = NSNull()
                        units[0]["gate"] = gate
                    } else { units[1][field] = NSNull() }
                    plan["units"] = units
                    object["plan"] = plan
                }
            }
            XCTAssertNotEqual(summary, FixtureStateSummary.summary(of: changed), field)
        }
    }
}
