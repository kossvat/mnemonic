import SwiftUI
import MnemonicShared

struct LiveProjectsPage: View {
    @ObservedObject var live: WidgetLiveController
    @ObservedObject var store: LiveStore
    let openProject: (String) -> Void
    let search: () -> Void

    init(live: WidgetLiveController, openProject: @escaping (String) -> Void,
         search: @escaping () -> Void) {
        self.live = live
        store = live.store
        self.openProject = openProject
        self.search = search
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            HStack(alignment: .firstTextBaseline) {
                Text("Live projects")
                    .font(.system(size: 23, weight: .bold)).tracking(-0.6)
                Spacer()
                if let snapshot = store.shown, store.failure == nil {
                    Text(verbatim: "\(snapshot.projects.count + snapshot.moreProjects)")
                        .font(.system(size: 12, weight: .semibold)).monospacedDigit()
                        .padding(.horizontal, 9).padding(.vertical, 4)
                        .background(Capsule().fill(WT.fill))
                }
            }
            .foregroundStyle(WT.text)

            if let failure = store.failure {
                stateCard(icon: "antenna.radiowaves.left.and.right.slash",
                          title: failure.widgetTitle, detail: failure.widgetDetail)
            } else if let snapshot = store.shown {
                totals(snapshot.today)
                if snapshot.projects.isEmpty {
                    stateCard(icon: "moon.zzz", title: "No projects live right now",
                              detail: "Your next agent session will appear here.")
                } else {
                    VStack(spacing: 10) {
                        ForEach(snapshot.projects) { project in
                            LiveProjectCard(project: project, now: live.now) {
                                openProject(project.key)
                            }
                        }
                    }
                }
                if snapshot.moreProjects > 0 {
                    Text(verbatim: "\(snapshot.moreProjects) more live projects in the app")
                        .font(.system(size: 11)).foregroundStyle(WT.sub)
                }
                let unknown = snapshot.unknown.sessions.count + snapshot.unknown.moreSessions
                if unknown > 0 {
                    Label {
                        Text(verbatim: "\(unknown) \(unknown == 1 ? "session" : "sessions") without a project")
                    } icon: { Image(systemName: "questionmark.folder") }
                        .font(.system(size: 11, weight: .medium)).foregroundStyle(WT.sub)
                }
            } else {
                stateCard(icon: "hourglass", title: "Waiting for live projects",
                          detail: "Asking this profile's daemon for its current activity.")
            }
            footer
        }
        .padding(16)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func totals(_ today: LiveTotal) -> some View {
        HStack(spacing: 0) {
            total("You, estimated", seconds: today.ownerSeconds)
            Rectangle().fill(WT.sep).frame(width: 1, height: 32)
            total("Agents", seconds: today.agentSeconds)
        }
        .padding(.vertical, 13)
        .background(RoundedRectangle(cornerRadius: WT.R.card).fill(WT.fill))
        .accessibilityElement(children: .combine)
        .accessibilityLabel(Text(verbatim: "Today. You, estimated \(fmtDur(Double(today.ownerSeconds))). Agents \(fmtDur(Double(today.agentSeconds)))."))
    }

    private func total(_ title: String, seconds: Int) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(title).font(.system(size: 10, weight: .medium)).foregroundStyle(WT.sub)
            Text(fmtDur(Double(max(0, seconds))))
                .font(.system(size: 19, weight: .semibold)).monospacedDigit()
                .foregroundStyle(WT.text)
        }
        .frame(maxWidth: .infinity, alignment: .leading).padding(.leading, 14)
    }

    private func stateCard(icon: String, title: String, detail: String) -> some View {
        VStack(alignment: .leading, spacing: 9) {
            Image(systemName: icon).font(.system(size: 24)).foregroundStyle(WT.accent)
            Text(title).font(.system(size: 15, weight: .semibold)).foregroundStyle(WT.text)
            Text(detail).font(.system(size: 12)).foregroundStyle(WT.sub).fixedSize(horizontal: false, vertical: true)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(16).background(RoundedRectangle(cornerRadius: WT.R.card).fill(WT.fill))
    }

    private var footer: some View {
        VStack(alignment: .leading, spacing: 11) {
            Rectangle().fill(WT.sep).frame(height: 1)
            HStack {
                Label("Capture not reported", systemImage: "waveform.path")
                Spacer(minLength: 4)
                Text("Review unavailable")
            }
            .font(.system(size: 9, weight: .medium)).foregroundStyle(WT.sub)
            Button(action: search) {
                HStack(spacing: 8) {
                    Image(systemName: "magnifyingglass")
                    Text("Search memories in app")
                    Spacer()
                    Image(systemName: "arrow.up.right")
                }
                .font(.system(size: 12, weight: .medium)).foregroundStyle(WT.sub)
                .padding(11).background(RoundedRectangle(cornerRadius: WT.R.btn).fill(WT.fill))
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)
        }
    }
}

struct LiveProjectCard: View {
    let project: LiveProject
    let now: Date
    let open: () -> Void

    static func agents(_ sessions: [LiveSession]) -> [(String, Int)] {
        Dictionary(grouping: sessions, by: \.agent)
            .map { name, group in (name, group.count) }.sorted { left, right in left.0 < right.0 }
    }

    static func progress(_ plan: LivePlan) -> Double? {
        guard plan.total > 0 else { return nil }
        return min(1, max(0, Double(plan.done) / Double(plan.total)))
    }

    var body: some View {
        Button(action: open) {
            VStack(alignment: .leading, spacing: 11) {
                HStack(alignment: .firstTextBaseline) {
                    Text(project.name).font(.system(size: 15, weight: .semibold))
                        .lineLimit(1).foregroundStyle(WT.text)
                    Spacer(minLength: 5)
                    Text(project.state.rawValue.capitalized)
                        .font(.system(size: 10, weight: .medium))
                        .foregroundStyle(project.state == .running ? WT.working : WT.sub)
                }
                HStack(spacing: 8) {
                    ForEach(Self.agents(project.sessions), id: \.0) { name, count in
                        Label {
                            Text(verbatim: "\(name.capitalized) \(count)")
                        } icon: { Image(systemName: name == "codex" ? "terminal" : "sparkle") }
                    }
                    if project.moreSessions > 0 { Text(verbatim: "+\(project.moreSessions) more") }
                }
                .font(.system(size: 10, weight: .medium)).foregroundStyle(WT.sub)
                HStack(spacing: 12) {
                    Text(verbatim: "You ~\(fmtDur(Double(project.today.ownerSeconds)))")
                    Text(verbatim: "Agents \(fmtDur(Double(project.today.agentSeconds)))")
                }
                .font(.system(size: 11)).monospacedDigit().foregroundStyle(WT.sub)
                if let plan = project.plan {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Text(plan.unit).lineLimit(1)
                            Spacer(minLength: 4)
                            if let fraction = Self.progress(plan) {
                                Text(verbatim: "\(Int((fraction * 100).rounded()))%")
                                    .monospacedDigit().foregroundStyle(WT.accent)
                            } else {
                                Text(plan.stage.capitalized).foregroundStyle(WT.sub)
                            }
                        }
                        .font(.system(size: 11, weight: .semibold)).foregroundStyle(WT.text)
                        if let fraction = Self.progress(plan) {
                            GeometryReader { size in
                                Capsule().fill(WT.barIdle).overlay(alignment: .leading) {
                                    Capsule().fill(WT.accent).frame(width: size.size.width * fraction)
                                }
                            }.frame(height: 4)
                        }
                        Text(plan.task ?? "No current task recorded")
                            .font(.system(size: 10)).lineLimit(2).foregroundStyle(WT.sub)
                        HStack(spacing: 6) {
                            Text(plan.stage.replacingOccurrences(of: "_", with: " ").capitalized)
                                .foregroundStyle(plan.stage == "blocked" ? WT.stopped : WT.sub)
                            if let gate = plan.gate { Text(verbatim: "Gate \(gate)").foregroundStyle(WT.sub) }
                        }
                        .font(.system(size: 9, weight: .medium))
                    }
                } else {
                    Text("No plan recorded").font(.system(size: 11)).foregroundStyle(WT.sub)
                }
                if let start = project.sessions.map(\.started).min() {
                    Text(verbatim: "Session started \(fmtDur(max(0, now.timeIntervalSince(start)))) ago")
                        .font(.system(size: 9)).foregroundStyle(WT.sub)
                }
            }
            .padding(13).frame(maxWidth: .infinity, alignment: .leading)
            .background(RoundedRectangle(cornerRadius: WT.R.card).fill(WT.fill))
            .overlay(RoundedRectangle(cornerRadius: WT.R.card).stroke(WT.sep, lineWidth: 1))
            .contentShape(RoundedRectangle(cornerRadius: WT.R.card))
        }
        .buttonStyle(.plain)
        .accessibilityHint("Open this project in the app")
    }
}
