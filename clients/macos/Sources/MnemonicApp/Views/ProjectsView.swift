import SwiftUI
import MnemonicShared

struct ProjectsView: View {
    let board: ProjectBoard?
    let live: LiveSnapshot?
    let failure: LocalAPIFailure?
    let loading: Bool
    let now: Date
    let onOpen: (String) -> Void
    /// Raised by the app whenever it takes back the proof of a paint.
    var paintRevision: UInt64 = 0
    /// Told which card was painted for which board and snapshot; the app's
    /// window passes it, renders do not.
    var onDraw: ((BoardPaint, String, UInt64) -> Void)? = nil
    @State private var showStale = false
    @Environment(\.calendar) private var calendar
    @Environment(\.mnemonicWindowVisible) private var windowVisible

    var body: some View {
        let display = ProjectPresentation.board(board: board, live: live, failure: failure,
                                                loading: loading, now: now, calendar: calendar)
        ScrollView {
            VStack(alignment: .leading, spacing: 12) {
                header(display)
                if let failure = display.failure {
                    VStack(alignment: .leading, spacing: 5) {
                        Text(verbatim: failure.title).font(.system(size: 14, weight: .semibold))
                            .foregroundStyle(AppPalette.textPrimary)
                        Text(verbatim: failure.detail).font(.system(size: 12))
                            .foregroundStyle(AppPalette.textSecondary)
                    }
                    .frame(maxWidth: .infinity, alignment: .leading).padding(14)
                    .background(AppPalette.raised, in: RoundedRectangle(cornerRadius: 15))
                }
                VStack(alignment: .leading, spacing: 10) {
                    HStack(spacing: 8) {
                        Text("Active now").font(.system(size: 14, weight: .semibold))
                        if let active = display.active {
                            Text(verbatim: active.projects).font(.system(size: 14))
                                .foregroundStyle(AppPalette.textSecondary)
                            Spacer(minLength: 12)
                            Text(verbatim: active.sessions).font(.system(size: 11))
                                .foregroundStyle(display.cut ? AppPalette.textSecondary : AppPalette.agents)
                        }
                    }
                    .frame(height: 24)
                    if let message = display.liveMessage { notice(message) }
                    if !display.cards.isEmpty {
                        LazyVGrid(columns: [GridItem(.adaptive(minimum: 300), spacing: 12)], spacing: 12) {
                            ForEach(display.cards) { card in
                                BoardProjectCard(card: card, onOpen: onOpen).overlay(alignment: .bottomTrailing) {
                                    if let onDraw {
                                        BoardPaintProbe(paint: BoardPaint(board: board, live: live), card: card.id,
                                                        revision: paintRevision, visible: windowVisible, onDraw: onDraw)
                                            .frame(width: 1, height: 1).allowsHitTesting(false).accessibilityHidden(true)
                                    }
                                }
                            }
                        }
                    }
                    ForEach(display.notices, id: \.self) { message in notice(message) }
                }
                if display.inactiveLine != nil || display.rowsMessage != nil {
                    VStack(alignment: .leading, spacing: 6) {
                        HStack {
                            Text("Not active now").font(.system(size: 14, weight: .semibold))
                            Spacer(minLength: 12)
                            if let line = display.inactiveLine {
                                Text(verbatim: line).font(.system(size: 11)).foregroundStyle(AppPalette.textSecondary)
                            }
                        }
                        .frame(height: 24)
                        if let message = display.rowsMessage { notice(message) }
                        rows(display.rows)
                    }
                }
                if let line = display.staleLine {
                    VStack(spacing: 6) {
                        Button { showStale.toggle() } label: {
                            HStack(spacing: 10) {
                                Image(systemName: showStale ? "chevron.down" : "chevron.right")
                                    .font(.system(size: 12)).frame(width: 15)
                                Text(verbatim: line)
                                Spacer(minLength: 12)
                                if let detail = display.staleDetail { Text(verbatim: detail).font(.system(size: 11)) }
                            }
                            .font(.system(size: 12)).foregroundStyle(AppPalette.textSecondary)
                            .padding(.horizontal, 12).frame(height: 44)
                            .background(AppPalette.folded, in: RoundedRectangle(cornerRadius: 9))
                            .overlay(RoundedRectangle(cornerRadius: 9).stroke(AppPalette.cardBorder, lineWidth: 1))
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .accessibilityValue(showStale ? "Expanded" : "Collapsed")
                        if showStale { rows(display.stale) }
                    }
                }
                if let excluded = display.excluded { notice("Outside this board: " + excluded) }
            }
            .foregroundStyle(AppPalette.textPrimary)
            .padding(.horizontal, 28).padding(.vertical, 24)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .background(AppPalette.window)
    }

    private func header(_ display: ProjectPresentation.Board) -> some View {
        HStack(alignment: .center, spacing: 20) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Projects").font(.system(size: 28, weight: .semibold)).tracking(-0.9)
                Text(verbatim: display.dateNote).font(.system(size: 11))
                    .foregroundStyle(AppPalette.textSecondary).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
            HStack(spacing: 24) {
                if let today = display.today { headerTimes("TODAY", today) }
                if let week = display.week { headerTimes("LAST 7 DAYS", week) }
            }
        }
        .frame(minHeight: 44)
    }

    private func headerTimes(_ label: String, _ values: ProjectPresentation.BoardTimes) -> some View {
        VStack(alignment: .leading, spacing: 3) {
            Text(verbatim: label).font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
            HStack(spacing: 9) {
                Text(verbatim: "You " + values.owner).foregroundStyle(AppPalette.owner)
                Text(verbatim: "Agents " + values.agents).foregroundStyle(AppPalette.agents)
            }
            .font(.system(size: 12)).monospacedDigit().fixedSize()
        }
    }

    private func notice(_ text: String) -> some View {
        Text(verbatim: text).font(.system(size: 12)).foregroundStyle(AppPalette.textSecondary)
            .frame(maxWidth: .infinity, alignment: .leading).padding(12)
            .background(AppPalette.folded, in: RoundedRectangle(cornerRadius: 9))
    }

    private func rows(_ values: [ProjectPresentation.BoardRow]) -> some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 440), spacing: 20)], spacing: 2) {
            ForEach(values) { row in
                Button { onOpen(row.id) } label: {
                    HStack(spacing: 12) {
                        VStack(alignment: .leading, spacing: 1) {
                            Text(verbatim: row.name).font(.system(size: 13, weight: .medium))
                                .foregroundStyle(AppPalette.textPrimary)
                            Text(verbatim: row.plan).font(.system(size: 10))
                                .foregroundStyle(AppPalette.textSecondary)
                            if let funnel = row.funnel {
                                if let counts = funnel.counts {
                                    Text(verbatim: counts).font(.system(size: 10))
                                        .foregroundStyle(AppPalette.textSecondary).help(counts)
                                }
                                if let weekly = funnel.weeklyCount {
                                    Text(verbatim: weekly).font(.system(size: 10)).foregroundStyle(AppPalette.agents)
                                }
                            }
                        }
                        .lineLimit(1).truncationMode(.tail).frame(maxWidth: .infinity, alignment: .leading)
                        VStack(alignment: .trailing, spacing: 1) {
                            Text(verbatim: row.activity).font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
                            Text(verbatim: "You " + row.week.owner).font(.system(size: 11)).foregroundStyle(AppPalette.owner)
                            Text(verbatim: "Agents " + row.week.agents + " · 7 days")
                                .font(.system(size: 10)).foregroundStyle(AppPalette.agents)
                        }
                        .monospacedDigit().fixedSize()
                    }
                    .padding(.horizontal, 12).frame(minHeight: 60)
                    .overlay(alignment: .bottom) { Rectangle().fill(AppPalette.border).frame(height: 1) }
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
            }
        }
    }
}
