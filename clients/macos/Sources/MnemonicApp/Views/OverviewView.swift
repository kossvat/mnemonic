import Charts
import SwiftUI
import MnemonicShared

struct OverviewView: View {
    @ObservedObject var model: MnemonicAppModel

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.xl) {
                header

                LazyVGrid(columns: [
                    GridItem(.adaptive(minimum: 180), spacing: Theme.Space.md)
                ], spacing: Theme.Space.md) {
                    KPICard(
                        icon: "tray.full",
                        value: "\(model.status?.totalMemories ?? 0)",
                        label: "Memories"
                    )
                    KPICard(
                        icon: "person.3.sequence",
                        value: "\(model.status?.entities ?? 0)",
                        label: "Entities"
                    )
                    KPICard(
                        icon: "point.3.connected.trianglepath.dotted",
                        value: "\(model.status?.edges ?? 0)",
                        label: "Edges"
                    )
                    KPICard(
                        icon: "clock",
                        value: RelativeTime.string(from: model.status?.lastActivity),
                        label: "Last activity"
                    )
                }

                if let status = model.status, !status.byType.isEmpty {
                    typeBreakdown(status)
                }

                dailyChart
            }
            .padding(Theme.Space.xl)
        }
        .background(Theme.Palette.bgPrimary)
        .task { await model.refreshOverview() }
        .onReceive(Timer.publish(every: model.refreshInterval, on: .main, in: .common).autoconnect()) { _ in
            Task { await model.refreshOverview() }
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 4) {
                Text("Overview")
                    .font(Theme.Font.display)
                    .tracking(Theme.Font.trackingDisplay)
                    .foregroundStyle(Theme.Palette.textPrimary)
                Text("Live memory daemon state")
                    .font(Theme.Font.body)
                    .foregroundStyle(Theme.Palette.textMuted)
            }
            Spacer()
            if model.isLoading {
                ProgressView().controlSize(.small)
            }
        }
    }

    private func typeBreakdown(_ status: StatusResponse) -> some View {
        Card {
            VStack(alignment: .leading, spacing: Theme.Space.md) {
                SectionLabel("By Type")
                VStack(alignment: .leading, spacing: Theme.Space.sm) {
                    ForEach(status.byType) { item in
                        HStack(spacing: Theme.Space.md) {
                            TypeChip(type: item.memoryType)
                            // Proportional progress bar
                            GeometryReader { geo in
                                let total = max(1, status.totalMemories)
                                let pct = CGFloat(item.count) / CGFloat(total)
                                let color = MemoryTypeIcon.color(for: item.memoryType)
                                ZStack(alignment: .leading) {
                                    RoundedRectangle(cornerRadius: 2, style: .continuous)
                                        .fill(Theme.Palette.bgTint)
                                    RoundedRectangle(cornerRadius: 2, style: .continuous)
                                        .fill(color.opacity(0.55))
                                        .frame(width: geo.size.width * pct)
                                }
                            }
                            .frame(height: 4)
                            Text("\(item.count)")
                                .font(Theme.Font.mono)
                                .foregroundStyle(Theme.Palette.textMuted)
                                .frame(width: 32, alignment: .trailing)
                        }
                    }
                }
            }
        }
    }

    private var dailyChart: some View {
        Card {
            VStack(alignment: .leading, spacing: Theme.Space.md) {
                SectionLabel("Daily Activity")
                if model.daily.isEmpty {
                    Text("Waiting for the first day of data...")
                        .font(Theme.Font.body)
                        .foregroundStyle(Theme.Palette.textSubtle)
                        .frame(maxWidth: .infinity, minHeight: 160)
                } else {
                    Chart(model.daily) { day in
                        BarMark(
                            x: .value("Day", day.date),
                            y: .value("Memories", day.count),
                            width: .ratio(0.7)
                        )
                        .foregroundStyle(Theme.Palette.accent.opacity(0.7))
                        .cornerRadius(2)
                    }
                    .chartYAxis {
                        AxisMarks(values: .automatic(desiredCount: 4)) { _ in
                            AxisGridLine().foregroundStyle(Theme.Palette.border)
                            AxisValueLabel()
                                .font(Theme.Font.caption)
                                .foregroundStyle(Theme.Palette.textSubtle)
                        }
                    }
                    .chartXAxis {
                        AxisMarks(values: .automatic(desiredCount: 6)) { _ in
                            AxisValueLabel(format: .dateTime.day().month(.abbreviated))
                                .font(Theme.Font.caption)
                                .foregroundStyle(Theme.Palette.textSubtle)
                        }
                    }
                    .frame(height: 220)
                }
            }
        }
    }
}
