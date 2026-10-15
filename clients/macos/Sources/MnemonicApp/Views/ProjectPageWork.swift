import SwiftUI

struct ProjectPageSessions: View {
    let sessions: [ProjectPresentation.PageSession]
    let message: String?
    let failure: (title: String, detail: String)?
    var body: some View {
        ProjectPagePanel(raised: true) {
            VStack(alignment: .leading, spacing: 7) {
                HStack(spacing: 8) {
                    Text("Live sessions").font(.system(size: 14, weight: .semibold))
                    Spacer(minLength: 0)
                    if !sessions.isEmpty {
                        Text("state from the last record").font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
                    }
                }
                if let message { ProjectPageNote(text: message) }
                if let failure {
                    ProjectPageNote(text: failure.title, warning: true)
                    ProjectPageNote(text: failure.detail)
                }
                ForEach(sessions) { session in
                    HStack(alignment: .top, spacing: 10) {
                        agent(session).frame(width: 126, alignment: .leading)
                        HStack(spacing: 4) {
                            Text(verbatim: session.state)
                            if let lastRecord = session.lastRecord {
                                Text(verbatim: "· " + lastRecord).foregroundStyle(AppPalette.textSecondary)
                            }
                        }
                        .font(.system(size: 11)).lineLimit(1).truncationMode(.tail)
                        .frame(maxWidth: .infinity, alignment: .leading)
                        Text(verbatim: session.elapsed).font(.system(size: 11)).monospacedDigit()
                            .foregroundStyle(AppPalette.textSecondary).fixedSize()
                    }
                    .padding(.vertical, 5)
                    .overlay(alignment: .bottom) { Rectangle().fill(AppPalette.cardBorder).frame(height: 1) }
                }
                if !sessions.isEmpty {
                    DisclosureGroup {
                        ForEach(sessions) { session in
                            ProjectPageNote(text: session.agent + " · " + session.driver)
                        }
                    } label: { Text("Session details").font(.system(size: 10)) }
                    .tint(AppPalette.textSecondary)
                }
                Spacer(minLength: 0)
            }
            .frame(minHeight: 104, alignment: .topLeading)
        }
    }

    private func agent(_ session: ProjectPresentation.PageSession) -> some View {
        HStack(spacing: 6) {
            ZStack {
                if session.kind == "claude" { Circle().stroke(AppPalette.agents, lineWidth: 1) }
                else {
                    RoundedRectangle(cornerRadius: 3)
                        .stroke(AppPalette.agents, style: StrokeStyle(lineWidth: 1, dash: [2, 1]))
                }
                Text(verbatim: session.kind == "claude" ? "C" : session.kind == "codex" ? "X" : "?")
                    .font(.system(size: 10))
            }
            .frame(width: 16, height: 16).accessibilityHidden(true)
            Text(verbatim: session.agent).font(.system(size: 11)).lineLimit(1).truncationMode(.tail)
        }
        .foregroundStyle(AppPalette.agents)
    }
}

struct ProjectPageRecent: View {
    let recent: ProjectPresentation.PageRecent
    var body: some View {
        ProjectPagePanel(padding: 16) {
            VStack(alignment: .leading, spacing: 9) {
                Text("Recent work").font(.system(size: 14, weight: .semibold))
                if let message = recent.timeMessage { ProjectPageNote(text: message) }
                if let message = recent.historyMessage { ProjectPageNote(text: message) }
                LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 20, alignment: .topLeading),
                                        count: recent.columnCount), alignment: .leading, spacing: 12) {
                    ForEach(recent.days) { day in
                        VStack(alignment: .leading, spacing: 3) {
                            dayHeading(day).font(.system(size: 11)).monospacedDigit()
                                .fixedSize(horizontal: false, vertical: true)
                            ForEach(day.history) { entry in
                                history(entry).font(.system(size: 11)).foregroundStyle(AppPalette.textSecondary)
                                    .lineLimit(recent.historyLineLimit).truncationMode(.tail)
                                    .fixedSize(horizontal: false, vertical: true).textSelection(.enabled)
                                    .help(entry.title)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .topLeading).padding(.vertical, 4)
                    }
                }
            }
        }
    }

    private func dayHeading(_ day: ProjectPresentation.PageDay) -> Text {
        let label = Text(verbatim: day.label).fontWeight(.semibold)
        guard let times = day.times else { return label }
        return label + Text(verbatim: " · You " + times.owner).foregroundColor(AppPalette.owner)
            + Text(verbatim: " · Agents " + times.agents).foregroundColor(AppPalette.agents)
    }

    private func history(_ entry: ProjectPresentation.PageHistory) -> Text {
        let title = Text(verbatim: entry.title)
        guard let kind = entry.kind else { return title }
        return Text(verbatim: kind + ": ").foregroundColor(kind == "Correction" ? AppPalette.owner : AppPalette.textSecondary) + title
    }
}
