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
    let live: LiveSnapshot?
    let registry: ProjectBoard?
    let laneWidth: Double?
    let onDay: (String) -> Void
    let onToday: () -> Void
    let onRefresh: () -> Void
    let onApply: @MainActor (AttributionCorrection, String) async -> Bool
    let onUndo: @MainActor () async -> Bool
    @State private var selection: TimelineSelection?
    @State private var undoing = false
    @State private var showCalendar = false
    @State private var scale: TimelineDrawing.Scale
    @State private var focus: TimelineDrawing.Span?

    init(day: String, snapshot: ProjectTimeline?, loading: Bool, failure: LocalAPIFailure?,
         mutationInFlight: Bool, message: String?, canUndo: Bool, now: Date = Date(),
         calendar: Calendar = .current, live: LiveSnapshot? = nil, registry: ProjectBoard? = nil,
         initialScale: TimelineDrawing.Scale = .focus, initialFocus: TimelineDrawing.Span? = nil,
         laneWidth: Double? = nil, onDay: @escaping (String) -> Void,
         onToday: @escaping () -> Void, onRefresh: @escaping () -> Void,
         onApply: @escaping @MainActor (AttributionCorrection, String) async -> Bool,
         onUndo: @escaping @MainActor () async -> Bool) {
        self.day = day; self.snapshot = snapshot; self.loading = loading; self.failure = failure
        self.mutationInFlight = mutationInFlight; self.message = message; self.canUndo = canUndo
        self.now = now; self.calendar = calendar; self.live = live; self.laneWidth = laneWidth
        self.registry = registry
        self.onDay = onDay; self.onToday = onToday; self.onRefresh = onRefresh
        self.onApply = onApply; self.onUndo = onUndo
        _scale = State(initialValue: initialScale); _focus = State(initialValue: initialFocus)
    }

    private var shown: ProjectTimeline? { failure == nil && snapshot?.day == day ? snapshot : nil }

    var body: some View {
        GeometryReader { geometry in
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    header
                    if let failure {
                        let notice = ProjectPresentation.failure(failure)
                        panel(notice.title, detail: notice.detail)
                    }
                    if let message {
                        Text(verbatim: message).font(.system(size: 12)).foregroundStyle(AppPalette.textSecondary)
                    }
                    if let shown {
                        let daySpan = TimelineDrawing.Span(from: shown.from, to: shown.to)
                        let focusSpan = focus.map { span in TimelineDrawing.focus(endingAt: span.to, day: daySpan) }
                            ?? TimelineDrawing.defaultFocus(shown, now: now, calendar: calendar)
                        let shownScale = TimelineDrawing.displayScale(scale, focus: focusSpan, snapshot: shown)
                        let drawing = TimelineDrawing(shown, span: shownScale == .focus ? focusSpan : daySpan,
                            width: laneWidth ?? TimelineDrawing.laneWidth(pageWidth: geometry.size.width),
                            registry: registry, scale: shownScale, calendar: calendar)
                        if drawing.valid {
                            timeline(drawing, snapshot: shown, focus: focusSpan)
                            TimelineRecordedIntervals(snapshot: shown, calendar: calendar,
                                onSelect: { selected in selection = selected }).id(shown.day)
                        } else {
                            panel("Timeline bounds unavailable", detail: "The response does not contain a valid day interval.")
                        }
                    } else if loading && failure == nil {
                        panel("Loading timeline", detail: "Reading recorded activity for this day.")
                    } else if failure == nil {
                        panel("Timeline not loaded", detail: "Refresh to read the recorded activity for this day.")
                    }
                }
                .foregroundStyle(AppPalette.textPrimary)
                .padding(.horizontal, 28).padding(.vertical, 24)
                .frame(maxWidth: .infinity, alignment: .topLeading)
            }
        }
        .background(AppPalette.window)
        .environment(\.timeZone, calendar.timeZone)
        .sheet(item: $selection) { selected in
            TimelineCorrectionSheet(selection: selected, day: day, snapshot: shown,
                                    mutationInFlight: mutationInFlight, message: message, calendar: calendar,
                                    onApply: onApply)
        }
        .onChange(of: day) { _, _ in selection = nil; focus = nil }
        .onChange(of: snapshot) { _, updated in
            if let selected = selection, !selected.current(day: day, snapshot: updated) { selection = nil }
        }
    }

    private var header: some View {
        VStack(spacing: 8) {
            HStack(spacing: 20) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("Timeline").font(.system(size: 28, weight: .semibold)).tracking(-0.9)
                    Text("Your time and agent work, across projects").font(.system(size: 11))
                        .foregroundStyle(AppPalette.textSecondary)
                }
                Spacer(minLength: 0)
                HStack(spacing: 8) {
                    Button { moveDay(-1) } label: { Image(systemName: "chevron.left").frame(width: 18) }
                        .help("Previous day").accessibilityLabel("Previous day")
                    Button { showCalendar.toggle() } label: {
                        Text(verbatim: ProjectPresentation.boardDate(TimelineDay.date(day, calendar: calendar) ?? now,
                                                                     calendar: calendar, format: "EEEE, d MMMM yyyy"))
                            .lineLimit(1).fixedSize().frame(minWidth: 158)
                    }
                    .popover(isPresented: $showCalendar) {
                        DatePicker("Day", selection: Binding(
                            get: { TimelineDay.date(day, calendar: calendar) ?? now },
                            set: { value in onDay(TimelineDay.string(value, calendar: calendar)); showCalendar = false }),
                                   displayedComponents: .date)
                            .datePickerStyle(.graphical).environment(\.calendar, calendar).padding(16)
                    }
                    Button { moveDay(1) } label: { Image(systemName: "chevron.right").frame(width: 18) }
                        .help("Next day").accessibilityLabel("Next day")
                }
                .buttonStyle(TimelineButtonStyle())
            }
            .frame(minHeight: 54)
            HStack(spacing: 12) {
                if loading { ProgressView().controlSize(.small) }
                Text(verbatim: calendar.timeZone.identifier).font(.system(size: 10))
                    .foregroundStyle(AppPalette.textSecondary)
                Spacer()
                Button("Today") { focus = nil; onToday() }
                Button("Refresh", action: onRefresh).disabled(loading || mutationInFlight)
                Button("Undo correction") { undo() }.disabled(!canUndo || mutationInFlight || undoing)
            }
            .font(.system(size: 11)).buttonStyle(.borderless).tint(AppPalette.agents)
        }
    }

    private func timeline(_ drawing: TimelineDrawing, snapshot: ProjectTimeline,
                          focus: TimelineDrawing.Span) -> some View {
        let totals = TimelineDrawing.totals(snapshot)
        return VStack(alignment: .leading, spacing: 16) {
            TimelineOverview(snapshot: snapshot, focus: focus, scale: drawing.scale, now: now, calendar: calendar,
                             onScale: { chosen in scale = chosen },
                             onFocus: { chosen in self.focus = chosen; scale = .focus })
            HStack(alignment: .top, spacing: 18) {
                VStack(alignment: .leading, spacing: 10) {
                    TimelineLanes(drawing: drawing, snapshot: snapshot, calendar: calendar,
                                  onSelect: { selected in selection = selected }).id(snapshot.day)
                    if let empty = TimelineDrawing.emptyMessage(snapshot) {
                        Text(verbatim: empty).font(.system(size: 12)).foregroundStyle(AppPalette.textSecondary)
                    }
                    Text("Each block is one segment of work. A session that changes project starts a new block.")
                        .font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                TimelineDayTotals(totals: totals, date: Date(timeIntervalSince1970: snapshot.from), calendar: calendar)
            }
            if !totals.projects.isEmpty { TimelineProjectTimes(totals: totals) }
            HStack(alignment: .top, spacing: 12) {
                Text("Day totals include activity outside the focus.")
                Spacer(minLength: 0)
                if let count = TimelineDrawing.liveCount(live) { Text(verbatim: count) }
            }
            .font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
        }
    }

    private func panel(_ title: String, detail: String) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(verbatim: title).font(.system(size: 14, weight: .semibold))
            Text(verbatim: detail).font(.system(size: 12)).foregroundStyle(AppPalette.textSecondary)
        }
        .frame(maxWidth: .infinity, alignment: .leading).padding(14)
        .background(AppPalette.raised, in: RoundedRectangle(cornerRadius: 12))
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

private struct TimelineButtonStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.font(.system(size: 12)).foregroundStyle(AppPalette.textPrimary)
            .padding(.horizontal, 13).frame(height: 44)
            .background(configuration.isPressed ? AppPalette.activeRow : AppPalette.raised,
                        in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).stroke(AppPalette.cardBorder, lineWidth: 1))
    }
}
