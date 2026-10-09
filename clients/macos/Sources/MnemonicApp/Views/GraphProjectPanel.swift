import MnemonicShared
import SwiftUI

/// A compact plan beside the graph; the full project page keeps every task.
struct GraphProjectPanel: View {
    let project: ProjectDetail?
    let loading: Bool
    let failure: CuratedGraphFailure?

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            HStack {
                Text("Plan").font(Theme.Font.title).foregroundStyle(Theme.Palette.textPrimary)
                Spacer()
                if loading { ProgressView().controlSize(.small) }
            }
            if let failure {
                Text(verbatim: failure.message).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
            if let project {
                if project.plan.units.isEmpty {
                    Text(verbatim: project.plan.message ?? "No plan units recorded.")
                        .font(Theme.Font.body).foregroundStyle(Theme.Palette.textMuted)
                }
                ForEach(project.plan.units.prefix(8)) { unit in
                    VStack(alignment: .leading, spacing: 7) {
                        Text(verbatim: unit.title).font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary)
                        HStack(spacing: 6) {
                            ProjectPill(text: ProjectPresentation.title(unit.stage.rawValue))
                            if unit.stale { ProjectPill(text: "Stale", color: Theme.Palette.security) }
                        }
                        Text(verbatim: ProjectPresentation.progressLabel(unit)).font(Theme.Font.caption)
                            .foregroundStyle(Theme.Palette.textMuted)
                        if let progress = ProjectPresentation.completion(unit) {
                            ProgressView(value: progress).tint(Theme.Palette.accent)
                        }
                        if let current = unit.currentTask {
                            Text(verbatim: "Current: \(current.title)").font(Theme.Font.body)
                                .foregroundStyle(Theme.Palette.textPrimary)
                        }
                        if let error = unit.error {
                            Text(verbatim: "Plan read incomplete: \(error)").font(Theme.Font.caption)
                                .foregroundStyle(Theme.Palette.security)
                        }
                        if !unit.waitingOnOwner.isEmpty {
                            Text(verbatim: "Waiting on owner: \(unit.waitingOnOwner.count) items")
                                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.security)
                        }
                        Text(verbatim: "Gate: \(ProjectPresentation.gate(unit.gate.status))")
                            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                        DisclosureGroup("Tasks") {
                            ForEach(unit.tasks.prefix(12)) { task in
                                HStack(alignment: .top, spacing: 6) {
                                    Image(systemName: task.status == "done" ? "checkmark.circle" : "circle")
                                    Text(verbatim: task.title).frame(maxWidth: .infinity, alignment: .leading)
                                }
                                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                                .padding(.vertical, 3)
                            }
                            if unit.tasks.isEmpty { Text("No task rows recorded.").font(Theme.Font.caption) }
                            if unit.tasks.count > 12 {
                                Text(verbatim: "\(unit.tasks.count - 12) more tasks on the full project page.").font(Theme.Font.caption)
                            }
                        }
                        .font(Theme.Font.caption)
                        Divider().overlay(Theme.Palette.border)
                    }
                }
                if project.plan.units.count > 8 {
                    Text(verbatim: "\(project.plan.units.count - 8) more plan units on the full project page.")
                        .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                }
                if let terms = project.topTerms {
                    Text("Top terms").font(Theme.Font.title).foregroundStyle(Theme.Palette.textPrimary)
                    ProjectTermsView(terms: terms)
                }
            } else if !loading && failure == nil {
                Text("No project detail received.").font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
        }
    }
}

struct ProjectTermsView: View {
    let terms: [ProjectTerm]

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            if terms.isEmpty {
                Text("No linked terms recorded.").font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            } else {
                LazyVGrid(columns: [GridItem(.adaptive(minimum: 115), alignment: .leading)], alignment: .leading, spacing: 8) {
                    ForEach(Array(terms.prefix(12).enumerated()), id: \.offset) { _, term in
                        Text(verbatim: "\(term.name) · \(term.memoryCount)")
                            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                            .padding(.horizontal, 8).padding(.vertical, 6)
                            .background(RoundedRectangle(cornerRadius: Theme.Radius.sm).fill(Theme.Palette.bgTint))
                    }
                }
                Text("Counts are linked memories.").font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                if terms.count > 12 {
                    Text(verbatim: "\(terms.count - 12) additional terms are not shown.")
                        .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                }
            }
        }
    }
}
