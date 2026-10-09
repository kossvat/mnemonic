import MnemonicShared
import SwiftUI

/// The host owns reads and mutations. Selection only opens local details.
struct TimelinePage: View {
    let day: String
    let snapshot: ProjectTimeline?
    let loading: Bool
    let failure: LocalAPIFailure?
    let mutationInFlight: Bool
    let message: String?
    let canUndo: Bool
    let now: Date
    let calendar: Calendar
    let onDay: (String) -> Void
    let onToday: () -> Void
    let onRefresh: () -> Void
    let onApply: @MainActor (AttributionCorrection, String) async -> Bool
    let onUndo: @MainActor () async -> Bool
    @State private var selection: TimelineSelection?
    @State private var undoing = false

    init(day: String, snapshot: ProjectTimeline?, loading: Bool, failure: LocalAPIFailure?,
         mutationInFlight: Bool, message: String?, canUndo: Bool, now: Date = Date(),
         calendar: Calendar = .current, onDay: @escaping (String) -> Void,
         onToday: @escaping () -> Void, onRefresh: @escaping () -> Void,
         onApply: @escaping @MainActor (AttributionCorrection, String) async -> Bool,
         onUndo: @escaping @MainActor () async -> Bool) {
        self.day = day; self.snapshot = snapshot; self.loading = loading; self.failure = failure
        self.mutationInFlight = mutationInFlight; self.message = message; self.canUndo = canUndo
        self.now = now; self.calendar = calendar; self.onDay = onDay; self.onToday = onToday
        self.onRefresh = onRefresh; self.onApply = onApply; self.onUndo = onUndo
    }

    private var shown: ProjectTimeline? { snapshot?.day == day ? snapshot : nil }

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.xl) {
                header
                if let failure {
                    let notice = ProjectPresentation.failure(failure)
                    Card { ProjectNotice(icon: "exclamationmark.circle", title: notice.title, detail: notice.detail) }
                }
                if let message {
                    Text(verbatim: message).font(Theme.Font.body).foregroundStyle(Theme.Palette.textMuted)
                }
                if let shown {
                    let drawing = TimelineDrawing(shown, calendar: calendar)
                    if drawing.valid {
                        timeline(drawing, snapshot: shown)
                        records(shown)
                    } else {
                        Card {
                            ProjectNotice(icon: "clock.badge.questionmark", title: "Timeline bounds unavailable",
                                          detail: "The response does not contain a valid day interval.")
                        }
                    }
                } else if !loading && failure == nil {
                    Card {
                        ProjectNotice(icon: "clock", title: "Timeline not loaded",
                                      detail: "Refresh to read the recorded activity for this day.")
                    }
                }
            }
            .padding(Theme.Space.xxl)
            .frame(maxWidth: 1_300, alignment: .leading)
            .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .background(Theme.Palette.bgPrimary)
        .environment(\.timeZone, calendar.timeZone)
        .sheet(item: $selection) { selected in
            TimelineCorrectionSheet(selection: selected, day: day, snapshot: shown,
                                    mutationInFlight: mutationInFlight, message: message, calendar: calendar,
                                    onApply: onApply)
        }
        .onChange(of: day) { _, _ in selection = nil }
        .onChange(of: snapshot) { _, updated in
            if let selected = selection, !selected.current(day: day, snapshot: updated) { selection = nil }
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: Theme.Space.lg) {
            HStack(alignment: .top) {
                VStack(alignment: .leading, spacing: Theme.Space.sm) {
                    Text(verbatim: "Timeline").font(Theme.Font.display).tracking(Theme.Font.trackingDisplay)
                        .foregroundStyle(Theme.Palette.textPrimary)
                    Text(verbatim: "Recorded agent work and estimated owner time.")
                        .font(Theme.Font.body).foregroundStyle(Theme.Palette.textMuted)
                }
                Spacer()
                if loading { ProgressView().controlSize(.small) }
            }
            HStack(spacing: Theme.Space.sm) {
                Button { moveDay(-1) } label: { Image(systemName: "chevron.left") }.help("Previous day")
                DatePicker("Day", selection: Binding(
                    get: { TimelineDay.date(day, calendar: calendar) ?? now },
                    set: { value in onDay(TimelineDay.string(value, calendar: calendar)) }), displayedComponents: .date)
                    .environment(\.calendar, calendar).labelsHidden().frame(width: 140)
                Button { moveDay(1) } label: { Image(systemName: "chevron.right") }.help("Next day")
                Button("Today", action: onToday)
                Spacer()
                Button("Refresh", action: onRefresh).disabled(loading || mutationInFlight)
                Button("Undo correction") { undo() }.disabled(!canUndo || mutationInFlight || undoing)
            }
            .buttonStyle(SubtleButtonStyle())
        }
    }

    private func timeline(_ drawing: TimelineDrawing, snapshot: ProjectTimeline) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            HStack(alignment: .firstTextBaseline) {
                Text(verbatim: "\(day) · \(String(format: "%g", drawing.duration / 3_600)) hour day")
                    .font(Theme.Font.title).foregroundStyle(Theme.Palette.textPrimary)
                Spacer()
                Text(verbatim: calendar.timeZone.identifier).font(Theme.Font.caption)
                    .foregroundStyle(Theme.Palette.textMuted)
            }
            HStack(spacing: Theme.Space.lg) {
                Label { Text(verbatim: "Claude · rounded") } icon: { Capsule().fill(Theme.Palette.feedback).frame(width: 18, height: 8) }
                Label { Text(verbatim: "Codex · square") } icon: { Rectangle().fill(Theme.Palette.accent).frame(width: 18, height: 8) }
                Text(verbatim: "Points have no elapsed duration")
            }
            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            TimelineLanes(drawing: drawing, snapshot: snapshot, calendar: calendar, onSelect: { selected in selection = selected })
            if snapshot.segments.isEmpty {
                ProjectNotice(icon: "moon", title: "No agent activity recorded",
                              detail: snapshot.owner.isEmpty ? "No agent segments are recorded for this day."
                                : "Owner activity is shown independently of agent transcripts.")
            }
            Text(verbatim: "Owner attribution is estimated. Stacked shares cover the same interval; they are not separate periods of work.")
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
        }
    }

    @ViewBuilder private func records(_ snapshot: ProjectTimeline) -> some View {
        if !snapshot.segments.isEmpty || !snapshot.owner.isEmpty {
            VStack(alignment: .leading, spacing: Theme.Space.md) {
                Text(verbatim: "Recorded intervals").font(Theme.Font.title)
                Text(verbatim: "Open a record to inspect its exact bounds or apply an attribution correction.")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                ForEach(snapshot.segments, id: \.id) { segment in
                    record(.agent(day: snapshot.day, segment), title: TimelineDrawing.agent(segment.agent),
                           subtitle: projectName(segment.project, snapshot: snapshot))
                }
                ForEach(snapshot.owner, id: \.id) { range in
                    let assignments = range.assignments.map { share in
                        projectName(share.project, snapshot: snapshot) + " " + String(format: "%.0f%%", share.share * 100)
                    }.joined(separator: " / ")
                    record(.owner(day: snapshot.day, range), title: "Owner estimate", subtitle: assignments)
                }
            }
        }
    }

    private func record(_ selected: TimelineSelection, title: String, subtitle: String) -> some View {
        Button { selection = selected } label: {
            HStack(alignment: .top, spacing: Theme.Space.lg) {
                VStack(alignment: .leading, spacing: 3) {
                    Text(verbatim: title).font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary)
                    Text(verbatim: subtitle).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                }
                Spacer()
                VStack(alignment: .trailing, spacing: 3) {
                    Text(verbatim: TimelineDrawing.time(selected.from, calendar: calendar) +
                         (selected.point ? " · point" : " – " + TimelineDrawing.time(selected.to, calendar: calendar)))
                        .font(Theme.Font.mono).foregroundStyle(Theme.Palette.textMuted)
                    if !selected.exactRange && selected.stream == nil {
                        Text(verbatim: "Fractional bounds · view only").font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    }
                }
                Image(systemName: "chevron.right").font(.system(size: 10)).foregroundStyle(Theme.Palette.textSubtle)
            }
            .padding(Theme.Space.md).background(Theme.Palette.bgSurface)
            .clipShape(RoundedRectangle(cornerRadius: Theme.Radius.sm))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func projectName(_ key: String?, snapshot: ProjectTimeline) -> String {
        key.flatMap { key in snapshot.projects.first(where: { project in project.key == key })?.name ?? key } ?? "Unassigned"
    }
    private func moveDay(_ amount: Int) {
        guard let date = TimelineDay.date(day, calendar: calendar),
              let next = calendar.date(byAdding: .day, value: amount, to: date) else { return }
        onDay(TimelineDay.string(next, calendar: calendar))
    }
    private func undo() {
        guard canUndo, !mutationInFlight, !undoing else { return }
        undoing = true
        Task { @MainActor in
            _ = await onUndo()
            undoing = false
        }
    }
}
