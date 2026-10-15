import SwiftUI
import MnemonicShared

struct ProjectPage: View {
    let key: String
    let detail: ProjectDetail?
    let failure: LocalAPIFailure?
    let loading: Bool
    let onBack: () -> Void
    let liveFailure: LocalAPIFailure?
    let now: Date
    @Environment(\.calendar) private var calendar

    init(key: String, detail: ProjectDetail?, failure: LocalAPIFailure?, loading: Bool,
         liveFailure: LocalAPIFailure? = nil, now: Date = Date(), onBack: @escaping () -> Void) {
        self.key = key
        self.detail = detail
        self.failure = failure
        self.loading = loading
        self.liveFailure = liveFailure
        self.now = now
        self.onBack = onBack
    }

    var body: some View {
        let page = ProjectPresentation.page(detail: detail, failure: failure, liveFailure: liveFailure,
                                            loading: loading, now: now, calendar: calendar)
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                header(page)
                if let failure = page.failure { ProjectPageFailure(message: failure) }
                if let focus = page.focus { ProjectPageFocus(unit: focus) }
                if let message = page.planMessage {
                    ProjectPagePanel { ProjectPageNote(text: message) }
                }
                HStack(alignment: .top, spacing: 16) {
                    ProjectPageUnits(units: page.units, focusID: page.focus?.id,
                                     terms: page.terms, termsMessage: page.termsMessage)
                        .frame(width: 280)
                    VStack(alignment: .leading, spacing: 12) {
                        HStack(alignment: .top, spacing: 12) {
                            if page.focus?.weekly == nil {
                                ProjectPageTasks(tasks: page.focus?.tasks, message: page.tasksMessage)
                            } else { Spacer(minLength: 0) }
                            ProjectPageGate(gate: page.gate, message: page.gateMessage)
                                .frame(width: 194)
                        }
                        ProjectPageSessions(sessions: page.sessions, message: page.sessionsMessage,
                                            failure: page.liveFailure)
                    }
                    .frame(maxWidth: .infinity)
                }
                ProjectPageRecent(recent: page.recent)
            }
            .padding(.horizontal, 28).padding(.vertical, 24)
            .frame(maxWidth: 1_160, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .foregroundStyle(AppPalette.textPrimary).background(AppPalette.window)
    }

    private func header(_ page: ProjectPresentation.Page) -> some View {
        HStack(spacing: 16) {
            VStack(alignment: .leading, spacing: 4) {
                HStack(spacing: 11) {
                    Text(verbatim: page.name).font(.system(size: 27, weight: .semibold)).tracking(-0.8)
                        .lineLimit(1).truncationMode(.tail).textSelection(.enabled)
                    if let state = page.state {
                        HStack(spacing: 5) {
                            Circle().frame(width: 5, height: 5)
                            Text(verbatim: state).font(.system(size: 11, weight: .semibold))
                        }
                        .foregroundStyle(state == "Live" ? AppPalette.agents : AppPalette.textSecondary)
                        .fixedSize()
                    }
                    if loading { ProgressView().controlSize(.small) }
                }
                HStack(spacing: 5) {
                    if let times = page.today {
                        Text("Today:")
                        Text(verbatim: "you " + times.owner).foregroundStyle(AppPalette.owner)
                        Text("·")
                        Text(verbatim: "agents " + times.agents).foregroundStyle(AppPalette.agents)
                    } else if let message = page.todayMessage { Text(verbatim: message) }
                    if let count = page.sessionCount { Text(verbatim: "· " + count) }
                    if let registry = page.registry { Text(verbatim: "· " + registry) }
                }
                .font(.system(size: 11)).monospacedDigit().foregroundStyle(AppPalette.textSecondary)
                .lineLimit(1).truncationMode(.tail)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            Button(action: onBack) {
                Label("Projects", systemImage: "chevron.left").font(.system(size: 12))
                    .padding(.horizontal, 13).frame(height: 44)
                    .background(AppPalette.raised, in: RoundedRectangle(cornerRadius: 8))
                    .overlay(RoundedRectangle(cornerRadius: 8).stroke(AppPalette.cardBorder, lineWidth: 1))
            }
            .buttonStyle(.plain)
        }
        .frame(minHeight: 56)
    }
}

struct ProjectPagePanel<Content: View>: View {
    var raised = false
    var padding: CGFloat = 14
    let content: Content

    init(raised: Bool = false, padding: CGFloat = 14, @ViewBuilder content: () -> Content) {
        self.raised = raised
        self.padding = padding
        self.content = content()
    }

    var body: some View {
        content.frame(maxWidth: .infinity, alignment: .leading).padding(padding)
            .background(raised ? AppPalette.raised : AppPalette.sidebar, in: RoundedRectangle(cornerRadius: 12))
            .overlay(RoundedRectangle(cornerRadius: 12).stroke(AppPalette.cardBorder, lineWidth: 1))
    }
}

struct ProjectPageNote: View {
    let text: String
    var warning = false
    var body: some View {
        Text(verbatim: text).font(.system(size: 11)).foregroundStyle(warning ? AppPalette.owner : AppPalette.textSecondary)
            .fixedSize(horizontal: false, vertical: true)
    }
}

struct ProjectPageFailure: View {
    let message: (title: String, detail: String)
    var body: some View {
        ProjectPagePanel {
            VStack(alignment: .leading, spacing: 5) {
                Label { Text(verbatim: message.title) } icon: { Image(systemName: "exclamationmark.circle") }
                    .font(.system(size: 13, weight: .medium)).foregroundStyle(AppPalette.owner)
                ProjectPageNote(text: message.detail)
            }
        }
    }
}

struct ProjectPageBar: View {
    let progress: ProjectPresentation.BoardProgress
    var active = true
    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(AppPalette.progressTrack)
                Capsule().fill(active ? AppPalette.agents : AppPalette.unitProgress)
                    .frame(width: geometry.size.width * progress.fraction)
            }
        }
        .frame(height: 4).accessibilityLabel(Text(verbatim: progress.tasks + ", " + progress.percent))
    }
}
