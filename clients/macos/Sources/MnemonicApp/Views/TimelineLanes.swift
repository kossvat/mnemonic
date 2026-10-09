import MnemonicShared
import SwiftUI

struct TimelineLanes: View {
    let drawing: TimelineDrawing
    let snapshot: ProjectTimeline
    let calendar: Calendar
    let onSelect: (TimelineSelection) -> Void
    private let labelWidth: CGFloat = 150

    private var height: CGFloat {
        52 + drawing.lanes.reduce(0) { total, lane in total + CGFloat(lane.tracks) * 48 + 20 } + 96
    }

    var body: some View {
        GeometryReader { geometry in
            let axisWidth = max(660, geometry.size.width - labelWidth - 20)
            ScrollView(.horizontal) {
                VStack(alignment: .leading, spacing: 0) {
                    axis(width: axisWidth)
                    ForEach(drawing.lanes) { lane in
                        HStack(alignment: .top, spacing: 0) {
                            VStack(alignment: .leading, spacing: 4) {
                                Text(verbatim: lane.name).font(Theme.Font.bodyMedium).lineLimit(2)
                                Text(verbatim: "\(lane.blocks.count) recorded segments")
                                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                            }
                            .frame(width: labelWidth, alignment: .leading).padding(.top, 8)
                            ZStack(alignment: .topLeading) {
                                grid(width: axisWidth)
                                ForEach(lane.blocks) { block in agent(block, width: axisWidth) }
                            }
                            .frame(width: axisWidth, height: CGFloat(lane.tracks) * 48 + 12)
                        }
                        .padding(.bottom, 8)
                    }
                    owner(width: axisWidth)
                }
                .frame(width: labelWidth + axisWidth + 20, alignment: .leading)
            }
        }
        .frame(height: height)
    }

    private func axis(width: CGFloat) -> some View {
        HStack(spacing: 0) {
            Text(verbatim: "Local time").font(Theme.Font.captionBold)
                .foregroundStyle(Theme.Palette.textMuted).frame(width: labelWidth, alignment: .leading)
            ZStack(alignment: .topLeading) {
                ForEach(drawing.ticks.filter(\.major)) { tick in
                    Text(verbatim: tick.label).font(Theme.Font.mono).foregroundStyle(Theme.Palette.textMuted)
                        .fixedSize().offset(x: CGFloat(tick.position) * width - (tick.position > 0.98 ? 40 : 0), y: 10)
                }
            }
            .frame(width: width, height: 48, alignment: .topLeading)
        }
    }

    private func grid(width: CGFloat) -> some View {
        GeometryReader { geometry in
            Path { path in
                for tick in drawing.ticks {
                    let x = CGFloat(tick.position) * width
                    path.move(to: CGPoint(x: x, y: 0))
                    path.addLine(to: CGPoint(x: x, y: geometry.size.height))
                }
            }
            .stroke(Theme.Palette.border.opacity(0.65), style: StrokeStyle(lineWidth: 0.5, dash: [3, 4]))
        }
        .accessibilityHidden(true)
    }

    private func agent(_ block: TimelineDrawing.Block, width: CGFloat) -> some View {
        let length = CGFloat(block.length) * width
        let title = block.label + (block.point ? " · point" : "")
        return Button { onSelect(.agent(day: snapshot.day, block.segment)) } label: {
            VStack(alignment: .leading, spacing: 5) {
                Text(verbatim: title).font(Theme.Font.captionBold).foregroundStyle(color(block.mark))
                    .fixedSize()
                if block.point {
                    Image(systemName: block.mark == .codex ? "diamond.fill" : block.mark == .claude ? "circle.fill" : "triangle.fill")
                        .font(.system(size: 10)).foregroundStyle(color(block.mark)).offset(x: -5)
                } else {
                    mark(block.mark).frame(width: length, height: 12)
                }
            }
            .frame(width: max(length, 90), height: 42, alignment: .topLeading)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .offset(x: CGFloat(block.position) * width, y: CGFloat(block.track) * 48 + 5)
        .help(segmentDescription(block))
        .accessibilityLabel(Text(verbatim: segmentDescription(block)))
    }

    private func owner(width: CGFloat) -> some View {
        HStack(alignment: .top, spacing: 0) {
            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: "Owner").font(Theme.Font.bodyMedium)
                Text(verbatim: "Estimated attribution").font(Theme.Font.caption)
                    .foregroundStyle(Theme.Palette.textMuted)
            }
            .frame(width: labelWidth, alignment: .leading).padding(.top, 10)
            ZStack(alignment: .topLeading) {
                grid(width: width)
                if !drawing.ownerAvailable {
                    Text(verbatim: "Owner activity unavailable · time is unknown")
                        .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted).padding(.top, 20)
                } else if drawing.owner.isEmpty {
                    Text(verbatim: "No owner activity recorded for this day")
                        .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted).padding(.top, 20)
                } else {
                    ForEach(drawing.owner) { block in
                        Button { onSelect(.owner(day: snapshot.day, block.range)) } label: {
                            VStack(spacing: 0) {
                                ForEach(Array(block.range.assignments.enumerated()), id: \.offset) { index, share in
                                    Rectangle().fill(ownerColor(index, project: share.project))
                                        .frame(height: share.share.isFinite ? CGFloat(max(0, min(1, share.share))) * 24 : 0)
                                }
                            }
                            .frame(width: CGFloat(block.length) * width, height: 24, alignment: .top)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(.plain)
                        .offset(x: CGFloat(block.position) * width, y: 18)
                        .help(ownerDescription(block.range))
                        .accessibilityLabel(Text(verbatim: ownerDescription(block.range)))
                    }
                }
            }
            .frame(width: width, height: 88)
        }
        .overlay(alignment: .top) { Rectangle().fill(Theme.Palette.border).frame(height: 1) }
    }

    @ViewBuilder private func mark(_ value: TimelineDrawing.AgentMark) -> some View {
        switch value {
        case .claude: Capsule().fill(color(value))
        case .codex: Rectangle().fill(color(value))
        case .unknown:
            RoundedRectangle(cornerRadius: 2).strokeBorder(color(value), style: StrokeStyle(lineWidth: 1.5, dash: [3, 2]))
        }
    }

    private func color(_ mark: TimelineDrawing.AgentMark) -> Color {
        switch mark { case .claude: Theme.Palette.feedback; case .codex: Theme.Palette.accent; case .unknown: Theme.Palette.textMuted }
    }
    private func ownerColor(_ index: Int, project: String?) -> Color {
        guard project != nil else { return Theme.Palette.textSubtle.opacity(0.5) }
        return index % 2 == 0 ? Theme.Palette.decision.opacity(0.8) : Theme.Palette.note.opacity(0.8)
    }
    private func ownerNames(_ range: TimelineOwnerRange) -> String {
        range.assignments.map { assignment in
            assignment.project.flatMap { key in snapshot.projects.first(where: { project in project.key == key })?.name ?? key }
                ?? "Unassigned"
        }.joined(separator: " / ")
    }
    private func segmentDescription(_ block: TimelineDrawing.Block) -> String {
        let start = TimelineDrawing.time(block.segment.from, calendar: calendar)
        if block.point { return "\(block.label), \(start), point with no elapsed duration. Open details." }
        return "\(block.label), \(start) to \(TimelineDrawing.time(block.segment.to, calendar: calendar)). Open details."
    }
    private func ownerDescription(_ range: TimelineOwnerRange) -> String {
        "Owner estimate: \(ownerNames(range)), \(TimelineDrawing.time(range.from, calendar: calendar)) to \(TimelineDrawing.time(range.to, calendar: calendar)). Shares cover the same interval. Open details."
    }
}
