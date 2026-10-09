import SwiftUI
import MnemonicShared

struct ProjectsView: View {
    let board: ProjectBoard?
    let live: LiveSnapshot?
    let failure: LocalAPIFailure?
    let loading: Bool
    let now: Date
    let onOpen: (String) -> Void
    @State private var showStale = false

    var body: some View {
        let groups = ProjectPresentation.groups(board: board, live: live, now: now)
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.xl) {
                header
                if let failure {
                    let message = ProjectPresentation.failure(failure)
                    Card {
                        ProjectNotice(icon: "exclamationmark.circle", title: message.title, detail: message.detail)
                    }
                }
                liveSection(groups)
                if board != nil {
                    if !groups.recent.isEmpty {
                        VStack(alignment: .leading, spacing: Theme.Space.md) {
                            sectionHeading("Other projects", count: groups.recent.count)
                            LazyVStack(spacing: Theme.Space.sm) {
                                ForEach(groups.recent) { row in projectRow(row) }
                            }
                        }
                    }
                    if !groups.stale.isEmpty {
                        DisclosureGroup(isExpanded: $showStale) {
                            LazyVStack(spacing: Theme.Space.sm) {
                                ForEach(groups.stale) { row in projectRow(row) }
                            }
                            .padding(.top, Theme.Space.md)
                        } label: {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(verbatim: "Older activity · \(groups.stale.count)")
                                    .font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary)
                                Text(verbatim: "Last recorded activity was more than 30 days ago.")
                                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                            }
                        }
                        .tint(Theme.Palette.textMuted)
                    }
                    if groups.live.isEmpty && groups.recent.isEmpty && groups.stale.isEmpty {
                        Card {
                            ProjectNotice(
                                icon: "folder", title: groups.excluded > 0 ? "No active projects" : "No projects registered",
                                detail: groups.excluded > 0
                                    ? "All registered projects are archived, ignored, or merged."
                                    : "Projects will appear here after the workspace registry discovers them.")
                        }
                    }
                    if groups.excluded > 0 {
                        HStack(spacing: Theme.Space.sm) {
                            Image(systemName: "archivebox")
                            Text(verbatim: "Outside this board: \(groups.excludedLabel)")
                        }
                        .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    }
                } else if !loading && failure == nil {
                    Card {
                        ProjectNotice(icon: "folder.badge.questionmark", title: "Project registry unavailable",
                                      detail: "The board has not received the registered projects yet.")
                    }
                }
            }
            .padding(Theme.Space.xxl)
            .frame(maxWidth: 1_120, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .background(Theme.Palette.bgPrimary)
    }

    private var header: some View {
        HStack(alignment: .top) {
            VStack(alignment: .leading, spacing: Theme.Space.sm) {
                Text(verbatim: "Projects").font(Theme.Font.display).tracking(Theme.Font.trackingDisplay)
                    .foregroundStyle(Theme.Palette.textPrimary)
                Text(verbatim: "Live work, plans, and the time behind them.")
                    .font(Theme.Font.body).foregroundStyle(Theme.Palette.textMuted)
            }
            Spacer()
            if loading { ProgressView().controlSize(.small).padding(.top, 8) }
        }
    }

    private func sectionHeading(_ title: String, count: Int) -> some View {
        HStack(spacing: Theme.Space.sm) {
            Text(verbatim: title).font(Theme.Font.title).foregroundStyle(Theme.Palette.textPrimary)
            Text(verbatim: String(count)).font(Theme.Font.captionBold)
                .foregroundStyle(Theme.Palette.textMuted)
                .padding(.horizontal, 7).padding(.vertical, 3)
                .background(Capsule().fill(Theme.Palette.bgTint))
        }
    }

    private func liveSection(_ groups: ProjectPresentation.Groups) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            sectionHeading("Live now", count: groups.live.count)
            if !groups.live.isEmpty {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 300), spacing: Theme.Space.md)],
                          alignment: .leading, spacing: Theme.Space.md) {
                    ForEach(groups.live) { card in liveCard(card) }
                }
            } else {
                Card {
                    ProjectNotice(icon: live == nil ? "antenna.radiowaves.left.and.right" : "moon",
                                  title: live == nil ? "Live activity unavailable" : "No projects live right now",
                                  detail: live == nil
                                    ? (loading ? "Asking the daemon for its current sessions." : "No live snapshot has been received.")
                                    : "A project appears here when an agent session is in progress.")
                }
            }
            if let live, live.moreProjects > 0 {
                Text(verbatim: "\(live.moreProjects) additional live projects are outside this snapshot.")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
            if let live {
                let unknown = live.unknown.sessions.count + live.unknown.moreSessions
                if unknown > 0 {
                    Label {
                        Text(verbatim: "\(unknown) \(unknown == 1 ? "session has" : "sessions have") no recorded project.")
                    } icon: { Image(systemName: "questionmark.folder") }
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                }
            }
        }
    }

    private func liveCard(_ card: ProjectPresentation.LiveCard) -> some View {
        let project = card.project
        return Button { onOpen(project.key) } label: {
            Card(padding: Theme.Space.xl) {
                VStack(alignment: .leading, spacing: Theme.Space.lg) {
                    HStack(alignment: .top) {
                        Text(verbatim: project.name).font(Theme.Font.heading)
                            .foregroundStyle(Theme.Palette.textPrimary).lineLimit(2)
                        Spacer(minLength: 6)
                        ProjectPill(text: ProjectPresentation.title(project.state.rawValue),
                                    color: project.state == .running ? Theme.Palette.decision : Theme.Palette.textMuted)
                    }
                    VStack(alignment: .leading, spacing: 6) {
                        ForEach(Array(project.sessions.prefix(3).enumerated()), id: \.offset) { _, session in
                            HStack {
                                ProjectSessionLabel(session: session)
                                if session.driver == "program" {
                                    Text(verbatim: "Automated").font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                                }
                            }
                        }
                        let extra = max(0, project.sessions.count - 3) + project.moreSessions
                        if extra > 0 {
                            Text(verbatim: "+\(extra) more sessions").font(Theme.Font.caption)
                                .foregroundStyle(Theme.Palette.textMuted)
                        }
                    }
                    HStack(spacing: Theme.Space.xl) {
                        metric("You · last 7 days", value: ProjectPresentation.ownerTime(
                            card.row?.week.ownerSeconds, estimated: card.row?.week.ownerEstimate ?? false))
                        metric("Agents · last 7 days", value: ProjectPresentation.duration(
                            card.row.map { row in Double(row.week.agentSeconds) }))
                    }
                    Divider().overlay(Theme.Palette.border)
                    VStack(alignment: .leading, spacing: 6) {
                        let plan = project.plan ?? card.row?.plan
                        Text(verbatim: ProjectPresentation.planLine(plan, message: card.row?.planMessage))
                            .font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary).lineLimit(2)
                        Text(verbatim: plan?.task ?? "No current task recorded")
                            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted).lineLimit(2)
                    }
                    HStack {
                        Text(verbatim: "Open project").font(Theme.Font.captionBold)
                        Spacer()
                        Image(systemName: "arrow.up.right").font(.system(size: 10, weight: .semibold))
                    }
                    .foregroundStyle(Theme.Palette.accent)
                }
                .frame(maxWidth: .infinity, minHeight: 225, alignment: .topLeading)
            }
            .contentShape(RoundedRectangle(cornerRadius: Theme.Radius.md))
        }
        .buttonStyle(.plain)
        .accessibilityHint(Text(verbatim: "Open this project's plan, gate, time, and history"))
    }

    private func metric(_ title: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(verbatim: title).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            Text(verbatim: value).font(Theme.Font.title).monospacedDigit().foregroundStyle(Theme.Palette.textPrimary)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private func projectRow(_ row: ProjectBoardRow) -> some View {
        Button { onOpen(row.key) } label: {
            Card {
                HStack(spacing: Theme.Space.lg) {
                    Image(systemName: "folder").foregroundStyle(Theme.Palette.entityProject).frame(width: 22)
                    VStack(alignment: .leading, spacing: 5) {
                        Text(verbatim: row.name).font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary)
                        Text(verbatim: ProjectPresentation.planLine(row.plan, message: row.planMessage))
                            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted).lineLimit(1)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    VStack(alignment: .trailing, spacing: 5) {
                        Text(verbatim: ProjectPresentation.activity(row.lastActiveAt, now: now))
                        Text(verbatim: "Last 7 days · You \(ProjectPresentation.ownerTime(row.week.ownerSeconds, estimated: row.week.ownerEstimate)) · Agents \(ProjectPresentation.duration(Double(row.week.agentSeconds)))")
                    }
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted).monospacedDigit()
                    Image(systemName: "chevron.right").font(.system(size: 10)).foregroundStyle(Theme.Palette.textSubtle)
                }
            }
            .contentShape(RoundedRectangle(cornerRadius: Theme.Radius.md))
        }
        .buttonStyle(.plain)
    }
}
