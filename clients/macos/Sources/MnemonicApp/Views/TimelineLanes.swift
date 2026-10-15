import MnemonicShared
import SwiftUI

struct TimelineLanes: View {
    let drawing: TimelineDrawing
    let snapshot: ProjectTimeline
    let calendar: Calendar
    let onSelect: (TimelineSelection) -> Void
    @State private var expanded: Set<String> = []

    var body: some View {
        let lanes = drawing.presentedLanes(expanded: expanded)
        ScrollView(.horizontal) {
            VStack(alignment: .leading, spacing: 0) {
                HStack(spacing: TimelineDrawing.labelGap) {
                    Text("PROJECT / AGENT").font(.system(size: 10)).tracking(0.6)
                        .frame(width: TimelineDrawing.labelWidth, alignment: .leading)
                    TimelineAxis(drawing: drawing)
                }
                .foregroundStyle(AppPalette.textSecondary).frame(height: 32)
                owner
                ForEach(lanes) { presented in
                    let lane = presented.lane
                    HStack(spacing: TimelineDrawing.labelGap) {
                        VStack(alignment: .leading, spacing: 3) {
                            Text(verbatim: lane.name).font(.system(size: 12, weight: .semibold))
                                .foregroundStyle(AppPalette.textPrimary)
                            if !lane.agents.isEmpty { Text(verbatim: lane.agents).font(.system(size: 10)) }
                            if lane.count > 0 { Text(verbatim: TimelineDrawing.segmentCount(lane.count)).font(.system(size: 10)) }
                        }
                        .lineLimit(1).truncationMode(.tail).foregroundStyle(AppPalette.textSecondary)
                        .frame(width: TimelineDrawing.labelWidth, alignment: .leading)
                        .help(lane.name + "\n" + lane.agents)
                        VStack(spacing: 0) {
                            if let message = lane.emptyMessage {
                                ownerNotice(message).frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .leading)
                            } else {
                                ForEach(presented.rows.indices, id: \.self) { index in agentRow(presented.rows[index]) }
                            }
                            if let label = presented.toggleLabel {
                                Button {
                                    if expanded.contains(lane.id) { expanded.remove(lane.id) }
                                    else { expanded.insert(lane.id) }
                                } label: {
                                    Text(verbatim: label).font(.system(size: 11))
                                        .frame(maxWidth: .infinity, minHeight: 24, alignment: .leading)
                                }
                                .buttonStyle(.plain).foregroundStyle(AppPalette.agents).padding(.leading, 8)
                            }
                        }
                        .frame(width: drawing.width, height: presented.height)
                        .background { TimelineGrid(drawing: drawing) }
                    }
                    .overlay(alignment: .bottom) { Rectangle().fill(AppPalette.cardBorder).frame(height: 1) }
                }
            }
            .frame(width: TimelineDrawing.labelWidth + TimelineDrawing.labelGap + drawing.width)
        }
        .frame(height: TimelineDrawing.presentedHeight(lanes))
    }

    private func agentRow(_ blocks: [TimelineDrawing.Block]) -> some View {
        ZStack(alignment: .topLeading) {
            ForEach(blocks) { block in
                TimelineAgentMark(mark: block.mark, geometry: block.geometry)
                    .frame(width: block.geometry.paint.width, height: block.geometry.paintHeight)
                    .offset(x: block.geometry.paint.x, y: block.geometry.paintY)
                    .help(description(block))
                    .accessibilityLabel(Text(verbatim: description(block)))
                    .accessibilityAddTraits(.isButton)
                    .accessibilityAction { onSelect(.agent(day: snapshot.day, block.segment)) }
                if let caption = block.caption {
                    Text(verbatim: caption.text).font(.system(size: 10)).fixedSize()
                        .foregroundStyle(AppPalette.textPrimary)
                        .frame(width: caption.rect.width, height: block.geometry.paintHeight)
                        .clipped()
                        .offset(x: caption.rect.x, y: block.geometry.paintY).accessibilityHidden(true)
                }
            }
        }
        .allowsHitTesting(false)
        .frame(width: drawing.width, height: TimelineDrawing.rowHeight, alignment: .topLeading)
        .clipped()
        .overlay {
            Color.clear.contentShape(Rectangle())
                .onTapGesture(coordinateSpace: .local) { location in
                    if let segment = drawing.segment(at: location.x, in: blocks) {
                        onSelect(.agent(day: snapshot.day, segment))
                    }
                }
                .accessibilityHidden(true)
        }
    }

    private var owner: some View {
        HStack(spacing: TimelineDrawing.labelGap) {
            Text("You at the Mac").font(.system(size: 12, weight: .semibold)).foregroundStyle(AppPalette.owner)
                .frame(width: TimelineDrawing.labelWidth, alignment: .leading)
            ZStack(alignment: .leading) {
                TimelineGrid(drawing: drawing)
                ForEach(drawing.owner) { block in
                    RoundedRectangle(cornerRadius: 3).fill(AppPalette.owner)
                        .frame(width: block.geometry.paint.width, height: block.geometry.paintHeight)
                        .overlay { TimelineContinuation(geometry: block.geometry, color: AppPalette.window) }
                        .offset(x: block.geometry.paint.x)
                        .accessibilityLabel(Text(verbatim: "Owner estimate, " + TimelineDrawing.time(block.range.from, calendar: calendar)
                            + " to " + TimelineDrawing.time(block.range.to, calendar: calendar)))
                        .accessibilityAddTraits(.isButton)
                        .accessibilityAction { onSelect(.owner(day: snapshot.day, block.range)) }
                }
                if let notice = TimelineDrawing.ownerNotice(drawing, snapshot: snapshot) { ownerNotice(notice) }
            }
            .allowsHitTesting(false)
            .frame(width: drawing.width, height: TimelineDrawing.ownerHeight).clipped()
            .overlay {
                Color.clear.contentShape(Rectangle())
                    .onTapGesture(coordinateSpace: .local) { location in
                        if let range = drawing.ownerRange(at: location.x) { onSelect(.owner(day: snapshot.day, range)) }
                    }
                    .accessibilityHidden(true)
            }
        }
        .overlay(alignment: .bottom) { Rectangle().fill(AppPalette.cardBorder).frame(height: 1) }
    }

    private func ownerNotice(_ text: String) -> some View {
        Text(verbatim: text).font(.system(size: 11)).foregroundStyle(AppPalette.textSecondary)
            .lineLimit(1).truncationMode(.tail).padding(.leading, 8)
    }

    private func description(_ block: TimelineDrawing.Block) -> String {
        block.label + ", " + TimelineDrawing.time(block.segment.from, calendar: calendar)
            + (block.point ? ", point with no elapsed duration" : " to " + TimelineDrawing.time(block.segment.to, calendar: calendar))
            + (block.geometry.continuesBefore ? ". Continues before this span" : "")
            + (block.geometry.continuesAfter ? ". Continues after this span" : "") + ". Open details."
    }
}

struct TimelineAxis: View {
    let drawing: TimelineDrawing

    var body: some View {
        ZStack(alignment: .topLeading) {
            ForEach(drawing.axisTicks) { tick in
                let rect = TimelineDrawing.tickLabel(tick, width: drawing.width)
                Text(verbatim: tick.label).font(.system(size: 10)).lineLimit(1)
                    .frame(width: rect.width, alignment: tick.position == 0 ? .leading : tick.position == 1 ? .trailing : .center)
                    .clipped()
                    .offset(x: rect.x)
            }
        }
        .frame(width: drawing.width, height: 14, alignment: .topLeading)
        .foregroundStyle(AppPalette.textSecondary)
    }
}

struct TimelineGrid: View {
    let drawing: TimelineDrawing
    var body: some View {
        GeometryReader { geometry in
            Path { path in
                for tick in drawing.ticks.filter(\.major) {
                    let x = tick.position * drawing.width
                    path.move(to: CGPoint(x: x, y: 0))
                    path.addLine(to: CGPoint(x: x, y: geometry.size.height))
                }
            }
            .stroke(AppPalette.timelineGrid, lineWidth: 1)
        }
        .accessibilityHidden(true)
    }
}
