import AppKit
import Foundation
import ImageIO
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class FixtureStatesTests: XCTestCase {
    private let ids = [
        "frame-running",
        "frame-off", "frame-unreachable", "frame-unauthorised", "frame-earlier", "frame-undecodable",
        "projects-full", "projects-none-live", "projects-empty", "projects-loading",
        "projects-off", "projects-unreachable", "projects-unauthorised", "projects-earlier", "projects-undecodable",
        "projects-long", "projects-many", "projects-cut", "projects-no-total", "projects-plan-unreadable", "projects-excluded",
        "projects-funnel", "projects-enriched",
        "project-full", "project-no-plan", "project-no-session", "project-gate-stale", "project-gate-failed", "project-gate-none",
        "project-stages", "project-many-units", "project-time-unavailable", "project-time-zero",
        "project-history-cut", "project-history-empty", "project-plan-error", "project-loading",
        "project-off", "project-unreachable", "project-unauthorised", "project-earlier", "project-undecodable",
        "project-enriched", "project-corrections",
        "timeline-focus", "timeline-day", "timeline-one", "timeline-short", "timeline-edge", "timeline-loading",
        "timeline-off", "timeline-unreachable", "timeline-unauthorised", "timeline-earlier", "timeline-undecodable",
        "timeline-activity-only", "timeline-empty", "timeline-owner-unavailable", "timeline-23h", "timeline-25h",
        "timeline-many", "timeline-crowded", "timeline-registry"
    ]

    func testEveryTableIDDecodesOnceInTableOrderAndHasItsLiteralSummary() throws {
        let states = try FixtureStates.all()
        XCTAssertEqual(states.map(\.id), ids)
        XCTAssertEqual(Set(states.map(\.id)).count, states.count)
        for state in states {
            XCTAssertEqual(FixtureStateSummary.summary(of: state), state.expected, state.id)
        }
    }

    func testFrameStatesNeedNoLaterTask() throws {
        let states = try FixtureStates.all()
        XCTAssertTrue(states.filter { state in state.screen == .frame }.allSatisfy { state in state.until == nil })
    }

    func testBoardStatesNeedNoLaterTaskAndAllHaveLiteralExpectations() throws {
        let states = try FixtureStates.all().filter { state in state.screen == .projects }
        XCTAssertTrue(states.allSatisfy { state in state.until == nil })
        XCTAssertEqual(Set(states.map(\.id)), Set(FixtureBoardExpectations.byID.keys))
    }

    func testProjectStatesNameTheNewPageAndHaveLiteralExpectations() throws {
        let states = try FixtureStates.all().filter { state in state.screen == .project }
        XCTAssertTrue(states.allSatisfy { state in state.until == nil })
        XCTAssertEqual(Set(states.map(\.id)), Set(FixtureProjectExpectations.byID.keys))
        let enriched = try XCTUnwrap(states.first { state in state.id == "project-enriched" }?.detail)
        XCTAssertEqual(enriched.plan.units.map(\.closed), [.value(false), .value(true)])
        XCTAssertEqual(enriched.plan.units.first?.closedAt, .null)
        XCTAssertNotNil(enriched.plan.units.last?.closedAt.value)
        XCTAssertTrue(enriched.plan.units.allSatisfy { unit in unit.gate.result == .value(.passed) })
        XCTAssertEqual(enriched.plan.units.first?.gate.status, .stale)
        XCTAssertTrue(enriched.sessions.allSatisfy { session in session.lastRecordAt.value != nil })
        XCTAssertEqual(enriched.history.map(\.kind), [.value("decision"), .value("correction")])
        let corrections = try XCTUnwrap(states.first { state in state.id == "project-corrections" }?.detail)
        XCTAssertFalse(corrections.history.isEmpty)
        XCTAssertTrue(corrections.history.allSatisfy { entry in entry.kind == .value("correction") })
    }

    func testVocabularyCoversEveryIdentityNarrativeAndSchemaLabel() throws {
        for state in try FixtureStates.all() {
            func check(_ values: String?...) {
                for value in values.compactMap({ value in value }) {
                    XCTAssertTrue(FixtureVocabulary.contains(value), "\(state.id): \(value)")
                }
            }
            func session(_ value: LiveSession) {
                check(value.agent, value.driver, value.state.rawValue)
            }
            for row in state.board?.projects ?? [] {
                check(row.key, row.name, row.mergedInto, row.plan?.unit, row.plan?.task, row.planMessage)
                check(row.state.rawValue, row.plan?.stage, row.plan?.gate)
                check(row.planProvider.value?.rawValue, row.lastClosed.value?.title)
                for status in row.planCounts.value?.keys.sorted() ?? [] { check(status) }
                for text in row.waitingOnOwner.value ?? [] { check(text) }
            }
            for project in state.live?.projects ?? [] {
                check(project.key, project.name, project.plan?.unit, project.plan?.task)
                check(project.state.rawValue, project.plan?.stage, project.plan?.gate, project.today.confidence)
                for value in project.sessions { session(value) }
            }
            for value in state.live?.unknown.sessions ?? [] { session(value) }
            if let detail = state.detail {
                check(detail.key, detail.name, detail.plan.message, detail.state.rawValue)
                for key in detail.gateDetail?.keys.sorted() ?? [] { check(key) }
                for value in detail.sessions { session(value) }
                for day in detail.time { check(day.confidence) }
                for unit in detail.plan.units {
                    check(unit.id, unit.title, unit.error, unit.currentTask?.id, unit.currentTask?.title)
                    check(unit.stage.rawValue, unit.provider.rawValue, unit.gate.status.rawValue,
                          unit.gate.result.value?.rawValue, unit.currentTask?.source)
                    for text in unit.waitingOnOwner { check(text) }
                    for status in unit.counts.keys { check(status) }
                    for task in unit.tasks { check(task.id, task.title, task.owner, task.status) }
                }
                for entry in detail.history { check(entry.id, entry.title, entry.kind.value) }
                for term in detail.topTerms ?? [] { check(term.name) }
            }
            if let timeline = state.timeline {
                for project in timeline.projects { check(project.key, project.name) }
                for segment in timeline.segments {
                    check(segment.id, segment.stream, segment.project, segment.agent, segment.driver)
                }
                for range in timeline.owner {
                    check(range.id)
                    for assignment in range.assignments { check(assignment.project, assignment.confidence) }
                }
            }
        }
        for value in ["Harbor 42", "Review the local cache", "T01", "T100", "decision-a", "beacon-2",
                      "segment-1", "owner-1", "fixture-12", "unit-2", "decision-10", "no plan file"] {
            XCTAssertTrue(FixtureVocabulary.contains(value), value)
        }
        for value in ["", "unlistedword", "Harbor/cache", "Harbor\nBeacon", "T1", "decision-xyz", "Harbor🙂",
                      "segment-x", "owner-0", "fixture-", "unit-0", "segment-1-extra", "no file cache",
                      "segment-1\n", "T01\n", "decision-a\n"] {
            XCTAssertFalse(FixtureVocabulary.contains(value), value)
        }
    }

    func testFocusAgreesWithTheBoardAndUsesServedOrderBeforeFallback() throws {
        let states = try FixtureStates.all()
        let detail = try XCTUnwrap(states.first { candidate in candidate.id == "project-full" }?.detail)
        let board = try XCTUnwrap(states.first { candidate in candidate.id == "projects-full" }?.board)
        let row = try XCTUnwrap(board.projects.first { candidate in candidate.key == detail.key })
        XCTAssertNil(detail.plan.units.first?.currentTask)
        XCTAssertEqual(ProjectPresentation.focusUnit(detail.plan.units)?.title, row.plan?.unit)
        let first = try XCTUnwrap(detail.plan.units.first)
        let candidates = try FixtureWire.detail(units: [
            FixtureWire.unit(2, title: "Review the guide"), FixtureWire.unit(3, title: "Verify the result")
        ]).plan.units
        XCTAssertEqual(ProjectPresentation.focusUnit(candidates), candidates.first)
        XCTAssertEqual(ProjectPresentation.focusUnit(Array(candidates.reversed())), candidates.last)
        XCTAssertEqual(ProjectPresentation.focusUnit([first, first]), first)
        XCTAssertNil(ProjectPresentation.focusUnit([]))
    }

    func testNewBoardStatesServeFunnelAndEnrichedValuesWithoutChangingEarlierInputs() throws {
        let states = try FixtureStates.all()
        let funnel = try XCTUnwrap(states.first { state in state.id == "projects-funnel" })
        let display = ProjectPresentation.board(board: funnel.board, live: funnel.live, failure: nil,
                                                loading: false, now: funnel.now, calendar: funnel.calendar)
        XCTAssertEqual(display.cards.map { card in card.plan?.progress?.percent }, ["50%", nil, "50%"])
        XCTAssertEqual(display.cards[1].plan?.funnel?.counts, "contacted 4 · waiting for you 2")
        XCTAssertEqual(display.cards[1].plan?.funnel?.weeklyCount, "3 this week")
        XCTAssertEqual(display.rows.first?.funnel, display.cards[1].plan?.funnel)
        let enriched = try XCTUnwrap(states.first { state in state.id == "projects-enriched" }?.board)
        XCTAssertNotNil(enriched.week.value)
        for row in enriched.projects {
            XCTAssertNotNil(row.gateAt.value)
            XCTAssertNotNil(row.planProvider.value)
            XCTAssertNotNil(row.planCounts.value)
            XCTAssertNotNil(row.planWeeklyCount.value)
            XCTAssertNotNil(row.openUnits.value)
            XCTAssertNotNil(row.waitingOnOwner.value)
            XCTAssertNotNil(row.lastClosed.value)
            XCTAssertNotNil(row.lastClosed.value?.closedAt.value)
        }
        for state in states where state.screen == .projects && state.id != "projects-funnel" && state.id != "projects-enriched" {
            guard let board = state.board else { continue }
            XCTAssertEqual(board.week, .missing, state.id)
            for row in board.projects {
                XCTAssertEqual(row.gateAt, .missing, state.id)
                XCTAssertEqual(row.planProvider, .missing, state.id)
                XCTAssertEqual(row.planCounts, .missing, state.id)
                XCTAssertEqual(row.planWeeklyCount, .missing, state.id)
                XCTAssertEqual(row.openUnits, .missing, state.id)
                XCTAssertEqual(row.waitingOnOwner, .missing, state.id)
                XCTAssertEqual(row.lastClosed, .missing, state.id)
            }
        }
    }

    func testServedDistinguishesOmittedNullEmptyAndNonemptyTopTerms() throws {
        let omitted = try FixtureWire.detail(terms: nil)
        var object = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(omitted)) as? FixtureWire.Object)
        object["top_terms"] = NSNull()
        let null = try FixtureWire.decode(object, as: ProjectDetail.self)
        let empty = try FixtureWire.detail(terms: [])
        let recorded = try FixtureWire.detail(terms: [FixtureWire.term("cache")])
        guard case .notServed = ProjectPresentation.served(omitted.topTermsPresence) else {
            return XCTFail("An omitted field must stay not served")
        }
        guard case .absent = ProjectPresentation.served(null.topTermsPresence) else {
            return XCTFail("A null field must stay a recorded absence")
        }
        guard case .absent = ProjectPresentation.served(empty.topTermsPresence) else {
            return XCTFail("An empty field must stay a recorded absence")
        }
        guard case .value(let terms) = ProjectPresentation.served(recorded.topTermsPresence) else {
            return XCTFail("A recorded field must retain its terms")
        }
        XCTAssertEqual(terms.map(\.name), ["cache"])
    }

    func testNamedTimelineInputsKeepWholeBoundsAndRealLocalDays() throws {
        let states = try FixtureStates.all()
        let short = try XCTUnwrap(states.first { candidate in candidate.id == "timeline-short" }?.timeline)
        XCTAssertEqual(short.segments.map { segment in (segment.from - short.from) / 60 }, [540, 550, 560, 580, 0])
        XCTAssertEqual(short.segments.map { segment in (segment.to - segment.from) / 60 }, [1, 1, 1, 0, 1])
        for (id, hours) in [("timeline-23h", 23.0), ("timeline-25h", 25.0)] {
            let state = try XCTUnwrap(states.first { candidate in candidate.id == id })
            let snapshot = try XCTUnwrap(state.timeline)
            XCTAssertEqual(state.timeZone.identifier, "America/New_York")
            XCTAssertEqual(TimelineDrawing(snapshot, calendar: state.calendar).duration, hours * 3_600)
        }
        let edge = try XCTUnwrap(states.first { candidate in candidate.id == "timeline-edge" }?.timeline)
        let segment = try XCTUnwrap(edge.segments.first)
        let selection = TimelineSelection.agent(day: edge.day, segment)
        XCTAssertEqual(selection.from, edge.from + 8 * 3_600)
        XCTAssertEqual(selection.to, edge.from + 10 * 3_600)
    }

    func testBoardCardRowAndCutInputsAreNotOnlyLabels() throws {
        let states = try FixtureStates.all()
        let full = try XCTUnwrap(states.first { candidate in candidate.id == "projects-full" })
        let groups = ProjectPresentation.groups(board: full.board, live: full.live, now: full.now)
        XCTAssertEqual(groups.live.count, 3)
        XCTAssertEqual(groups.recent.map(\.key), ["atlas", "juniper", "orchard", "lantern", "quarry", "meadow"])
        XCTAssertEqual(groups.stale.count, 2)
        XCTAssertEqual(groups.live.first?.project.sessions.map(\.agent), ["claude", "codex"])
        XCTAssertNil(groups.live.last?.project.plan)
        XCTAssertNil(groups.live.last?.row?.plan)
        let many = try XCTUnwrap(states.first { candidate in candidate.id == "projects-many" })
        let manyGroups = ProjectPresentation.groups(board: many.board, live: many.live, now: many.now)
        XCTAssertEqual(manyGroups.live.count, 9)
        XCTAssertEqual(manyGroups.recent.count, 40)
        let cut = try XCTUnwrap(states.first { candidate in candidate.id == "projects-cut" }?.live)
        XCTAssertGreaterThan(cut.moreProjects, 0)
        XCTAssertGreaterThan(cut.projects.first?.moreSessions ?? 0, 0)
        XCTAssertFalse(cut.unknown.sessions.isEmpty)
    }

    func testHistoryCutContainsTenReturnedTitlesOnExactlyTwoLocalDays() throws {
        let state = try XCTUnwrap(FixtureStates.all().first { candidate in candidate.id == "project-history-cut" })
        let detail = try XCTUnwrap(state.detail)
        let days = Set(detail.history.map { entry in
            TimelineDay.string(Date(timeIntervalSince1970: Double(entry.at)), calendar: state.calendar)
        })
        XCTAssertEqual(detail.history.count, 10)
        XCTAssertEqual(days, ["2026-09-29", "2026-09-30"])
    }

    func testBuilderSchemaDriftThrowsInsteadOfReturningAnUncheckedObject() {
        XCTAssertThrowsError(try FixtureWire.decode(["projects": [FixtureWire.row(0, state: "unserved")]], as: ProjectBoard.self))
        XCTAssertThrowsError(try FixtureWire.detail(units: [FixtureWire.unit(stage: "unserved")]))
    }

    @MainActor
    func testRenderWritesExactlyTwoPicturesPerStateAtDoubleWidth() throws {
        let output = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: output) }
        let states = try FixtureStates.all()
        try FixtureStateRender.draw(states, to: output)
        let expected = Set(ids.flatMap { id in ["\(id)-light.png", "\(id)-dark.png"] })
        XCTAssertEqual(Set(try FileManager.default.contentsOfDirectory(atPath: output.path)), expected)
        // The header of a picture names its size; the pixels are not read back.
        for state in states {
            for appearance in ["light", "dark"] {
                let url = output.appendingPathComponent("\(state.id)-\(appearance).png")
                let source = try XCTUnwrap(CGImageSourceCreateWithURL(url as CFURL, nil), url.lastPathComponent)
                let properties = try XCTUnwrap(CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any])
                XCTAssertEqual(properties[kCGImagePropertyPixelWidth] as? Int, 2_480, url.lastPathComponent)
                XCTAssertEqual(properties[kCGImagePropertyPixelHeight] as? Int, Int((state.height ?? 800) * 2),
                               url.lastPathComponent)
            }
        }
    }

    @MainActor
    func testRunRefusesTheUnisolatedProcessBeforeCreatingOutput() throws {
        XCTAssertFalse(MnemonicProfile.current.isolated)
        let output = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: output) }
        XCTAssertThrowsError(try FixtureStateRender.run(arguments: [
            "MnemonicApp", "--render-fixture-states", output.path, "--only", "projects-full"
        ])) { error in
            guard case AppFixturePreview.Failure.profile = error else {
                return XCTFail("Expected profile refusal, got \(error)")
            }
        }
        XCTAssertFalse(FileManager.default.fileExists(atPath: output.path))
    }
}
