import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class BoardDecisionTests: XCTestCase {
    private func footer(open: Any? = nil, waiting: Any? = nil, closed: Any? = nil) throws -> String? {
        var row = FixtureWire.row(0)
        row["open_units"] = open; row["waiting_on_owner"] = waiting; row["last_closed"] = closed
        return ProjectPresentation.boardFooter(try FixtureWire.decode(row, as: ProjectBoardRow.self),
                                                calendar: FixtureWire.calendar())?.text
    }

    func testOneFooterFactWinsOverEveryRecordedAbsence() throws {
        let closed: FixtureWire.Object = ["title": "Draft the guide", "closed_at": NSNull()]
        let absences: [Any?] = [nil, NSNull(), [] as [String]]
        for waiting in absences {
            for open in [nil, NSNull(), 0, 1] as [Any?] {
                XCTAssertEqual(try footer(open: open, waiting: waiting, closed: closed), "Draft the guide · Closed")
            }
            for closed in [nil, NSNull()] as [Any?] {
                XCTAssertEqual(try footer(open: 1, waiting: waiting, closed: closed), "1 open unit")
                XCTAssertEqual(try footer(open: 0, waiting: waiting, closed: closed), "0 open units")
            }
        }
        for open in [nil, NSNull(), 2] as [Any?] {
            for closure in [nil, NSNull(), closed] as [Any?] {
                XCTAssertEqual(try footer(open: open, waiting: ["Review the guide"], closed: closure),
                               "Waiting for you: Review the guide")
                XCTAssertEqual(try footer(open: open, waiting: ["Review the guide", "Verify the result"], closed: closure),
                               "2 items wait for you: Review the guide; Verify the result")
            }
        }
    }

    func testUnservedFooterIsOmittedAndAbsenceUsesTheSamePriority() throws {
        XCTAssertNil(try footer())
        XCTAssertEqual(try footer(open: NSNull()), "Open units not recorded")
        XCTAssertEqual(try footer(closed: NSNull()), "No closed unit recorded")
        XCTAssertEqual(try footer(waiting: NSNull()), "Nothing recorded as waiting for you")
        XCTAssertEqual(try footer(waiting: []), "Nothing recorded as waiting for you")
        XCTAssertEqual(try footer(open: NSNull(), closed: NSNull()), "No closed unit recorded")
        XCTAssertEqual(try footer(open: NSNull(), waiting: [], closed: NSNull()), "Nothing recorded as waiting for you")
    }

    func testFunnelStatusWordsDoNotRewriteOwnerText() throws {
        var object = FixtureWire.funnelRow(0)
        object["plan"] = FixtureWire.planLine(unit: "Review_the_guide", task: "Check_the_local_cache")
        object["plan_counts"] = ["waiting_on_owner": 1, "in_review": 2, "contacted": 0]
        object["waiting_on_owner"] = ["Verify_the_result"]
        let row = try FixtureWire.decode(object, as: ProjectBoardRow.self)
        let plan = ProjectPresentation.boardPlan(row)
        XCTAssertEqual(plan.funnel?.counts, "contacted 0 · in review 2 · waiting for you 1")
        XCTAssertEqual(plan.unit, "Review_the_guide")
        XCTAssertEqual(plan.task, "Check_the_local_cache")
        XCTAssertNil(plan.progress)
        XCTAssertEqual(ProjectPresentation.boardFooter(row, calendar: FixtureWire.calendar())?.text,
                       "Waiting for you: Verify_the_result")
    }
}
