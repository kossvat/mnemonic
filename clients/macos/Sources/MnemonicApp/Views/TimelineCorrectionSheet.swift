import MnemonicShared
import SwiftUI

struct TimelineCorrectionSheet: View {
    let selection: TimelineSelection
    let day: String
    let snapshot: ProjectTimeline?
    let mutationInFlight: Bool
    let message: String?
    let calendar: Calendar
    let onApply: @MainActor (AttributionCorrection, String) async -> Bool
    @Environment(\.dismiss) private var dismiss
    @State private var project: String?
    @State private var transcript = false
    @State private var submitting = false

    init(selection: TimelineSelection, day: String, snapshot: ProjectTimeline?, mutationInFlight: Bool,
         message: String?, calendar: Calendar,
         onApply: @escaping @MainActor (AttributionCorrection, String) async -> Bool) {
        self.selection = selection
        self.day = day
        self.snapshot = snapshot
        self.mutationInFlight = mutationInFlight
        self.message = message
        self.calendar = calendar
        self.onApply = onApply
        _project = State(initialValue: selection.project)
    }

    private var current: Bool { selection.current(day: day, snapshot: snapshot) }
    private var busy: Bool { mutationInFlight || submitting }
    private var correction: AttributionCorrection? {
        selection.correction(project: project, transcript: transcript, day: day, snapshot: snapshot)
    }
    private var projects: [TimelineProject] {
        var values = snapshot?.projects ?? []
        if let original = selection.project, !values.contains(where: { value in value.key == original }) {
            values.append(TimelineProject(key: original, name: original))
        }
        return values.sorted { left, right in left.name == right.name ? left.key < right.key : left.name < right.name }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.lg) {
            Text(verbatim: "Timeline details").font(Theme.Font.heading)
                .foregroundStyle(Theme.Palette.textPrimary)
            details
            Divider()
            if !current {
                ProjectNotice(icon: "arrow.clockwise", title: "This selection is out of date",
                              detail: "Close these details and select the current block before applying a correction.")
            } else if !selection.exactRange && selection.stream == nil {
                ProjectNotice(icon: "info.circle", title: "Fractional range · view only",
                              detail: "This estimate includes fractions of a second. Corrections store whole-second bounds, so this range cannot be changed here without changing its duration.")
            } else {
                correctionFields
            }
            if let message {
                Text(verbatim: message).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    .textSelection(.enabled)
            }
            HStack {
                Button("Close") { dismiss() }.keyboardShortcut(.cancelAction)
                Spacer()
                if busy { ProgressView().controlSize(.small) }
                Button { apply() } label: { Text(verbatim: transcript ? "Apply transcript pin" : "Apply") }
                    .buttonStyle(.borderedProminent)
                    .disabled(busy || correction == nil)
            }
        }
        .padding(Theme.Space.xl)
        .frame(width: 510)
        .background(Theme.Palette.bgPrimary)
    }

    private var details: some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            switch selection {
            case .agent(_, let segment):
                Text(verbatim: TimelineDrawing.agent(segment.agent)).font(Theme.Font.title)
                    .foregroundStyle(Theme.Palette.textPrimary)
                Text(verbatim: segment.driver == "owner" ? "Owner-led agent session" :
                        segment.driver == "program" ? "Automated agent session" : "Unknown session driver")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            case .owner(_, let range):
                Text(verbatim: "Owner attribution estimate").font(Theme.Font.title)
                ForEach(Array(range.assignments.enumerated()), id: \.offset) { _, assignment in
                    let name = assignment.project.flatMap { key in
                        snapshot?.projects.first(where: { value in value.key == key })?.name ?? key
                    } ?? "Unassigned"
                    Text(verbatim: "\(name) · \(share(assignment.share)) · \(assignment.confidence ?? "unknown confidence")")
                        .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                }
                Text(verbatim: "These shares cover the same interval concurrently.")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
            Text(verbatim: selection.day).font(Theme.Font.mono).foregroundStyle(Theme.Palette.textMuted)
            Text(verbatim: TimelineDrawing.time(selection.from, calendar: calendar) +
                 (selection.point ? " · point, no elapsed duration" : " – " + TimelineDrawing.time(selection.to, calendar: calendar)))
                .font(Theme.Font.body).foregroundStyle(Theme.Palette.textPrimary)
            if !selection.point {
                Text(verbatim: "Recorded duration: \(ProjectPresentation.duration(selection.to - selection.from))")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
        }
    }

    private var correctionFields: some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            Text(verbatim: "Correct project attribution").font(Theme.Font.title)
            Picker("Project", selection: $project) {
                Text(verbatim: "Unassigned").tag(Optional<String>.none)
                ForEach(projects, id: \.key) { value in
                    Text(verbatim: value.name).tag(Optional(value.key))
                }
            }
            .disabled(busy)
            if selection.stream != nil {
                Toggle("Pin the entire transcript to this project", isOn: $transcript)
                    .disabled(busy)
                Text(verbatim: transcript
                     ? "This changes attribution for the whole transcript, including activity outside this day."
                     : selection.point
                        ? "A point has no elapsed duration. Select the whole-transcript pin explicitly to change its attribution."
                        : selection.exactRange
                            ? "Apply changes only the recorded interval shown above."
                            : "This fractional interval cannot be edited as a range. A whole-transcript pin is a separate choice.")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            } else {
                Text(verbatim: "Apply assigns the entire recorded interval to the selected project. Choosing Unassigned removes inferred attribution for this interval.")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
            Text(verbatim: "Selecting a project does not save a change. Use Apply to confirm; Undo is available after a successful correction.")
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
        }
    }

    private func share(_ value: Double) -> String {
        value.isFinite ? String(format: "%.0f%%", value * 100) : "Unknown share"
    }

    private func apply() {
        guard !busy, let correction else { return }
        submitting = true
        Task { @MainActor in
            let applied = await onApply(correction, selection.day)
            submitting = false
            if applied { dismiss() }
        }
    }
}
