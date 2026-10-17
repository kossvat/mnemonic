import Foundation
import MnemonicShared

extension ProjectPresentation {
    struct BoardTimes: Equatable {
        let owner: String
        let agents: String
    }

    struct AgentCount: Identifiable, Equatable {
        let kind: String
        let count: Int
        var id: String { kind }
        var name: String { kind == "claude" ? "Claude Code" : kind == "codex" ? "Codex" : kind }
        var label: String { "\(name) \(count)" }
    }

    struct BoardProgress {
        let fraction: Double
        let percent: String
        let tasks: String
        let compactTasks: String
    }

    struct BoardPlan {
        let unit: String?
        let progress: BoardProgress?
        let stage: String?
        let gate: String?
        let task: String?
        let message: String?
        var provider: String? = nil
        var funnel: BoardFunnel? = nil
    }

    struct BoardCard: Identifiable {
        let id: String
        let name: String
        let state: String
        let agents: [AgentCount]?
        let today: BoardTimes
        let week: BoardTimes?
        let plan: BoardPlan?
        let sessions: String
        let longest: String?
        let footer: BoardFooter?
    }

    struct BoardRow: Identifiable {
        let id: String
        let name: String
        let plan: String
        let activity: String
        let week: BoardTimes
        let provider: String?
        let funnel: BoardFunnel?
    }

    struct ActiveNow {
        let projects: String
        let sessions: String
    }

    struct Board {
        let dateNote: String
        let today: BoardTimes?
        let week: BoardTimes?
        let active: ActiveNow?
        let cards: [BoardCard]
        let rows: [BoardRow]
        let stale: [BoardRow]
        let staleLine: String?
        let staleDetail: String?
        let inactiveLine: String?
        let excluded: String?
        let cut: Bool
        let notices: [String]
        let liveMessage: String?
        let rowsMessage: String?
        let failure: (title: String, detail: String)?
    }

    static func board(board: ProjectBoard?, live: LiveSnapshot?, failure: LocalAPIFailure?, loading: Bool,
                      now: Date, calendar: Calendar = .current) -> Board {
        let snapshot = failure == nil ? live : nil
        let groups = groups(board: board, live: snapshot, now: now)
        let cut = snapshot.map(snapshotCut) ?? false
        let cards = groups.live.map { card in boardCard(card, now: now, cut: cut, calendar: calendar) }
        let rows = groups.recent.map { row in boardRow(row, calendar: calendar) }
        let stale = groups.stale.map { row in boardRow(row, calendar: calendar) }
        let inactiveCount = rows.count
        var notices: [String] = []
        if cut { notices.append("Snapshot incomplete. Counts by agent and longest sessions are unavailable.") }
        if let snapshot {
            if snapshot.moreProjects > 0 {
                notices.append("\(snapshot.moreProjects) additional live projects are outside this snapshot.")
            }
            let unknown = snapshot.unknown.sessions.count + snapshot.unknown.moreSessions
            if unknown > 0 {
                notices.append("\(unknown) \(unknown == 1 ? "session has" : "sessions have") no recorded project.")
            }
        }
        let liveMessage: String?
        if failure != nil || !cards.isEmpty { liveMessage = nil }
        else if snapshot == nil { liveMessage = loading ? "Loading live projects." : "Live activity unavailable." }
        else if cut { liveMessage = "No live projects are included in this snapshot." }
        else if snapshot?.hasLiveSession == true { liveMessage = "No live projects are shown on this board." }
        else { liveMessage = "Nothing is in work now." }
        let rowsMessage: String?
        if board == nil {
            rowsMessage = failure != nil ? nil : loading ? "Loading projects." : "Project registry unavailable."
        } else if !rows.isEmpty || !stale.isEmpty { rowsMessage = nil }
        else if groups.excluded > 0 && cards.isEmpty {
            rowsMessage = "All registered projects are archived, ignored, or merged."
        } else { rowsMessage = cards.isEmpty ? "No projects registered." : "No other projects recorded." }
        return Board(
            dateNote: boardDate(now, calendar: calendar) + " · you: estimate when several projects are live · agents: exact",
            today: snapshot.map { value in headerTotals(value) },
            week: board.flatMap { value in boardHeaderWeek(value.week) },
            active: snapshot.map { value in activeNow(value) }, cards: cards, rows: rows, stale: stale,
            staleLine: stale.isEmpty ? nil : "\(stale.count) stale \(stale.count == 1 ? "project" : "projects")",
            staleDetail: stale.isEmpty ? nil : "Last recorded activity was more than 30 days ago.",
            inactiveLine: board == nil ? nil : "\(inactiveCount) \(inactiveCount == 1 ? "project" : "projects") · last activity",
            excluded: groups.excluded > 0 ? groups.excludedLabel : nil, cut: cut, notices: notices,
            liveMessage: liveMessage, rowsMessage: rowsMessage, failure: failure.map(Self.failure))
    }

    static func boardDate(_ now: Date, calendar: Calendar, format: String = "EEEE, d MMMM") -> String {
        FixedDateFormat.string(now, format: format, calendar: calendar)
    }

    static func headerTotals(_ snapshot: LiveSnapshot) -> BoardTimes {
        BoardTimes(owner: duration(Double(snapshot.today.ownerSeconds)),
                   agents: duration(Double(snapshot.today.agentSeconds)))
    }

    static func snapshotCut(_ snapshot: LiveSnapshot) -> Bool {
        snapshot.moreProjects > 0 || snapshot.unknown.moreSessions > 0
            || snapshot.projects.contains { project in project.moreSessions > 0 }
    }

    static func countsByKind(_ sessions: [LiveSession], cut: Bool) -> [AgentCount]? {
        guard !cut else { return nil }
        let counts = Dictionary(grouping: sessions, by: \.agent).map { kind, values in
            AgentCount(kind: kind, count: values.count)
        }
        func order(_ kind: String) -> Int { kind == "claude" ? 0 : kind == "codex" ? 1 : 2 }
        return counts.sorted { left, right in
            order(left.kind) == order(right.kind) ? left.kind < right.kind : order(left.kind) < order(right.kind)
        }
    }

    static func activeNow(_ snapshot: LiveSnapshot) -> ActiveNow {
        let count = snapshot.projects.count + snapshot.moreProjects
        let sessions = snapshot.projects.flatMap(\.sessions) + snapshot.unknown.sessions
        let counts = countsByKind(sessions, cut: snapshotCut(snapshot))
        let label = counts.map { values in
            values.isEmpty ? "0 sessions" : values.map { value in "\(value.count) \(value.name)" }.joined(separator: " · ")
        } ?? "Snapshot incomplete"
        return ActiveNow(projects: "\(count) \(count == 1 ? "project" : "projects")", sessions: label)
    }

    static func boardProgress(_ plan: LivePlan) -> BoardProgress? {
        guard plan.total > 0 else { return nil }
        let fraction = min(1, max(0, Double(plan.done) / Double(plan.total)))
        return BoardProgress(fraction: fraction, percent: "\(Int((fraction * 100).rounded()))%",
                             tasks: "\(plan.done) of \(plan.total) tasks", compactTasks: "\(plan.done)/\(plan.total)")
    }

    static func longestSession(_ sessions: [LiveSession], now: Date, cut: Bool) -> String? {
        guard !cut, let start = sessions.map(\.startedAt).min() else { return nil }
        return duration(max(0, now.timeIntervalSince1970 - Double(start)))
    }

    static func boardGate(_ gate: String?) -> String {
        switch gate {
        case "passed": return "Gate · Passed"
        case "failed": return "Gate · Failed"
        case "stale": return "Gate · Stale"
        case nil, "", "none": return "Gate · Not recorded"
        default: return "Gate · " + (gate ?? "")
        }
    }

    static func boardPlan(_ row: ProjectBoardRow, calendar: Calendar = .current) -> BoardPlan {
        guard let plan = row.plan else {
            return BoardPlan(unit: nil, progress: nil, stage: nil, gate: nil, task: nil,
                             message: row.planMessage.flatMap { message in message.isEmpty ? nil : message }
                                ?? "No plan recorded")
        }
        let provider = boardProvider(row.planProvider)
        let funnel = boardFunnel(row)
        let gateDate = boardGateDate(row.gateAt, calendar: calendar)
        let gate: String
        if row.gateAt == .null && (plan.gate == nil || plan.gate == "" || plan.gate == "none") {
            gate = "Gate · No run recorded"
        } else { gate = boardGate(plan.gate) + (gateDate.map { day in ", " + day } ?? "") }
        return BoardPlan(unit: plan.unit, progress: boardAllowsProgress(row.planProvider) ? boardProgress(plan) : nil,
                         stage: title(plan.stage), gate: gate,
                         task: plan.task, message: nil, provider: provider, funnel: funnel)
    }

    /// The plan line of the live snapshot, for a card whose board row has not arrived.
    static func boardPlan(line plan: LivePlan) -> BoardPlan {
        BoardPlan(unit: plan.unit, progress: boardProgress(plan), stage: title(plan.stage),
                  gate: boardGate(plan.gate), task: plan.task, message: nil)
    }

    static func boardWeek(_ week: ProjectWeek) -> BoardTimes {
        BoardTimes(owner: week.ownerSeconds == nil ? "Unavailable" : ownerTime(week.ownerSeconds, estimated: week.ownerEstimate),
                   agents: duration(Double(week.agentSeconds)))
    }

    static func boardCard(_ card: LiveCard, now: Date, cut: Bool, calendar: Calendar = .current) -> BoardCard {
        let project = card.project
        let count = project.sessions.count + project.moreSessions
        let longest = longestSession(project.sessions, now: now, cut: cut)
        let sessions = "\(count) \(count == 1 ? "session" : "sessions")"
            + (longest.map { value in " · longest \(value)" } ?? "")
        return BoardCard(id: project.key, name: project.name,
                         state: project.state == .running ? "Live" : title(project.state.rawValue),
                         agents: countsByKind(project.sessions, cut: cut),
                         today: BoardTimes(owner: ownerTime(Double(project.today.ownerSeconds), estimated: true),
                                           agents: duration(Double(project.today.agentSeconds))),
                         week: card.row.map { row in boardWeek(row.week) },
                         plan: card.row.map { row in boardPlan(row, calendar: calendar) }
                            ?? project.plan.map { line in boardPlan(line: line) },
                         sessions: sessions, longest: longest,
                         footer: card.row.flatMap { row in boardFooter(row, calendar: calendar) })
    }

    static func boardRow(_ row: ProjectBoardRow, calendar: Calendar) -> BoardRow {
        let display = boardPlan(row, calendar: calendar)
        let line: String
        if let plan = row.plan, !boardAllowsProgress(row.planProvider) {
            line = "\(plan.unit) · \(title(plan.stage))"
        } else { line = planLine(row.plan, message: display.message) }
        return BoardRow(id: row.key, name: row.name, plan: line,
                 activity: row.lastActiveAt.map { at in
                     boardDate(Date(timeIntervalSince1970: Double(at)), calendar: calendar, format: "d MMM")
                 } ?? "No activity recorded", week: boardWeek(row.week),
                 provider: display.provider, funnel: display.funnel)
    }

    struct BoardFunnel: Equatable {
        let counts: String?
        let weeklyCount: String?
    }

    struct BoardFooter: Equatable {
        let openUnits: String?
        let waiting: String?
        let lastClosed: String?
        let text: String
    }

    static func boardHeaderWeek(_ presence: FieldPresence<ProjectWeek>) -> BoardTimes? {
        switch presence {
        case .missing: return nil
        case .null: return BoardTimes(owner: "unavailable", agents: "unavailable")
        case .value(let week):
            return BoardTimes(owner: week.ownerSeconds == nil ? "unavailable"
                : ownerTime(week.ownerSeconds, estimated: week.ownerEstimate), agents: duration(Double(week.agentSeconds)))
        }
    }

    static func boardGateDate(_ presence: FieldPresence<Int>, calendar: Calendar) -> String? {
        switch presence {
        case .missing: return nil
        case .null: return "No gate run recorded"
        case .value(let at):
            return boardDate(Date(timeIntervalSince1970: Double(at)), calendar: calendar, format: "d MMM")
        }
    }

    static func boardProvider(_ presence: FieldPresence<ProjectPlanProvider>) -> String? {
        switch presence {
        case .missing: return nil
        case .null: return "Plan provider not recorded"
        case .value(let provider): return provider.rawValue
        }
    }

    static func boardAllowsProgress(_ presence: FieldPresence<ProjectPlanProvider>) -> Bool {
        switch presence {
        case .missing: return true
        case .null: return false
        case .value(let provider): return provider != .funnel
        }
    }

    static func boardFunnel(_ row: ProjectBoardRow) -> BoardFunnel? {
        guard row.planProvider == .value(.funnel) else { return nil }
        return BoardFunnel(counts: boardFunnelCounts(row.planCounts), weeklyCount: boardFunnelWeek(row.planWeeklyCount))
    }

    static func boardFunnelCounts(_ presence: FieldPresence<[String: Int]>) -> String? {
        switch presence {
        case .missing: return nil
        case .null: return "No status counts recorded"
        case .value(let counts):
            guard !counts.isEmpty else { return "No status counts recorded" }
            return counts.sorted { left, right in left.key < right.key }
                .map { status, count in
                    let label = status == "waiting_on_owner" ? "waiting for you" : status.replacingOccurrences(of: "_", with: " ")
                    return "\(label) \(count)"
                }.joined(separator: " · ")
        }
    }

    static func boardFunnelWeek(_ presence: FieldPresence<Int>) -> String? {
        switch presence {
        case .missing: return nil
        case .null: return "Weekly count not recorded"
        case .value(let count): return "\(count) this week"
        }
    }

    static func boardOpenUnits(_ presence: FieldPresence<Int>) -> String? {
        switch presence {
        case .missing: return nil
        case .null: return "Open units not recorded"
        case .value(let count): return "\(count) open \(count == 1 ? "unit" : "units")"
        }
    }

    static func boardWaiting(_ presence: FieldPresence<[String]>) -> String? {
        switch presence {
        case .missing: return nil
        case .null: return "Nothing recorded as waiting for you"
        case .value(let items):
            guard !items.isEmpty else { return "Nothing recorded as waiting for you" }
            let label = items.count == 1 ? "Waiting for you: " : "\(items.count) items wait for you: "
            return label + items.joined(separator: "; ")
        }
    }

    static func boardLastClosed(_ presence: FieldPresence<ProjectLastClosed>, calendar: Calendar) -> String? {
        let unit: ProjectLastClosed
        switch presence {
        case .missing: return nil
        case .null: return "No closed unit recorded"
        case .value(let value): unit = value
        }
        let day = unit.closedAt.value.map { at in
            " " + boardDate(Date(timeIntervalSince1970: Double(at)), calendar: calendar, format: "d MMM")
        } ?? ""
        return unit.title + " · Closed" + day
    }

    static func boardFooter(_ row: ProjectBoardRow, calendar: Calendar) -> BoardFooter? {
        let open = boardOpenUnits(row.openUnits), waiting = boardWaiting(row.waitingOnOwner)
        let closed = boardLastClosed(row.lastClosed, calendar: calendar)
        let fact: String?
        if let items = row.waitingOnOwner.value, !items.isEmpty { fact = waiting }
        else if row.lastClosed.value != nil { fact = closed }
        else if row.openUnits.value != nil { fact = open }
        else { fact = nil }
        guard let text = fact ?? waiting ?? closed ?? open else { return nil }
        return BoardFooter(openUnits: open, waiting: waiting, lastClosed: closed, text: text)
    }
}
