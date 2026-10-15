import MnemonicShared
import SwiftUI

struct TimelineOverview: View {
    let snapshot: ProjectTimeline
    let focus: TimelineDrawing.Span
    let scale: TimelineDrawing.Scale
    let now: Date
    let calendar: Calendar
    let onScale: (TimelineDrawing.Scale) -> Void
    let onFocus: (TimelineDrawing.Span) -> Void

    var body: some View {
        HStack(spacing: 22) {
            VStack(alignment: .leading, spacing: 3) {
                Text(verbatim: snapshot.day == TimelineDay.today(now: now, calendar: calendar) ? "Today, so far" : "Day overview")
                    .font(.system(size: 12, weight: .semibold))
                Text(verbatim: TimelineDrawing.overviewNote(scale)).font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
            }
            .frame(width: 126, alignment: .leading)
            GeometryReader { geometry in
                let day = TimelineDrawing(snapshot, width: geometry.size.width, calendar: calendar)
                let frame = TimelineDrawing.frame(focus, in: .init(from: day.from, to: day.to), width: day.width)
                VStack(spacing: 6) {
                    TimelineAxis(drawing: day)
                    ZStack(alignment: .topLeading) {
                        RoundedRectangle(cornerRadius: 4).fill(AppPalette.activeRow)
                        RoundedRectangle(cornerRadius: 2).fill(AppPalette.timelineFocus.opacity(0.6))
                            .frame(width: frame.width).offset(x: frame.x)
                        ForEach(day.lanes.flatMap(\.blocks)) { block in
                            TimelineAgentMark(mark: block.mark, geometry: block.geometry)
                                .frame(width: block.geometry.paint.width, height: 6)
                                .offset(x: block.geometry.paint.x, y: 2)
                        }
                        ForEach(day.owner) { block in
                            Rectangle().fill(AppPalette.owner).frame(width: block.geometry.paint.width, height: 2)
                                .offset(x: block.geometry.paint.x, y: 10)
                        }
                        RoundedRectangle(cornerRadius: 2).strokeBorder(AppPalette.agents, lineWidth: 1)
                            .frame(width: frame.width).offset(x: frame.x)
                    }
                    .frame(height: 13).clipped()
                    .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(AppPalette.timelineTrackBorder, lineWidth: 1))
                }
                .frame(maxHeight: .infinity)
                .contentShape(Rectangle())
                .onTapGesture(coordinateSpace: .local) { location in
                    onFocus(TimelineDrawing.focus(at: location.x, width: day.width, day: .init(from: day.from, to: day.to)))
                }
                .accessibilityElement(children: .ignore)
                .accessibilityLabel("Day overview. Select a time to centre the two hour focus.")
                .accessibilityValue(Text(verbatim: TimelineDrawing.spanLabel(focus, calendar: calendar)))
                .accessibilityAdjustableAction { direction in
                    let end = direction == .increment ? focus.to + 1_800 : focus.to - 1_800
                    onFocus(TimelineDrawing.focus(endingAt: end, day: .init(from: day.from, to: day.to)))
                }
            }
            HStack(spacing: 2) {
                ForEach(TimelineDrawing.Scale.allCases, id: \.self) { value in
                    Button { onScale(value) } label: {
                        Text(verbatim: value.rawValue).font(.system(size: 12)).padding(.horizontal, 10).frame(height: 40)
                            .background(scale == value ? AppPalette.activeRow : .clear, in: RoundedRectangle(cornerRadius: 6))
                            .contentShape(Rectangle())
                    }
                    .buttonStyle(.plain).accessibilityAddTraits(scale == value ? .isSelected : [])
                }
            }
            .padding(2).overlay(RoundedRectangle(cornerRadius: 8).stroke(AppPalette.cardBorder, lineWidth: 1))
        }
        .padding(.horizontal, 16).frame(height: 72)
        .background(AppPalette.sidebar, in: RoundedRectangle(cornerRadius: 11))
        .overlay(RoundedRectangle(cornerRadius: 11).stroke(AppPalette.cardBorder, lineWidth: 1))
    }
}

struct TimelineDayTotals: View {
    let totals: TimelineDrawing.Totals
    let date: Date
    let calendar: Calendar

    var body: some View {
        VStack(alignment: .leading, spacing: 17) {
            Text(verbatim: ProjectPresentation.boardDate(date, calendar: calendar, format: "EEEE") + " totals")
                .font(.system(size: 16, weight: .semibold))
            value("You at the Mac", TimelineDrawing.ownerTime(totals.owner), color: AppPalette.owner)
            VStack(alignment: .leading, spacing: 4) {
                value("Agents", ProjectPresentation.duration(totals.agents), color: AppPalette.agents)
                Text("Parallel sessions count once.").font(.system(size: 11)).foregroundStyle(AppPalette.textSecondary)
            }
            Rectangle().fill(AppPalette.cardBorder).frame(height: 1)
            VStack(alignment: .leading, spacing: 8) {
                legend("C", "Claude Code · solid", rounded: true)
                legend("X", "Codex · dashed", rounded: false)
                HStack(spacing: 7) {
                    RoundedRectangle(cornerRadius: 2).fill(AppPalette.owner).frame(width: 17, height: 5)
                    Text("You · thin amber lane").font(.system(size: 10))
                }
            }
        }
        .padding(16).frame(width: 212, alignment: .leading)
        .background(AppPalette.raised, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(AppPalette.cardBorder, lineWidth: 1))
    }

    private func value(_ title: String, _ text: String, color: Color) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(verbatim: title).font(.system(size: 11)).foregroundStyle(color)
            Text(verbatim: text).font(.system(size: text == "Unavailable" ? 20 : 30, weight: .semibold))
                .tracking(-0.8).monospacedDigit().lineLimit(1).minimumScaleFactor(0.8)
        }
    }

    private func legend(_ mark: String, _ title: String, rounded: Bool) -> some View {
        HStack(spacing: 7) {
            Text(verbatim: mark).font(.system(size: 9, weight: .bold)).foregroundStyle(AppPalette.agents)
                .frame(width: 15, height: 15)
                .background(rounded ? AppPalette.timelineClaude : AppPalette.timelineCodex,
                            in: RoundedRectangle(cornerRadius: rounded ? 8 : 3))
                .overlay(RoundedRectangle(cornerRadius: rounded ? 8 : 3)
                    .stroke(AppPalette.agents, style: StrokeStyle(lineWidth: 1, dash: rounded ? [] : [2, 1])))
            Text(verbatim: title).font(.system(size: 10))
        }
    }
}

struct TimelineProjectTimes: View {
    let totals: TimelineDrawing.Totals

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack {
                Text("Time by project").font(.system(size: 14, weight: .semibold))
                Spacer()
                Text("you: estimate · agents: exact").font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
            }
            LazyVGrid(columns: [GridItem(.adaptive(minimum: 200), spacing: 12)], spacing: 12) {
                ForEach(totals.projects) { project in
                    VStack(alignment: .leading, spacing: 9) {
                        Text(verbatim: project.name).font(.system(size: 13, weight: .medium))
                            .lineLimit(1).truncationMode(.tail).help(project.name)
                        HStack(spacing: 6) {
                            Text(verbatim: "You " + TimelineDrawing.ownerTime(project.owner)).foregroundStyle(AppPalette.owner)
                            Spacer(minLength: 0)
                            Text(verbatim: "Agents " + ProjectPresentation.duration(project.agents)).foregroundStyle(AppPalette.agents)
                        }
                        .font(.system(size: 11)).monospacedDigit().lineLimit(1)
                    }
                    .padding(.horizontal, 14).frame(maxWidth: .infinity, alignment: .leading).frame(height: 84)
                    .background(AppPalette.sidebar, in: RoundedRectangle(cornerRadius: 10))
                    .overlay(RoundedRectangle(cornerRadius: 10).stroke(AppPalette.cardBorder, lineWidth: 1))
                }
            }
        }
    }
}
