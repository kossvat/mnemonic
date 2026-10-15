import Foundation
import MnemonicShared

extension ProjectPresentation {
    struct Page {
        let name: String
        let state: String?
        let today: BoardTimes?
        let todayMessage: String?
        let sessionCount: String?
        let registry: String?
        let focus: PageUnit?
        let units: PageUnits
        let planMessage: String?
        let tasksMessage: String?
        let gate: PageGate?
        let gateMessage: String
        let sessions: [PageSession]
        let sessionsMessage: String?
        let recent: PageRecent
        let terms: [String]?
        let termsMessage: String?
        let failure: (title: String, detail: String)?
        let liveFailure: (title: String, detail: String)?
    }

    struct PageUnit: Identifiable {
        let id: String
        let title: String
        let progress: BoardProgress?
        let current: String?
        let stage: PageStage
        let tasks: PageTasks?
        let counts: [String]
        let weekly: String?
        let closure: String?
        let error: String?
        let waiting: [String]
        let checkoutDifferences: String?
        let metadata: String
    }

    struct PageUnits {
        let heading: String
        let openCount: Int?
        let rows: [PageUnit]
        let unclassified: [PageUnit]
        let lastClosed: PageUnit?
        let closedMessage: String?
    }

    struct PageStage {
        struct Step: Identifiable {
            let index: Int
            let label: String
            let current: Bool
            let complete: Bool
            var id: Int { index }
            var number: String { String(index + 1) }
            var caption: String { label + (current ? " · now" : "") }
        }
        let label: String
        let steps: [Step]
        let flags: [String]
        var accessibility: String { "Stage: " + label + (flags.contains("Stale") ? ", stale" : "") }
    }

    struct PageTask: Identifiable {
        let id: String
        let title: String
        let status: String
        let owner: String?
        var line: String { id + " " + title }
    }

    /// The word over the task in focus: in progress only when the plan says
    /// so; a task picked as the first open one is the current one.
    static func currentTaskLabel(_ task: PageTask) -> String {
        task.status == "in_progress" ? "IN PROGRESS" : "CURRENT"
    }

    struct PageTasks {
        let finished: [PageTask]
        let current: PageTask?
        let next: PageTask?
        let queued: [PageTask]
        let summary: String
        let foldedLabel: String
        let queueLabel: String
        let emptyMessage: String?
        var nextLabel: String? { next.map { task in task.id + " next · " + queueLabel } }
    }

    struct PageGate {
        let status: String
        let warning: Bool
        let day: String?
        let result: String?
        let movement: String?
        let checkout: String?
        let dirty: Bool
        let failedRuns: String?
    }

    static func page(detail: ProjectDetail?, failure: LocalAPIFailure?, liveFailure: LocalAPIFailure?,
                     loading: Bool, now: Date, calendar: Calendar = .current) -> Page {
        let rawUnits = detail?.plan.units ?? []
        let focus = focusUnit(rawUnits)
        let liveUnavailable = failure != nil || liveFailure != nil
        let sessions = liveUnavailable ? [] : detail?.sessions ?? []
        let today = detail?.time.first { day in day.day == TimelineDay.string(now, calendar: calendar) }
        let registry = rawUnits.first { unit in unit.provider == .registry && unit.total > 0 }
        let planMessage: String?
        if let detail {
            planMessage = detail.plan.message ?? (rawUnits.isEmpty ? "No plan recorded" : nil)
        } else { planMessage = loading ? "Loading plan" : "Plan unavailable" }
        let terms: [String]?
        let termsMessage: String?
        switch detail.map({ value in served(value.topTermsPresence) }) {
        case .some(.value(let values)):
            terms = values.map { term in "\(term.name) · \(term.memoryCount)" }; termsMessage = nil
        case .some(.absent): terms = []; termsMessage = "No top terms recorded"
        case .some(.notServed), .none: terms = nil; termsMessage = nil
        }
        return Page(
            name: detail?.name ?? "Project", state: detail.map { value in pageState(value.state, sessions: sessions) },
            today: today.map { day in pageTimes(day, estimated: detail?.ownerEstimate ?? false) },
            todayMessage: detail != nil && today == nil ? "No time recorded for today" : nil,
            sessionCount: detail != nil && !liveUnavailable
                ? "\(sessions.count) live \(sessions.count == 1 ? "session" : "sessions")" : nil,
            registry: registry.map { unit in "registry \(unit.done) of \(unit.total) tasks" },
            focus: focus.map { unit in pageUnit(unit, calendar: calendar) },
            units: pageUnits(rawUnits, calendar: calendar), planMessage: planMessage,
            tasksMessage: focus == nil
                ? (detail == nil ? (loading ? "Loading tasks" : "Tasks unavailable") : "No task rows recorded") : nil,
            gate: focus.map { unit in pageGate(unit.gate, checkout: detail?.gateDetail?[unit.id],
                                              calendar: calendar) },
            gateMessage: detail == nil ? (loading ? "Loading gate" : "Gate unavailable") : "No gate recorded",
            sessions: sessions.enumerated().map { index, session in
                pageSession(session, index: index, now: now, calendar: calendar)
            },
            sessionsMessage: liveUnavailable ? "Live sessions unavailable"
                : detail == nil ? (loading ? "Loading live sessions" : "Live sessions unavailable")
                : sessions.isEmpty ? "No live sessions" : nil,
            recent: detail.map { value in pageRecent(value, calendar: calendar) }
                ?? PageRecent(days: [], timeMessage: loading ? "Loading recent work" : "Recent work unavailable",
                              historyMessage: loading ? "Loading history" : "History unavailable"),
            terms: terms, termsMessage: termsMessage, failure: failure.map(Self.failure),
            liveFailure: liveFailure.map(Self.failure))
    }

    static func pageState(_ state: ProjectState, sessions: [LiveSession]) -> String {
        guard state == .active else { return title(state.rawValue) }
        if sessions.contains(where: { session in session.state == .running }) { return "Live" }
        if sessions.contains(where: { session in session.state == .waiting }) { return "Waiting" }
        return sessions.isEmpty ? "Active" : "Quiet"
    }

    static func pageUnit(_ unit: ProjectPlanUnit, calendar: Calendar) -> PageUnit {
        let progress = unit.provider == .funnel ? nil : boardProgress(LivePlan(
            unit: unit.title, done: unit.done, total: unit.total, stage: unit.stage.rawValue, task: nil, gate: nil))
        let closure: String?
        switch unit.closed {
        case .missing, .value(false): closure = nil
        case .null: closure = "Closure status not recorded"
        case .value(true):
            switch unit.closedAt {
            case .missing: closure = nil
            case .null: closure = "No closure time recorded"
            case .value(let at): closure = "Closed " + pageDate(at, calendar: calendar)
            }
        }
        return PageUnit(id: unit.id, title: unit.title, progress: progress,
                        current: unit.currentTask.map { task in "\(task.id) \(task.title)" },
                        stage: pageStage(unit.stage, stale: unit.stale), tasks: pageTasks(unit),
                        counts: unit.provider == .funnel ? unit.counts.keys.sorted().map { status in
                            "\(status): \(unit.counts[status] ?? 0)"
                        } : [], weekly: unit.provider == .funnel ? "\(unit.weeklyCount) this week" : nil,
                        closure: closure, error: unit.error, waiting: unit.waitingOnOwner,
                        checkoutDifferences: unit.checkouts > 0
                            ? "\(unit.checkouts) other \(unit.checkouts == 1 ? "checkout differs" : "checkouts differ")" : nil,
                        metadata: "\(unit.provider.rawValue) · Read \(pageDate(unit.asOf, calendar: calendar, format: "d MMM yyyy, HH:mm"))")
    }

    static func pageUnits(_ units: [ProjectPlanUnit], calendar: Calendar) -> PageUnits {
        let hasMissing = units.contains { unit in if case .missing = unit.closed { return true }; return false }
        if hasMissing || units.isEmpty {
            let rows = units.map { unit -> PageUnit in
                let row = pageUnit(unit, calendar: calendar)
                return PageUnit(id: row.id, title: row.title, progress: row.progress, current: row.current,
                                stage: row.stage, tasks: row.tasks, counts: row.counts, weekly: row.weekly,
                                closure: unit.closed == .null ? "Closure status not recorded" : nil,
                                error: row.error, waiting: row.waiting,
                                checkoutDifferences: row.checkoutDifferences, metadata: row.metadata)
            }
            return PageUnits(heading: "Units", openCount: nil, rows: rows, unclassified: [],
                             lastClosed: nil, closedMessage: nil)
        }
        let open = units.filter { unit in unit.closed == .value(false) }
        let unknown = units.filter { unit in unit.closed == .null }
        let closed = units.first { unit in unit.closed == .value(true) }
        return PageUnits(heading: "Open changes", openCount: open.count,
                         rows: open.map { unit in pageUnit(unit, calendar: calendar) },
                         unclassified: unknown.map { unit in pageUnit(unit, calendar: calendar) },
                         lastClosed: closed.map { unit in pageUnit(unit, calendar: calendar) },
                         closedMessage: closed == nil && unknown.isEmpty ? "No closed unit returned" : nil)
    }

    static func pageStage(_ stage: ProjectPlanStage, stale: Bool) -> PageStage {
        let stages: [ProjectPlanStage] = [.proposed, .planned, .building, .review, .done]
        let current = stages.firstIndex(of: stage)
        let steps = stages.enumerated().map { index, value in
            PageStage.Step(index: index, label: title(value.rawValue), current: index == current,
                           complete: current.map { selected in index < selected } ?? false)
        }
        return PageStage(label: title(stage.rawValue), steps: steps,
                         flags: (stage == .blocked ? ["Blocked"] : []) + (stale ? ["Stale"] : []))
    }

    static func pageTasks(_ unit: ProjectPlanUnit) -> PageTasks? {
        guard unit.provider != .funnel else { return nil }
        func row(_ task: ProjectTask) -> PageTask {
            PageTask(id: task.id, title: task.title, status: task.status, owner: task.owner)
        }
        let finished = unit.tasks.filter { task in task.status == "done" }.map(row)
        let unfinished = unit.tasks.filter { task in task.status != "done" }
        let current = unit.currentTask.map { task in
            unfinished.first { candidate in candidate.id == task.id }.map(row)
                ?? PageTask(id: task.id, title: task.title, status: "", owner: nil)
        }
        let queued = unfinished.filter { task in task.id != current?.id }.map(row)
        return PageTasks(finished: finished, current: current, next: queued.first, queued: queued,
                         summary: "\(finished.count) complete · \(unfinished.count) remaining",
                         foldedLabel: "\(finished.count) completed \(finished.count == 1 ? "task" : "tasks") folded",
                         queueLabel: "\(queued.count) queued",
                         emptyMessage: unit.tasks.isEmpty ? "No task rows recorded" : nil)
    }

    static func pageGate(_ gate: ProjectGate, checkout: ProjectGateDetail?, calendar: Calendar) -> PageGate {
        let status: String
        switch gate.status {
        case .passed: status = "Passed"
        case .failed: status = "Failed"
        case .stale: status = "Stale"
        case .none: status = "No gate recorded"
        }
        let result: String?
        switch gate.result {
        case .missing: result = nil
        case .null: result = "Run result not recorded"
        case .value(let value): result = "Last run result: " + value.rawValue
        }
        return PageGate(status: status, warning: gate.status == .failed || gate.status == .stale,
                        day: gate.at.map { at in "Last run " + pageDate(at, calendar: calendar) }
                            ?? (gate.status == .none ? nil : "No run time recorded"), result: result,
                        movement: checkout.map { value in commits(value) },
                        checkout: checkout.map { value in Self.checkout(value) },
                        dirty: checkout?.dirty == true,
                        failedRuns: gate.failedRuns > 0 ? "Failed runs: \(gate.failedRuns)" : nil)
    }

    static func pageDate(_ at: Int, calendar: Calendar, format: String = "d MMM yyyy") -> String {
        boardDate(Date(timeIntervalSince1970: Double(at)), calendar: calendar, format: format)
    }
}
