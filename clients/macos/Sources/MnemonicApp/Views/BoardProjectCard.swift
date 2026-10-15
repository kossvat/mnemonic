import SwiftUI

struct BoardProjectCard: View {
    let card: ProjectPresentation.BoardCard
    let onOpen: (String) -> Void

    var body: some View {
        Button { onOpen(card.id) } label: {
            VStack(alignment: .leading, spacing: 6) {
                VStack(alignment: .leading, spacing: 3) {
                    HStack(spacing: 8) {
                        Text(verbatim: card.name).font(.system(size: 19, weight: .semibold)).tracking(-0.6)
                            .lineLimit(1).truncationMode(.tail).frame(maxWidth: .infinity, alignment: .leading)
                        HStack(spacing: 5) {
                            Circle().frame(width: 5, height: 5)
                            Text(verbatim: card.state).font(.system(size: 11, weight: .semibold))
                        }
                        .foregroundStyle(card.state == "Live" ? AppPalette.agents : AppPalette.textSecondary)
                        .fixedSize()
                    }
                    .frame(height: 25)
                    Text(verbatim: card.sessions).font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
                        .lineLimit(1).frame(height: 14)
                }
                if let agents = card.agents, !agents.isEmpty {
                    HStack(spacing: 12) {
                        ForEach(agents) { agent in agentChip(agent) }
                    }
                    .frame(height: 18)
                }
                VStack(spacing: 4) {
                    times("Today", card.today)
                    if let week = card.week { times("7 days", week) }
                }
                .padding(.vertical, 8)
                .overlay(alignment: .top) { Rectangle().fill(AppPalette.cardBorder).frame(height: 1) }
                .overlay(alignment: .bottom) { Rectangle().fill(AppPalette.cardBorder).frame(height: 1) }
                if let plan = card.plan { planBlock(plan) }
                Spacer(minLength: 0)
                if let footer = card.footer {
                    Text(verbatim: footer.text).font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
                        .lineLimit(1).truncationMode(.tail).frame(height: 14, alignment: .leading)
                        .help(footer.text).accessibilityLabel(footer.text)
                }
            }
            .foregroundStyle(AppPalette.textPrimary).padding(14)
            .frame(maxWidth: .infinity).frame(height: 330, alignment: .topLeading)
            .background(AppPalette.raised, in: RoundedRectangle(cornerRadius: 15))
            .overlay(RoundedRectangle(cornerRadius: 15).stroke(AppPalette.cardBorder, lineWidth: 1))
            .contentShape(RoundedRectangle(cornerRadius: 15))
        }
        .buttonStyle(.plain)
        .accessibilityHint("Open this project's plan, gate, time, and history")
    }

    private func agentChip(_ agent: ProjectPresentation.AgentCount) -> some View {
        HStack(spacing: 5) {
            ZStack {
                if agent.kind == "claude" { Circle().stroke(AppPalette.agents, lineWidth: 1) }
                else {
                    RoundedRectangle(cornerRadius: 3)
                        .stroke(AppPalette.agents, style: StrokeStyle(lineWidth: 1, dash: [2, 1]))
                }
                Text(verbatim: agent.kind == "claude" ? "C" : agent.kind == "codex" ? "X" : "?")
                    .font(.system(size: 9, weight: .bold))
            }
            .frame(width: 14, height: 14).accessibilityHidden(true)
            Text(verbatim: agent.label).font(.system(size: 11)).lineLimit(1).truncationMode(.tail)
        }
        .foregroundStyle(AppPalette.agents)
    }

    private func times(_ label: String, _ values: ProjectPresentation.BoardTimes) -> some View {
        HStack(spacing: 9) {
            Text(verbatim: label).foregroundStyle(AppPalette.textSecondary).frame(width: 47, alignment: .leading)
            Text(verbatim: "You " + values.owner).foregroundStyle(AppPalette.owner)
                .frame(maxWidth: .infinity, alignment: .leading)
            Text(verbatim: "Agents " + values.agents).foregroundStyle(AppPalette.agents)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
        .font(.system(size: 11)).monospacedDigit().lineLimit(1).truncationMode(.tail).frame(height: 15)
    }

    @ViewBuilder
    private func planBlock(_ plan: ProjectPresentation.BoardPlan) -> some View {
        if let message = plan.message {
            Text(verbatim: message).font(.system(size: 12)).foregroundStyle(AppPalette.textSecondary)
                .lineLimit(2).truncationMode(.tail)
        }
        if let unit = plan.unit {
            VStack(alignment: .leading, spacing: 2) {
                Text("ACTIVE CHANGE").font(.system(size: 10)).tracking(1).foregroundStyle(AppPalette.textSecondary)
                Text(verbatim: unit).font(.system(size: 14, weight: .medium)).lineLimit(1).truncationMode(.tail)
            }
            if let progress = plan.progress {
                VStack(spacing: 7) {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(verbatim: progress.percent).font(.system(size: 27, weight: .bold)).tracking(-0.8)
                        Spacer(minLength: 0)
                        Text(verbatim: progress.tasks).font(.system(size: 11)).foregroundStyle(AppPalette.textSecondary)
                    }
                    GeometryReader { geometry in
                        ZStack(alignment: .leading) {
                            Capsule().fill(AppPalette.progressTrack)
                            Capsule().fill(AppPalette.agents).frame(width: geometry.size.width * progress.fraction)
                        }
                    }
                    .frame(height: 4).accessibilityHidden(true)
                }
            }
            if let funnel = plan.funnel {
                VStack(alignment: .leading, spacing: 3) {
                    if let counts = funnel.counts {
                        Text(verbatim: counts).font(.system(size: 11)).foregroundStyle(AppPalette.textSecondary)
                            .lineLimit(2).truncationMode(.tail).help(counts)
                    }
                    if let weekly = funnel.weeklyCount {
                        Text(verbatim: weekly).font(.system(size: 12, weight: .medium)).foregroundStyle(AppPalette.agents)
                            .lineLimit(1).truncationMode(.tail)
                    }
                }
            }
            HStack(spacing: 6) {
                if let stage = plan.stage { badge(stage, stage: true) }
                if let gate = plan.gate { badge(gate, stage: false) }
            }
            if let task = plan.task {
                Text(verbatim: task).font(.system(size: 12)).lineSpacing(2)
                    .lineLimit(2).truncationMode(.tail).frame(height: 34, alignment: .topLeading)
                    .frame(maxWidth: .infinity, alignment: .leading)
            }
        }
    }

    private func badge(_ label: String, stage: Bool) -> some View {
        Text(verbatim: label).font(.system(size: 10)).lineLimit(1).truncationMode(.tail)
            .foregroundStyle(stage ? AppPalette.agents : AppPalette.textSecondary)
            .padding(.horizontal, 7).padding(.vertical, 2)
            .background(stage ? AppPalette.stage : AppPalette.badge, in: RoundedRectangle(cornerRadius: 5))
    }
}
