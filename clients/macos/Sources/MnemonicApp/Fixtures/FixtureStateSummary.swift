import Foundation
import MnemonicShared

enum FixtureStateSummary {
    static func summary(of state: FixtureState) -> String {
        if state.screen == .projects { return board(state) }
        if state.screen == .project { return FixtureProjectSummary.summary(state) }
        if state.screen == .timeline { return FixtureTimelinePresentation.summary(state) }
        if let failure = state.failure, state.screen != .frame {
            return "failure=" + ProjectPresentation.failure(failure).title
        }
        if state.loading, state.screen != .frame { return "loading" }
        switch state.screen {
        case .frame:
            let groups = SidebarModel.groups().map { group in
                group.label + ":" + group.entries.map(\.title).joined(separator: ",")
            }
            return "groups=\(list(groups)) daemon=\(SidebarModel.daemonText(state.failure))"
        case .projects: return board(state)
        case .project: return FixtureProjectSummary.summary(state)
        case .timeline: return FixtureTimelinePresentation.summary(state)
        }
    }

    private static func list(_ values: [String]) -> String { "[" + values.joined(separator: " | ") + "]" }

    private static func board(_ state: FixtureState) -> String {
        let display = ProjectPresentation.board(board: state.board, live: state.live, failure: state.failure,
                                                loading: state.loading, now: state.now, calendar: state.calendar)
        func times(_ value: ProjectPresentation.BoardTimes) -> String { "You \(value.owner), Agents \(value.agents)" }
        // The kind of a plan is not drawn, so the summary does not name it.
        func additions(_ funnel: ProjectPresentation.BoardFunnel?) -> String {
            let counts = funnel?.counts.map { value in "; counts=\(value)" } ?? ""
            let weekly = funnel?.weeklyCount.map { value in "; weekly-count=\(value)" } ?? ""
            return counts + weekly
        }
        let cards = display.cards.map { card in
            let plan = card.plan
            let progress = plan?.progress.map { value in "\(value.percent), \(value.tasks)" } ?? "absent"
            return "\(card.id):\(card.name){state=\(card.state); agents=\(card.agents.map { values in list(values.map(\.label)) } ?? "absent"); "
                + "today=\(times(card.today)); week=\(card.week.map(times) ?? "not-served"); "
                + "plan=\(plan?.unit ?? plan?.message ?? "not-served"); progress=\(progress); "
                + "stage=\(plan?.stage ?? "absent"); gate=\(plan?.gate ?? "absent"); task=\(plan?.task ?? "absent"); "
                + "sessions=\(card.sessions); longest=\(card.longest ?? "absent")"
                + additions(plan?.funnel)
                + (card.footer.map { footer in "; footer=\(footer.text)" } ?? "") + "}"
        }
        func row(_ value: ProjectPresentation.BoardRow) -> String {
            "\(value.id):\(value.name){plan=\(value.plan); activity=\(value.activity); week=\(times(value.week))"
                + additions(value.funnel) + "}"
        }
        let active = display.active.map { value in "\(value.projects); \(value.sessions)" } ?? "not-shown"
        let failure = display.failure.map { value in "\(value.title): \(value.detail)" } ?? "none"
        let week = display.week.map { value in "week=[\(times(value))] " } ?? ""
        return "today=[\(display.today.map(times) ?? "not-shown")] " + week + "active=[\(active)] "
            + "cards=\(list(cards)) rows=\(list(display.rows.map(row))) stale=\(list(display.stale.map(row))) "
            + "stale-line=[\(display.staleLine ?? "")] excluded=[\(display.excluded ?? "")] cut=\(display.cut ? "yes" : "no") "
            + "notices=\(list(display.notices)) live-message=\(display.liveMessage ?? "none") "
            + "rows-message=\(display.rowsMessage ?? "none") failure=\(failure)"
    }
}
