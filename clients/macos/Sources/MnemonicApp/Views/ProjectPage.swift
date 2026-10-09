import SwiftUI
import MnemonicShared

struct ProjectPage: View {
    let key: String
    let detail: ProjectDetail?
    let failure: LocalAPIFailure?
    let loading: Bool
    let onBack: () -> Void
    let liveFailure: LocalAPIFailure?

    init(key: String, detail: ProjectDetail?, failure: LocalAPIFailure?, loading: Bool,
         liveFailure: LocalAPIFailure? = nil, onBack: @escaping () -> Void) {
        self.key = key
        self.detail = detail
        self.failure = failure
        self.loading = loading
        self.liveFailure = liveFailure
        self.onBack = onBack
    }

    // The controller binds delivery to this route; merged keys may resolve canonically.
    private var shown: ProjectDetail? { detail }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.xl) {
                header
                if let failure {
                    let message = ProjectPresentation.failure(failure)
                    Card {
                        ProjectNotice(icon: "exclamationmark.circle", title: message.title, detail: message.detail)
                    }
                }
                ProjectSection(title: "Now", icon: "bolt.circle") { nowSection }
                ProjectSection(title: "Plan", icon: "checklist") {
                    planSection
                    if let terms = shown?.topTerms, !terms.isEmpty {
                        Text("Top terms").font(Theme.Font.captionBold).foregroundStyle(Theme.Palette.textMuted)
                        ProjectTermsView(terms: terms)
                    }
                }
                ProjectSection(title: "Gate", icon: "checkmark.shield") { gateSection }
                ProjectSection(title: "Time", icon: "clock") { timeSection }
                ProjectSection(title: "History", icon: "text.book.closed") { historySection }
            }
            .padding(Theme.Space.xxl)
            .frame(maxWidth: 1_040, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .background(Theme.Palette.bgPrimary)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: Theme.Space.lg) {
            Button(action: onBack) {
                Label { Text(verbatim: "Projects") } icon: { Image(systemName: "chevron.left") }
            }
            .buttonStyle(SubtleButtonStyle())
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: Theme.Space.sm) {
                    Text(verbatim: shown?.name ?? "Project").font(Theme.Font.display)
                        .tracking(Theme.Font.trackingDisplay).foregroundStyle(Theme.Palette.textPrimary)
                        .textSelection(.enabled)
                    Text(verbatim: key).font(Theme.Font.mono).foregroundStyle(Theme.Palette.textMuted)
                        .textSelection(.enabled)
                }
                Spacer()
                if let shown, shown.state != .active {
                    ProjectPill(text: ProjectPresentation.title(shown.state.rawValue))
                }
                if loading { ProgressView().controlSize(.small).padding(.top, 8) }
            }
        }
    }

    @ViewBuilder private var nowSection: some View {
        if let liveFailure {
            let message = ProjectPresentation.failure(liveFailure)
            ProjectNotice(icon: "antenna.radiowaves.left.and.right", title: "Live sessions unavailable", detail: message.detail)
        } else if let shown {
            if shown.sessions.isEmpty {
                ProjectNotice(icon: "moon", title: "No live sessions",
                              detail: "No agent session is currently recorded for this project.")
            } else {
                VStack(spacing: Theme.Space.md) {
                    ForEach(Array(shown.sessions.prefix(24).enumerated()), id: \.offset) { _, session in
                        HStack(alignment: .top) {
                            VStack(alignment: .leading, spacing: 5) {
                                ProjectSessionLabel(session: session)
                                Text(verbatim: session.driver == "program" ? "Automated session" : "Owner-led session")
                                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                            }
                            Spacer()
                            Text(verbatim: "Started \(ProjectPresentation.date(session.startedAt))")
                                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                        }
                    }
                    if shown.sessions.count > 24 {
                        boundedNotice(shown.sessions.count - 24, noun: "sessions")
                    }
                }
            }
        } else {
            unavailable("Live sessions")
        }
    }

    @ViewBuilder private var planSection: some View {
        if let shown {
            if shown.plan.units.isEmpty {
                ProjectNotice(icon: "doc.badge.ellipsis", title: "No plan recorded",
                              detail: shown.plan.message ?? "No plan units or task files have been recorded for this project.")
            } else {
                LazyVStack(alignment: .leading, spacing: Theme.Space.xl) {
                    ForEach(shown.plan.units) { unit in planUnit(unit) }
                }
            }
        } else {
            unavailable("Plan")
        }
    }

    private func planUnit(_ unit: ProjectPlanUnit) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            HStack(alignment: .top, spacing: Theme.Space.sm) {
                VStack(alignment: .leading, spacing: 4) {
                    Text(verbatim: unit.title).font(Theme.Font.title).foregroundStyle(Theme.Palette.textPrimary)
                    Text(verbatim: unit.id).font(Theme.Font.mono).foregroundStyle(Theme.Palette.textMuted)
                }
                Spacer()
                ProjectPill(text: ProjectPresentation.title(unit.stage.rawValue),
                            color: unit.stage == .blocked ? Theme.Palette.security : Theme.Palette.textMuted)
                if unit.stale { ProjectPill(text: "Stale", color: Theme.Palette.security) }
            }
            Text(verbatim: ProjectPresentation.progressLabel(unit)).font(Theme.Font.bodyMedium)
                .foregroundStyle(Theme.Palette.textPrimary)
            if let fraction = ProjectPresentation.completion(unit) {
                ProgressView(value: fraction).tint(Theme.Palette.accent)
                    .accessibilityLabel(Text(verbatim: ProjectPresentation.progressLabel(unit)))
            }
            if unit.provider == .funnel {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 135), alignment: .leading)], alignment: .leading) {
                    ForEach(unit.counts.keys.sorted(), id: \.self) { status in
                        Text(verbatim: "\(ProjectPresentation.title(status)): \(unit.counts[status] ?? 0)")
                            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    }
                }
            }
            if let task = unit.currentTask {
                VStack(alignment: .leading, spacing: 4) {
                    Text(verbatim: "Current · \(task.id)").font(Theme.Font.captionBold).foregroundStyle(Theme.Palette.textMuted)
                    Text(verbatim: task.title).font(Theme.Font.body).foregroundStyle(Theme.Palette.textPrimary)
                    Text(verbatim: ProjectPresentation.currentSource(task.source)).font(Theme.Font.caption)
                        .foregroundStyle(Theme.Palette.textMuted)
                }
            } else {
                Text(verbatim: "No current task recorded").font(Theme.Font.body).foregroundStyle(Theme.Palette.textMuted)
            }
            if let error = unit.error {
                Label { Text(verbatim: "Plan read incomplete · \(error)") } icon: {
                    Image(systemName: "exclamationmark.triangle")
                }
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.security)
            }
            if !unit.waitingOnOwner.isEmpty {
                DisclosureGroup {
                    ForEach(Array(unit.waitingOnOwner.prefix(20).enumerated()), id: \.offset) { _, item in
                        Text(verbatim: item).font(Theme.Font.body).foregroundStyle(Theme.Palette.textPrimary)
                    }
                    if unit.waitingOnOwner.count > 20 { boundedNotice(unit.waitingOnOwner.count - 20, noun: "items") }
                } label: {
                    Text(verbatim: "Waiting on owner · \(unit.waitingOnOwner.count)")
                        .font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.security)
                }
            }
            if !unit.tasks.isEmpty {
                DisclosureGroup {
                    VStack(alignment: .leading, spacing: Theme.Space.md) {
                        ForEach(unit.tasks.prefix(100)) { task in taskRow(task) }
                        if unit.tasks.count > 100 { boundedNotice(unit.tasks.count - 100, noun: "tasks") }
                    }
                    .padding(.top, Theme.Space.sm)
                } label: {
                    Text(verbatim: "Tasks · \(unit.tasks.count)").font(Theme.Font.bodyMedium)
                        .foregroundStyle(Theme.Palette.textPrimary)
                }
            } else {
                Text(verbatim: unit.provider == .funnel ? "This status table has no task rows." : "No task rows recorded.")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
            Text(verbatim: "Read \(ProjectPresentation.date(unit.asOf)) · \(ProjectPresentation.title(unit.provider.rawValue))")
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            Divider().overlay(Theme.Palette.border)
        }
    }

    private func taskRow(_ task: ProjectTask) -> some View {
        HStack(alignment: .top, spacing: Theme.Space.sm) {
            Image(systemName: task.status == "done" ? "checkmark.circle" : "circle")
                .foregroundStyle(task.status == "done" ? Theme.Palette.decision : Theme.Palette.textMuted)
            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: task.title).font(Theme.Font.body).foregroundStyle(Theme.Palette.textPrimary)
                Text(verbatim: "\(task.id) · \(ProjectPresentation.title(task.status))")
                    .font(Theme.Font.mono).foregroundStyle(Theme.Palette.textMuted)
                if let owner = task.owner {
                    Text(verbatim: "Owner: \(owner)").font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                }
            }
        }
    }

    @ViewBuilder private var gateSection: some View {
        if let shown {
            if shown.plan.units.isEmpty {
                ProjectNotice(icon: "checkmark.shield", title: "No gate receipts recorded",
                              detail: "A plan unit's latest gate result and checkout details will appear here.")
            } else {
                VStack(alignment: .leading, spacing: Theme.Space.lg) {
                    ForEach(shown.plan.units) { unit in
                        VStack(alignment: .leading, spacing: Theme.Space.sm) {
                            HStack {
                                Text(verbatim: unit.title).font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary)
                                Spacer()
                                ProjectPill(text: ProjectPresentation.gate(unit.gate.status), color: gateColor(unit.gate.status))
                            }
                            if let at = unit.gate.at {
                                Text(verbatim: "Receipt · \(ProjectPresentation.date(at))")
                                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                            } else {
                                Text(verbatim: "No gate timestamp recorded").font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                            }
                            if unit.gate.failedRuns > 0 {
                                Text(verbatim: "Failed runs: \(unit.gate.failedRuns)")
                                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.feedback)
                            }
                            let checkout = shown.gateDetail?[unit.id]
                            Text(verbatim: unit.checkouts == 0 ? "No checkout recorded" : "\(unit.checkouts) \(unit.checkouts == 1 ? "checkout" : "checkouts")")
                                .font(Theme.Font.captionBold).foregroundStyle(Theme.Palette.textMuted)
                            Text(verbatim: "\(ProjectPresentation.checkout(checkout)) · \(ProjectPresentation.commits(checkout))")
                                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                            Divider().overlay(Theme.Palette.border)
                        }
                    }
                }
            }
        } else {
            unavailable("Gate receipts and checkout details")
        }
    }

    private func gateColor(_ status: ProjectGateStatus) -> Color {
        switch status {
        case .passed: return Theme.Palette.decision
        case .failed: return Theme.Palette.feedback
        case .stale: return Theme.Palette.security
        case .none: return Theme.Palette.textMuted
        }
    }

    @ViewBuilder private var timeSection: some View {
        if let shown {
            let days = ProjectPresentation.days(shown.time)
            if days.isEmpty {
                ProjectNotice(icon: "clock", title: "No daily time recorded",
                              detail: "Your attributed activity and agent time will appear here for the last 14 days.")
            } else {
                VStack(alignment: .leading, spacing: Theme.Space.lg) {
                    HStack(spacing: Theme.Space.xxl) {
                        timeMetric("You", value: ProjectPresentation.ownerTime(
                            ProjectPresentation.ownerTotal(days), estimated: shown.ownerEstimate))
                        timeMetric("Agents", value: ProjectPresentation.duration(
                            days.reduce(0) { sum, day in sum + Double(day.agentSeconds) }))
                        Spacer()
                        Text(verbatim: "\(days.count) recorded days").font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    }
                    Text(verbatim: shown.ownerEstimate
                         ? "Your time is estimated from attributed activity. Agent time is counted separately."
                         : "Your time and agent time are counted separately.")
                        .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    VStack(spacing: 0) {
                        HStack {
                            Text(verbatim: "Day").frame(maxWidth: .infinity, alignment: .leading)
                            Text(verbatim: "You").frame(width: 110, alignment: .trailing)
                            Text(verbatim: "Agents").frame(width: 110, alignment: .trailing)
                        }
                        .font(Theme.Font.captionBold).foregroundStyle(Theme.Palette.textMuted).padding(.bottom, 8)
                        ForEach(days) { day in
                            HStack {
                                Text(verbatim: day.day).frame(maxWidth: .infinity, alignment: .leading)
                                Text(verbatim: ProjectPresentation.ownerTime(day.ownerSeconds, estimated: shown.ownerEstimate))
                                    .frame(width: 110, alignment: .trailing)
                                Text(verbatim: ProjectPresentation.duration(Double(day.agentSeconds)))
                                    .frame(width: 110, alignment: .trailing)
                            }
                            .font(Theme.Font.mono).foregroundStyle(Theme.Palette.textPrimary)
                            .padding(.vertical, 7)
                            Divider().overlay(Theme.Palette.border)
                        }
                    }
                    if days.contains(where: { day in day.ownerSeconds == nil }) {
                        Text(verbatim: "Unknown means owner attribution is unavailable; it does not mean zero time.")
                            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    }
                }
            }
        } else {
            unavailable("Daily time")
        }
    }

    private func timeMetric(_ label: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(verbatim: label).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            Text(verbatim: value).font(Theme.Font.heading).monospacedDigit().foregroundStyle(Theme.Palette.textPrimary)
        }
    }

    @ViewBuilder private var historySection: some View {
        if let shown {
            if shown.history.isEmpty {
                ProjectNotice(icon: "text.book.closed", title: "No project memories recorded",
                              detail: "Linked memory titles and their stable IDs will appear here.")
            } else {
                let entries = shown.history.sorted { left, right in
                    left.at == right.at ? left.id < right.id : left.at > right.at
                }
                LazyVStack(alignment: .leading, spacing: Theme.Space.lg) {
                    ForEach(entries.prefix(100)) { item in
                        VStack(alignment: .leading, spacing: 5) {
                            Text(verbatim: item.title).font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary)
                                .textSelection(.enabled)
                            HStack {
                                Text(verbatim: item.id).font(Theme.Font.mono).textSelection(.enabled)
                                Spacer()
                                Text(verbatim: ProjectPresentation.date(item.at)).font(Theme.Font.caption)
                            }
                            .foregroundStyle(Theme.Palette.textMuted)
                        }
                    }
                    if entries.count > 100 { boundedNotice(entries.count - 100, noun: "history entries") }
                }
            }
        } else {
            unavailable("Project history")
        }
    }

    private func unavailable(_ name: String) -> some View {
        ProjectNotice(icon: loading ? "hourglass" : "questionmark.circle",
                      title: loading ? "Loading \(name.lowercased())" : "\(name) unavailable",
                      detail: loading ? "Waiting for this project's data." : "No project detail has been received.")
    }

    private func boundedNotice(_ count: Int, noun: String) -> some View {
        Text(verbatim: "\(count) additional \(noun) are not shown in this view.")
            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
    }
}
