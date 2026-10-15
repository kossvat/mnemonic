// These fragments are literal expectations, independent of fixture inputs and presentation code.
enum FixtureProjectExpectations {
    private static let times = "You ~10m, Agents 20m"
    private static let entry = "decision-1:Keep the local cache 1"
    private static let focus = "unit-1; scale=Proposed+>Planned+>Building*>Review>Done; current=T02 Check the local cache"
    private static let tasks = "1 complete · 1 remaining; folded=1 completed task folded[T01:Draft the guide]; current=T02:Check the local cache; next=none; queue=0 queued; empty=none"
    private static let claude = "Claude Code:Running; elapsed=1h; record=not-served; driver=Owner-led session"
    private static let codex = "Codex:Running; elapsed=1h; record=not-served; driver=Owner-led session"

    private static func row(_ id: String = "unit-1", title: String = "Review the guide",
                            progress: String = "50%, 1/2", stage: String = "Building",
                            checkouts: String = "1 other checkout differs", flags: String = "[]",
                            closure: String = "not-shown", error: String = "none", provider: String = "checkbox") -> String {
        "\(id):\(title){progress=\(progress); details-stage=\(stage); details-checkouts=\(checkouts); "
            + "flags=\(flags); closure=\(closure); error=\(error); "
            + "counts=[]; weekly=not-shown; waiting=[]; \(provider) · Read 30 Sep 2026, 11:00}"
    }

    private static func gate(_ status: String = "Passed", day: String = "Last run 30 Sep 2026",
                             result: String = "not-served", movement: String = "0 commits since gate",
                             checkout: String = "No uncommitted changes",
                             failures: String = "none") -> String {
        "\(status); day=\(day); result=\(result); movement=\(movement); checkout=\(checkout); failures=\(failures)"
    }

    private static func days(_ time: String = FixtureProjectExpectations.times,
                             latest: String = FixtureProjectExpectations.entry, previous: String = "") -> String {
        "[2026-09-30{\(time); history=[\(latest)]} | 2026-09-29{\(time); history=[\(previous)]} | "
            + "2026-09-28{\(time); history=[]} | 2026-09-27{\(time); history=[]} | 2026-09-26{\(time); history=[]} | "
            + "2026-09-25{\(time); history=[]} | 2026-09-24{\(time); history=[]} | 2026-09-23{\(time); history=[]} | "
            + "2026-09-22{\(time); history=[]} | 2026-09-21{\(time); history=[]} | 2026-09-20{\(time); history=[]} | "
            + "2026-09-19{\(time); history=[]} | 2026-09-18{\(time); history=[]} | 2026-09-17{\(time); history=[]}]"
    }

    private static func page(rows: [String] = [FixtureProjectExpectations.row()], focus: String = FixtureProjectExpectations.focus,
                             state: String = "Live", today: String = FixtureProjectExpectations.times, live: String = "1 live session",
                             registry: String = "not-shown", heading: String = "Units", open: String = "not-served",
                             closed: String = "not-shown", tasks: String = FixtureProjectExpectations.tasks,
                             gate: String = FixtureProjectExpectations.gate(), sessions: [String] = [FixtureProjectExpectations.claude],
                             sessionsMessage: String = "none", days: String = FixtureProjectExpectations.days(),
                             historyMessage: String = "none", planMessage: String = "none") -> String {
        "project=harbor:Harbor state=\(state) today=[\(today)] live=\(live) registry=\(registry) focus=[\(focus)] "
            + "units=\(heading); open=\(open) rows=[\(rows.joined(separator: " | "))] unclassified=[] last-closed=[\(closed)] "
            + "tasks=[\(tasks)] gate=[\(gate)] sessions=[\(sessions.joined(separator: " | "))] sessions-message=\(sessionsMessage) "
            + "recent-columns=3 history-lines=2 days=\(days) time-message=none history-message=\(historyMessage) plan-message=\(planMessage) "
            + "terms=[cache · 1 | guide · 1] terms-message=none failure=none"
    }

    private static let unavailable = "project=not-loaded plan=Plan unavailable tasks=Tasks unavailable gate=Gate unavailable sessions=Live sessions unavailable recent=Recent work unavailable history=History unavailable failure="

    static let byID: [String: String] = [
        "project-full": page(rows: [row(title: "Draft the guide", progress: "100%, 2/2", stage: "Done"), row("unit-2")],
            focus: "unit-2; scale=Proposed+>Planned+>Building*>Review>Done; current=T02 Check the local cache",
            live: "2 live sessions", sessions: [claude, codex]),
        "project-no-plan": page(rows: [], focus: "none", tasks: "No task rows recorded", gate: "No gate recorded", planMessage: "no plan file"),
        "project-no-session": page(state: "Active", live: "0 live sessions", sessions: [], sessionsMessage: "No live sessions"),
        "project-gate-stale": page(gate: gate("Stale")),
        "project-gate-failed": page(gate: gate("Failed", failures: "Failed runs: 1")),
        "project-gate-none": page(gate: gate("No gate recorded", day: "not-shown")),
        "project-stages": page(rows: [
            row(stage: "Proposed"), row("unit-2", stage: "Planned"), row("unit-3"),
            row("unit-4", stage: "Review", flags: "[Stale]"), row("unit-5", stage: "Blocked", flags: "[Blocked]"),
            row("unit-6", stage: "Done")],
            focus: "unit-3; scale=Proposed+>Planned+>Building*>Review>Done; current=T02 Check the local cache"),
        "project-many-units": page(rows: [
            row(title: "Review the guide 1"), row("unit-2", title: "Review the guide 2"),
            row("unit-3", title: "Review the guide 3"), row("unit-4", title: "Review the guide 4"),
            row("unit-5", title: "Review the guide 5"), row("unit-6", title: "Review the guide 6"),
            row("unit-7", title: "Review the guide 7"), row("unit-8", title: "Review the guide 8"),
            row("unit-9", title: "Review the guide 9"), row("unit-10", title: "Review the guide 10"),
            row("unit-11", title: "Review the guide 11"), row("unit-12", title: "Review the guide 12")]),
        "project-time-unavailable": page(today: "You Unavailable, Agents 20m", days: days("You Unavailable, Agents 20m")),
        "project-time-zero": page(today: "You ~0m, Agents 0m", days: days("You ~0m, Agents 0m")),
        "project-history-cut": page(days: days(
            latest: "decision-1:Keep the local cache 1 | decision-2:Keep the local cache 2 | decision-3:Keep the local cache 3 | decision-4:Keep the local cache 4 | decision-5:Keep the local cache 5 | decision-6:Keep the local cache 6",
            previous: "decision-7:Keep the local cache 7 | decision-8:Keep the local cache 8 | decision-9:Keep the local cache 9 | decision-10:Keep the local cache 10")),
        "project-history-empty": page(days: days(latest: ""), historyMessage: "The returned history holds no decision or correction."),
        "project-plan-error": page(rows: [row(error: "Plan read incomplete")]),
        "project-loading": "project=not-loaded plan=Loading plan tasks=Loading tasks gate=Loading gate sessions=Loading live sessions recent=Loading recent work history=Loading history failure=none",
        "project-off": unavailable + "Local API is off: Project data is unavailable while this profile's API is disabled.",
        "project-unreachable": unavailable + "Daemon unavailable: The local daemon did not answer. Available data may be out of date.",
        "project-unauthorised": unavailable + "Access unavailable: The daemon did not accept this profile's access token.",
        "project-earlier": unavailable + "Daemon update needed: This daemon does not provide the requested project data.",
        "project-undecodable": unavailable + "Project data unavailable: The daemon's answer could not be read. Available data may be out of date.",
        "project-enriched": page(rows: [row(progress: "25%, 1/4", checkouts: "2 other checkouts differ", provider: "registry")],
            live: "2 live sessions", registry: "registry 1 of 4 tasks", heading: "Open changes", open: "1",
            closed: row("unit-2", title: "Draft the guide", progress: "100%, 2/2", stage: "Review", closure: "Closed 28 Sep 2026"),
            tasks: "1 complete · 3 remaining; folded=1 completed task folded[T01:Draft the guide]; current=T02:Check the local cache; next=T03:Verify the result; queue=2 queued; empty=none",
            gate: gate("Stale", day: "Last run 29 Sep 2026", result: "Last run result: passed", movement: "3 commits since gate",
                       checkout: "Uncommitted changes"),
            sessions: ["Claude Code:Running; elapsed=1h; record=last record 1 min ago; driver=Owner-led session",
                       "Codex:Waiting for you; elapsed=1h; record=last record 10 min ago; driver=Automated session"],
            days: "[2026-09-30{You ~10m, Agents 20m; history=[decision-1:Decision:Keep the local cache 1]} | 2026-09-29{You ~10m, Agents 20m; history=[decision-2:Correction:Keep the local cache 2]} | 2026-09-28{You ~10m, Agents 20m; history=[]}]"),
        "project-corrections": page(days: days(latest: "decision-1:Correction:Keep the local cache 1",
                                               previous: "decision-2:Correction:Keep the local cache 2"))
    ]
}
