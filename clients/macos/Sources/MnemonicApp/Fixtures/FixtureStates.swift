import Foundation
import MnemonicShared

struct FixtureState {
    enum Screen: String { case frame, projects, project, timeline }
    let id: String
    let screen: Screen
    var board: ProjectBoard? = nil
    var live: LiveSnapshot? = nil
    var detail: ProjectDetail? = nil
    var timeline: ProjectTimeline? = nil
    var failure: LocalAPIFailure? = nil
    var loading = false
    var now = FixtureWire.now
    var timeZone = FixtureWire.utc
    var day = "2026-09-29"
    var height: CGFloat? = nil
    var until: String? = nil
    let expected: String
    var calendar: Calendar { FixtureWire.calendar(timeZone) }
}

enum FixtureStates {
    private static let frameGroups = "groups=[NOW:Projects,Timeline | MEMORY:Memories,Search,Entities,Memory Map | EXPLORE:Project Graph,Overview,Manage]"

    static func all() throws -> [FixtureState] {
        try frames() + projects() + projectPages() + timelines()
    }

    private static func frames() throws -> [FixtureState] {
        let board = try FixtureWire.board([FixtureWire.row(0)])
        let live = try FixtureWire.live(count: 1)
        return [FixtureState(id: "frame-running", screen: .frame, board: board, live: live,
                             expected: frameGroups + " daemon=daemon running")]
            + failures(.frame)
    }

    static func failures(_ screen: FixtureState.Screen, until: String? = nil) -> [FixtureState] {
        if screen == .frame {
            return [FixtureState(id: "frame-off", screen: .frame, failure: .off,
                                 expected: frameGroups + " daemon=local API is off"),
                    FixtureState(id: "frame-unreachable", screen: .frame, failure: .unreachable,
                                 expected: frameGroups + " daemon=daemon unavailable"),
                    FixtureState(id: "frame-unauthorised", screen: .frame, failure: .unauthorised,
                                 expected: frameGroups + " daemon=access unavailable"),
                    FixtureState(id: "frame-earlier", screen: .frame, failure: .earlierDaemon,
                                 expected: frameGroups + " daemon=daemon update needed"),
                    FixtureState(id: "frame-undecodable", screen: .frame, failure: .undecodable,
                                 expected: frameGroups + " daemon=answer unreadable")]
        }
        func expected(_ failure: String, _ words: String) -> String {
            let id = screen.rawValue + "-" + failure
            switch screen {
            case .projects: return FixtureBoardExpectations.byID[id]!
            case .project: return FixtureProjectExpectations.byID[id]!
            case .timeline: return FixtureTimelineExpectations.byID[id]!
            default: return words
            }
        }
        return [FixtureState(id: screen.rawValue + "-off", screen: screen, failure: .off, until: until,
                      expected: expected("off", "failure=Local API is off")),
         FixtureState(id: screen.rawValue + "-unreachable", screen: screen, failure: .unreachable, until: until,
                      expected: expected("unreachable", "failure=Daemon unavailable")),
         FixtureState(id: screen.rawValue + "-unauthorised", screen: screen, failure: .unauthorised, until: until,
                      expected: expected("unauthorised", "failure=Access unavailable")),
         FixtureState(id: screen.rawValue + "-earlier", screen: screen, failure: .earlierDaemon, until: until,
                      expected: expected("earlier", "failure=Daemon update needed")),
         FixtureState(id: screen.rawValue + "-undecodable", screen: screen, failure: .undecodable, until: until,
                      expected: expected("undecodable", "failure=Project data unavailable"))]
    }

    private static func projects() throws -> [FixtureState] {
        let fullRows = (0..<11).map { index in
            FixtureWire.row(index, ageDays: index < 9 ? index : 31 + index,
                            plan: index == 2 ? nil : FixtureWire.planLine())
        }
        let full = try FixtureWire.board(fullRows)
        let live = try FixtureWire.live(count: 3, agents: [0: ["claude", "codex"]], withoutPlan: [2])
        let none = try FixtureWire.live()
        var states = [
            FixtureState(id: "projects-full", screen: .projects, board: full, live: live, height: 1_600,
                         expected: FixtureBoardExpectations.byID["projects-full"]!),
            FixtureState(id: "projects-none-live", screen: .projects,
                         board: try FixtureWire.board((0..<3).map { index in FixtureWire.row(index, ageDays: index) }), live: none,
                         expected: FixtureBoardExpectations.byID["projects-none-live"]!),
            FixtureState(id: "projects-empty", screen: .projects, board: try FixtureWire.board(), live: none,
                         expected: FixtureBoardExpectations.byID["projects-empty"]!),
            FixtureState(id: "projects-loading", screen: .projects, loading: true, expected: FixtureBoardExpectations.byID["projects-loading"]!)
        ]
        states += failures(.projects)
        let longTask = Array(repeating: "Check the local cache and verify the response", count: 5).joined(separator: " ")
        let longPlan = FixtureWire.planLine(task: longTask)
        let longRows = (0..<3).map { index in FixtureWire.row(index, long: true, plan: longPlan) }
        let manyRows = (0..<49).map { index in FixtureWire.row(index, ageDays: index % 30) }
        let noTotal = FixtureWire.planLine(done: 0, total: 0)
        states += [
            FixtureState(id: "projects-long", screen: .projects, board: try FixtureWire.board(longRows),
                         live: try FixtureWire.live(count: 3, plans: [0: longPlan, 1: longPlan, 2: longPlan], long: true),
                         height: 1_200,
                         expected: FixtureBoardExpectations.byID["projects-long"]!),
            FixtureState(id: "projects-many", screen: .projects, board: try FixtureWire.board(manyRows),
                         live: try FixtureWire.live(count: 9), height: 5_200,
                         expected: FixtureBoardExpectations.byID["projects-many"]!),
            FixtureState(id: "projects-cut", screen: .projects,
                         board: try FixtureWire.board([FixtureWire.row(0)]),
                         live: try FixtureWire.live(count: 1, agents: [0: ["claude", "codex", "claude", "codex"]],
                                                    moreProjects: 2, moreSessions: [0: 3],
                                                    unknown: FixtureWire.sessions(["claude"]), moreUnknown: 2),
                         expected: FixtureBoardExpectations.byID["projects-cut"]!),
            FixtureState(id: "projects-no-total", screen: .projects,
                         board: try FixtureWire.board([FixtureWire.row(0, plan: noTotal)]),
                         live: try FixtureWire.live(count: 1, plans: [0: noTotal]),
                         expected: FixtureBoardExpectations.byID["projects-no-total"]!),
            FixtureState(id: "projects-plan-unreadable", screen: .projects,
                         board: try FixtureWire.board([FixtureWire.row(0, plan: nil, message: "Plan read incomplete")]), live: none,
                         expected: FixtureBoardExpectations.byID["projects-plan-unreadable"]!),
            FixtureState(id: "projects-excluded", screen: .projects,
                         board: try FixtureWire.board([FixtureWire.row(0, state: "archived"),
                             FixtureWire.row(1, state: "ignored"), FixtureWire.row(2, mergedInto: FixtureWire.key(0))]), live: none,
                         expected: FixtureBoardExpectations.byID["projects-excluded"]!),
            FixtureState(id: "projects-funnel", screen: .projects, board: try FixtureWire.funnelBoard(),
                         live: try FixtureWire.live(count: 3, plans: [1: noTotal]),
                         expected: FixtureBoardExpectations.byID["projects-funnel"]!),
            FixtureState(id: "projects-enriched", screen: .projects, board: try FixtureWire.enrichedBoard(),
                         live: try FixtureWire.live(count: 3, plans: [1: noTotal]),
                         expected: FixtureBoardExpectations.byID["projects-enriched"]!)
        ]
        return states
    }

    private static func projectPages() throws -> [FixtureState] {
        let finished = FixtureWire.unit(1, title: "Draft the guide", done: 2, stage: "done", current: false,
            tasks: [FixtureWire.task(1, title: "Draft the guide", status: "done"),
                    FixtureWire.task(2, title: "Verify the result", status: "done")])
        let full = try FixtureWire.detail(units: [finished, FixtureWire.unit(2)], agents: ["claude", "codex"])
        let stages = ["proposed", "planned", "building", "review", "blocked", "done"].enumerated().map { index, stage in
            FixtureWire.unit(index + 1, stage: stage, stale: stage == "review", current: stage == "building")
        }
        var states = [
            FixtureState(id: "project-full", screen: .project, detail: full, height: 2_400,
                         expected: FixtureProjectExpectations.byID["project-full"]!),
            FixtureState(id: "project-no-plan", screen: .project,
                         detail: try FixtureWire.detail(units: [], message: "no plan file"), height: 1_800,
                         expected: FixtureProjectExpectations.byID["project-no-plan"]!),
            FixtureState(id: "project-no-session", screen: .project, detail: try FixtureWire.detail(agents: []), height: 2_000,
                         expected: FixtureProjectExpectations.byID["project-no-session"]!),
            FixtureState(id: "project-gate-stale", screen: .project,
                         detail: try FixtureWire.detail(units: [FixtureWire.unit(gate: "stale")]), height: 2_000,
                         expected: FixtureProjectExpectations.byID["project-gate-stale"]!),
            FixtureState(id: "project-gate-failed", screen: .project,
                         detail: try FixtureWire.detail(units: [FixtureWire.unit(gate: "failed")]), height: 2_000,
                         expected: FixtureProjectExpectations.byID["project-gate-failed"]!),
            FixtureState(id: "project-gate-none", screen: .project,
                         detail: try FixtureWire.detail(units: [FixtureWire.unit(gate: "none")]), height: 2_000,
                         expected: FixtureProjectExpectations.byID["project-gate-none"]!),
            FixtureState(id: "project-stages", screen: .project, detail: try FixtureWire.detail(units: stages),
                         height: 4_200, expected: FixtureProjectExpectations.byID["project-stages"]!),
            FixtureState(id: "project-many-units", screen: .project,
                         detail: try FixtureWire.detail(units: (1...12).map { number in FixtureWire.unit(number, title: "Review the guide \(number)") }),
                         height: 6_600, expected: FixtureProjectExpectations.byID["project-many-units"]!),
            FixtureState(id: "project-time-unavailable", screen: .project,
                         detail: try FixtureWire.detail(time: FixtureWire.days(owner: nil)), height: 2_000,
                         expected: FixtureProjectExpectations.byID["project-time-unavailable"]!),
            FixtureState(id: "project-time-zero", screen: .project,
                         detail: try FixtureWire.detail(time: FixtureWire.days(owner: 0, agent: 0)), height: 2_000,
                         expected: FixtureProjectExpectations.byID["project-time-zero"]!),
            FixtureState(id: "project-history-cut", screen: .project,
                         detail: try FixtureWire.detail(history: FixtureWire.history(count: 10, spacing: 7_200)),
                         height: 2_800, expected: FixtureProjectExpectations.byID["project-history-cut"]!),
            FixtureState(id: "project-history-empty", screen: .project, detail: try FixtureWire.detail(history: []),
                         height: 2_000, expected: FixtureProjectExpectations.byID["project-history-empty"]!),
            FixtureState(id: "project-plan-error", screen: .project,
                         detail: try FixtureWire.detail(units: [FixtureWire.unit(error: "Plan read incomplete")]), height: 2_000,
                         expected: FixtureProjectExpectations.byID["project-plan-error"]!),
            FixtureState(id: "project-loading", screen: .project, loading: true,
                         expected: FixtureProjectExpectations.byID["project-loading"]!)
        ]
        states += failures(.project)
        states += [
            FixtureState(id: "project-enriched", screen: .project, detail: try FixtureWire.projectEnriched(), height: 1_200,
                         expected: FixtureProjectExpectations.byID["project-enriched"]!),
            FixtureState(id: "project-corrections", screen: .project, detail: try FixtureWire.projectCorrections(), height: 2_000,
                         expected: FixtureProjectExpectations.byID["project-corrections"]!)
        ]
        return states
    }
}
