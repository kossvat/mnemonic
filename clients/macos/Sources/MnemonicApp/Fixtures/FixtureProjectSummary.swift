import Foundation
import MnemonicShared

enum FixtureProjectSummary {
    private static func list(_ values: [String]) -> String { "[" + values.joined(separator: " | ") + "]" }
    private static func times(_ value: ProjectPresentation.BoardTimes) -> String {
        "You \(value.owner), Agents \(value.agents)"
    }

    static func summary(_ state: FixtureState) -> String {
        let page = ProjectPresentation.page(detail: state.detail, failure: state.failure, liveFailure: nil,
                                            loading: state.loading, now: state.now, calendar: state.calendar)
        let failure = page.failure.map { value in value.title + ": " + value.detail } ?? "none"
        guard let detail = state.detail else {
            return "project=not-loaded plan=\(page.planMessage ?? "none") tasks=\(page.tasksMessage ?? "none") "
                + "gate=\(page.gateMessage) sessions=\(page.sessionsMessage ?? "none") "
                + "recent=\(page.recent.timeMessage ?? "none") history=\(page.recent.historyMessage ?? "none") failure=\(failure)"
        }
        let focus = page.focus.map { value in
            let scale = value.stage.steps.map { step in
                step.label + (step.current ? "*" : step.complete ? "+" : "")
            }.joined(separator: ">")
            return "\(value.id); scale=\(scale); current=\(value.current ?? "none")"
        } ?? "none"
        let sessions = page.sessions.map { session in
            "\(session.agent):\(session.state); elapsed=\(session.elapsed); record=\(session.lastRecord ?? "not-served"); driver=\(session.driver)"
        }
        let days = page.recent.days.map { day in
            let entries = day.history.map { entry in
                "\(entry.id):\(entry.kind.map { kind in kind + ":" } ?? "")\(entry.title)"
            }
            return "\(day.id){\(day.times.map(times) ?? "not-served"); history=\(list(entries))}"
        }
        let terms = page.terms.map(list) ?? "not-served"
        let taskSummary = (page.focus?.tasks).map(tasks) ?? page.tasksMessage ?? "not-shown"
        return "project=\(detail.key):\(page.name) state=\(page.state ?? "none") "
            + "today=[\(page.today.map(times) ?? page.todayMessage ?? "not-shown")] "
            + "live=\(page.sessionCount ?? "not-shown") registry=\(page.registry ?? "not-shown") focus=[\(focus)] "
            + "units=\(page.units.heading); open=\(page.units.openCount.map(String.init) ?? "not-served") "
            + "rows=\(list(page.units.rows.map(unit))) unclassified=\(list(page.units.unclassified.map(unit))) "
            + "last-closed=[\(page.units.lastClosed.map(unit) ?? page.units.closedMessage ?? "not-shown")] "
            + "tasks=[\(taskSummary)] gate=[\(page.gate.map(gate) ?? page.gateMessage)] "
            + "sessions=\(list(sessions)) sessions-message=\(page.sessionsMessage ?? "none") "
            + "recent-columns=\(page.recent.columnCount) history-lines=\(page.recent.historyLineLimit) days=\(list(days)) "
            + "time-message=\(page.recent.timeMessage ?? "none") history-message=\(page.recent.historyMessage ?? "none") "
            + "plan-message=\(page.planMessage ?? "none") terms=\(terms) terms-message=\(page.termsMessage ?? "none") failure=\(failure)"
    }

    private static func unit(_ value: ProjectPresentation.PageUnit) -> String {
        let progress = value.progress.map { progress in "\(progress.percent), \(progress.compactTasks)" } ?? "not-shown"
        return "\(value.id):\(value.title){progress=\(progress); details-stage=\(value.stage.label); "
            + "details-checkouts=\(value.checkoutDifferences ?? "not-shown"); flags=\(list(value.stage.flags)); "
            + "closure=\(value.closure ?? "not-shown"); error=\(value.error ?? "none"); "
            + "counts=\(list(value.counts)); weekly=\(value.weekly ?? "not-shown"); waiting=\(list(value.waiting)); \(value.metadata)}"
    }

    private static func task(_ value: ProjectPresentation.PageTask) -> String {
        "\(value.id):\(value.title)" + (value.owner.map { owner in "; owner=\(owner)" } ?? "")
    }

    private static func tasks(_ value: ProjectPresentation.PageTasks) -> String {
        "\(value.summary); folded=\(value.foldedLabel)\(list(value.finished.map(task))); "
            + "current=\(value.current.map(task) ?? "none"); next=\(value.next.map(task) ?? "none"); "
            + "queue=\(value.queueLabel); empty=\(value.emptyMessage ?? "none")"
    }

    private static func gate(_ value: ProjectPresentation.PageGate) -> String {
        "\(value.status); day=\(value.day ?? "not-shown"); result=\(value.result ?? "not-served"); "
            + "movement=\(value.movement ?? "not-served"); checkout=\(value.checkout ?? "not-served"); "
            + "failures=\(value.failedRuns ?? "none")"
    }
}
