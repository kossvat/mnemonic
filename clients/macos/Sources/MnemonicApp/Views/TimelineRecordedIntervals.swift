import MnemonicShared
import SwiftUI

struct TimelineRecordedIntervals: View {
    let snapshot: ProjectTimeline
    let calendar: Calendar
    let onSelect: (TimelineSelection) -> Void
    let records: TimelineDrawing.RecordedIntervals
    @State private var expanded: Bool

    init(snapshot: ProjectTimeline, calendar: Calendar, onSelect: @escaping (TimelineSelection) -> Void) {
        self.snapshot = snapshot; self.calendar = calendar; self.onSelect = onSelect
        let records = TimelineDrawing.recordedIntervals(snapshot)
        self.records = records
        _expanded = State(initialValue: records.initiallyExpanded)
    }

    var body: some View {
        if !records.selections.isEmpty {
            DisclosureGroup(isExpanded: $expanded) {
                VStack(alignment: .leading, spacing: 10) {
                    Text("Open a record to inspect its exact bounds or apply an attribution correction.")
                        .font(.system(size: 11)).foregroundStyle(AppPalette.textSecondary)
                    ForEach(records.selections) { selected in
                        switch selected {
                        case .agent(_, let segment):
                            record(selected, title: TimelineDrawing.agent(segment.agent),
                                   subtitle: TimelineDrawing.projectName(segment.project, snapshot: snapshot))
                        case .owner(_, let range):
                            let assignments = range.assignments.map { share in
                                TimelineDrawing.projectName(share.project, snapshot: snapshot) + " " + String(format: "%.0f%%", share.share * 100)
                            }.joined(separator: " / ")
                            record(selected, title: "Owner estimate", subtitle: assignments)
                        }
                    }
                }
                .padding(.top, 10)
            } label: { Text(verbatim: records.title).font(.system(size: 14, weight: .semibold)) }
            .tint(AppPalette.textSecondary)
        }
    }

    private func record(_ selected: TimelineSelection, title: String, subtitle: String) -> some View {
        Button { onSelect(selected) } label: {
            HStack(alignment: .top, spacing: 16) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: title).font(.system(size: 12, weight: .medium)).foregroundStyle(AppPalette.textPrimary)
                    Text(verbatim: subtitle).font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
                }
                .lineLimit(1).truncationMode(.tail)
                Spacer(minLength: 0)
                VStack(alignment: .trailing, spacing: 3) {
                    Text(verbatim: TimelineDrawing.time(selected.from, calendar: calendar) +
                         (selected.point ? " · point" : " – " + TimelineDrawing.time(selected.to, calendar: calendar)))
                        .font(.system(size: 11, design: .monospaced)).foregroundStyle(AppPalette.textSecondary)
                    if !selected.exactRange && selected.stream == nil {
                        Text("Fractional bounds · view only").font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
                    }
                }
                Image(systemName: "chevron.right").font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
            }
            .padding(12).background(AppPalette.sidebar, in: RoundedRectangle(cornerRadius: 9))
            .overlay(RoundedRectangle(cornerRadius: 9).stroke(AppPalette.cardBorder, lineWidth: 1))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}
