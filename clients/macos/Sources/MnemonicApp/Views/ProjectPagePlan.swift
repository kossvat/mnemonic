import SwiftUI

struct ProjectPageFocus: View {
    let unit: ProjectPresentation.PageUnit
    var body: some View {
        VStack(alignment: .leading, spacing: 13) {
            HStack(spacing: 22) {
                VStack(alignment: .leading, spacing: 3) {
                    Text("ACTIVE CHANGE").font(.system(size: 10)).tracking(1.1).foregroundStyle(AppPalette.textSecondary)
                    Text(verbatim: unit.title).font(.system(size: 23, weight: .semibold)).tracking(-0.6)
                        .lineLimit(1).truncationMode(.tail)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                if let progress = unit.progress {
                    HStack(alignment: .firstTextBaseline, spacing: 12) {
                        Text(verbatim: progress.percent).font(.system(size: 43, weight: .semibold)).tracking(-1.8)
                        Text(verbatim: progress.tasks).font(.system(size: 12)).foregroundStyle(AppPalette.textSecondary)
                    }
                    .fixedSize()
                }
            }
            if let progress = unit.progress { ProjectPageBar(progress: progress) }
            if let weekly = unit.weekly {
                HStack(spacing: 10) {
                    Text(verbatim: weekly).foregroundStyle(AppPalette.agents)
                    ForEach(unit.counts, id: \.self) { count in Text(verbatim: count) }
                }
                .font(.system(size: 11)).lineLimit(1).truncationMode(.tail)
            }
            HStack(spacing: 10) {
                if let current = unit.current {
                    Text("CURRENT").font(.system(size: 10, weight: .semibold)).foregroundStyle(AppPalette.agents)
                        .padding(.horizontal, 6).padding(.vertical, 1)
                        .background(AppPalette.stage, in: RoundedRectangle(cornerRadius: 4))
                    Text(verbatim: current).font(.system(size: 13)).lineLimit(1).truncationMode(.tail)
                } else { ProjectPageNote(text: "No current task recorded") }
            }
            ProjectPageStages(stage: unit.stage)
            if let error = unit.error { ProjectPageNote(text: error, warning: true) }
            ForEach(unit.waiting, id: \.self) { reason in
                HStack(spacing: 6) {
                    Text("Waiting on you").font(.system(size: 10, weight: .medium)).foregroundStyle(AppPalette.owner)
                    Text(verbatim: reason).font(.system(size: 11)).lineLimit(1).truncationMode(.tail)
                }
            }
        }
        .padding(.horizontal, 20).padding(.vertical, 17).frame(minHeight: 170)
        .background(AppPalette.raised, in: RoundedRectangle(cornerRadius: 14))
        .overlay(RoundedRectangle(cornerRadius: 14).stroke(AppPalette.focusBorder, lineWidth: 1))
    }
}

struct ProjectPageStages: View {
    let stage: ProjectPresentation.PageStage
    var body: some View {
        HStack(alignment: .bottom, spacing: 8) {
            ForEach(stage.steps) { step in
                VStack(alignment: .leading, spacing: 7) {
                    Capsule().fill(step.current ? AppPalette.agents : step.complete ? AppPalette.stageComplete : AppPalette.stageTrack)
                        .frame(height: 3)
                    HStack(spacing: 5) {
                        if step.complete { Image(systemName: "checkmark").font(.system(size: 10)) }
                        else {
                            Text(verbatim: step.number).font(.system(size: 8))
                                .frame(width: 13, height: 13)
                                .overlay(Circle().stroke(step.current ? AppPalette.agents : AppPalette.stageOutline, lineWidth: 1))
                        }
                        Text(verbatim: step.caption)
                            .font(.system(size: 11, weight: step.current ? .semibold : .regular))
                            .lineLimit(1).truncationMode(.tail)
                    }
                }
                .foregroundStyle(step.current ? AppPalette.agents : AppPalette.textSecondary)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            ForEach(stage.flags, id: \.self) { flag in
                Text(verbatim: flag).font(.system(size: 10, weight: .semibold)).foregroundStyle(AppPalette.owner)
                    .padding(.horizontal, 7).padding(.vertical, 3)
                    .background(AppPalette.badge, in: RoundedRectangle(cornerRadius: 5))
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel(Text(verbatim: stage.accessibility))
    }
}

struct ProjectPageUnits: View {
    let units: ProjectPresentation.PageUnits
    let focusID: String?
    let terms: [String]?
    let termsMessage: String?
    var body: some View {
        ProjectPagePanel(padding: 13) {
            VStack(alignment: .leading, spacing: 5) {
                HStack(spacing: 6) {
                    Text(verbatim: units.heading).font(.system(size: 15, weight: .semibold))
                    if let count = units.openCount {
                        Text(verbatim: String(count)).font(.system(size: 15)).foregroundStyle(AppPalette.textSecondary)
                    }
                }
                .padding(.horizontal, 7).padding(.bottom, 3)
                ForEach(units.rows) { unit in row(unit) }
                if !units.unclassified.isEmpty {
                    Text("Units").font(.system(size: 12, weight: .medium)).padding(.top, 8)
                    ForEach(units.unclassified) { unit in row(unit) }
                }
                if let closed = units.lastClosed {
                    Divider().overlay(AppPalette.cardBorder).padding(.vertical, 6)
                    Text("Last closed").font(.system(size: 11, weight: .medium)).foregroundStyle(AppPalette.textSecondary)
                    row(closed)
                }
                if let message = units.closedMessage { ProjectPageNote(text: message) }
                if !units.rows.isEmpty || !units.unclassified.isEmpty || units.lastClosed != nil {
                    DisclosureGroup {
                        ForEach(units.rows) { unit in details(unit) }
                        ForEach(units.unclassified) { unit in details(unit) }
                        if let closed = units.lastClosed { details(closed) }
                    } label: { Text("Plan details").font(.system(size: 11)) }
                    .tint(AppPalette.textSecondary).padding(.top, 8)
                }
                if let terms {
                    DisclosureGroup {
                        ForEach(terms, id: \.self) { term in
                            Text(verbatim: term).font(.system(size: 11)).lineLimit(1).truncationMode(.tail)
                        }
                        if let message = termsMessage { ProjectPageNote(text: message) }
                    } label: { Text("Top terms").font(.system(size: 11)) }
                    .tint(AppPalette.textSecondary).padding(.top, 8)
                }
                Spacer(minLength: 0)
            }
            .frame(minHeight: 328, alignment: .topLeading)
        }
    }

    private func row(_ unit: ProjectPresentation.PageUnit) -> some View {
        let active = unit.id == focusID
        return VStack(alignment: .leading, spacing: 4) {
            HStack(spacing: 5) {
                Text(verbatim: unit.title).font(.system(size: 11)).lineLimit(1).truncationMode(.tail)
                    .frame(maxWidth: .infinity, alignment: .leading)
                if let progress = unit.progress {
                    Text(verbatim: progress.percent).font(.system(size: 10))
                        .foregroundStyle(active ? AppPalette.agents : AppPalette.textSecondary)
                }
            }
            if let progress = unit.progress {
                HStack(spacing: 8) {
                    ProjectPageBar(progress: progress, active: active)
                    Text(verbatim: progress.compactTasks).font(.system(size: 10)).monospacedDigit()
                        .foregroundStyle(AppPalette.textSecondary).fixedSize()
                }
            }
            if let weekly = unit.weekly { ProjectPageNote(text: weekly) }
            if let closure = unit.closure { ProjectPageNote(text: closure) }
            if let error = unit.error { ProjectPageNote(text: error, warning: true) }
        }
        .padding(.horizontal, 9).padding(.vertical, 6).frame(minHeight: 47)
        .background(active ? AppPalette.selectedUnit : .clear, in: RoundedRectangle(cornerRadius: 7))
        .overlay(RoundedRectangle(cornerRadius: 7).stroke(active ? AppPalette.selectedUnitBorder : .clear, lineWidth: 1))
    }

    private func details(_ unit: ProjectPresentation.PageUnit) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            Text(verbatim: unit.title).font(.system(size: 11, weight: .medium)).lineLimit(1).truncationMode(.tail)
            stage(unit)
            ProjectPageNote(text: unit.metadata)
            if let differences = unit.checkoutDifferences { ProjectPageNote(text: differences) }
            ForEach(unit.counts, id: \.self) { count in ProjectPageNote(text: count) }
            ForEach(unit.waiting, id: \.self) { reason in
                Text(verbatim: reason).font(.system(size: 11)).lineLimit(1).truncationMode(.tail)
                    .foregroundStyle(AppPalette.owner)
            }
        }
        .padding(.vertical, 5)
    }

    private func stage(_ unit: ProjectPresentation.PageUnit) -> some View {
        HStack(spacing: 4) {
            Text(verbatim: unit.stage.label)
            if unit.stage.flags.contains("Stale") { Text("Stale") }
        }
        .font(.system(size: 9)).foregroundStyle(unit.stage.flags.isEmpty ? AppPalette.textSecondary : AppPalette.owner)
        .fixedSize()
    }
}

struct ProjectPageTasks: View {
    let tasks: ProjectPresentation.PageTasks?
    let message: String?
    var body: some View {
        ProjectPagePanel(raised: true) {
            VStack(alignment: .leading, spacing: 9) {
                HStack(spacing: 8) {
                    Text("Tasks").font(.system(size: 14, weight: .semibold))
                    Spacer(minLength: 0)
                    if let tasks { ProjectPageNote(text: tasks.summary) }
                }
                if let tasks {
                    if !tasks.finished.isEmpty {
                        DisclosureGroup {
                            ForEach(tasks.finished) { task in taskRow(task, current: false) }
                        } label: { Text(verbatim: tasks.foldedLabel).font(.system(size: 11)) }
                        .tint(AppPalette.textSecondary)
                    }
                    if let current = tasks.current { taskRow(current, current: true) }
                    else { ProjectPageNote(text: "No current task recorded") }
                    if let next = tasks.next {
                        HStack(alignment: .top, spacing: 8) {
                            Image(systemName: "square").font(.system(size: 11)).padding(.top, 2)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(verbatim: tasks.nextLabel ?? "").font(.system(size: 11))
                                Text(verbatim: next.title).font(.system(size: 11)).lineLimit(1).truncationMode(.tail)
                            }
                        }
                        .foregroundStyle(AppPalette.textSecondary)
                    } else { ProjectPageNote(text: tasks.queueLabel) }
                    if let message = tasks.emptyMessage { ProjectPageNote(text: message) }
                } else if let message { ProjectPageNote(text: message) }
                Spacer(minLength: 0)
            }
            .frame(minHeight: 182, alignment: .topLeading)
        }
    }

    private func taskRow(_ task: ProjectPresentation.PageTask, current: Bool) -> some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: current ? "circle.inset.filled" : "checkmark").font(.system(size: 10)).padding(.top, 4)
                .foregroundStyle(current ? AppPalette.agents : AppPalette.textSecondary)
            VStack(alignment: .leading, spacing: 2) {
                if current {
                    Text(verbatim: ProjectPresentation.currentTaskLabel(task)).font(.system(size: 9, weight: .bold)).tracking(0.8).foregroundStyle(AppPalette.agents)
                }
                Text(verbatim: task.line).font(.system(size: 12)).lineLimit(2).truncationMode(.tail)
                if let owner = task.owner {
                    Text(verbatim: owner).font(.system(size: 10)).foregroundStyle(AppPalette.textSecondary)
                        .lineLimit(1).truncationMode(.tail)
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
        .padding(.horizontal, 10).padding(.vertical, 9)
        .background(current ? AppPalette.currentTask : .clear, in: RoundedRectangle(cornerRadius: 8))
        .overlay(RoundedRectangle(cornerRadius: 8).stroke(current ? AppPalette.currentTaskBorder : .clear, lineWidth: 1))
    }
}

struct ProjectPageGate: View {
    let gate: ProjectPresentation.PageGate?
    let message: String
    var body: some View {
        ProjectPagePanel {
            VStack(alignment: .leading, spacing: 10) {
                Text("VERIFICATION GATE").font(.system(size: 10)).tracking(1).foregroundStyle(AppPalette.textSecondary)
                if let gate {
                    Text(verbatim: gate.status).font(.system(size: 20, weight: .medium))
                        .foregroundStyle(gate.warning ? AppPalette.owner : AppPalette.textPrimary)
                    if let day = gate.day { ProjectPageNote(text: day) }
                    if let result = gate.result { ProjectPageNote(text: result) }
                    if let movement = gate.movement { ProjectPageNote(text: movement) }
                    if let failed = gate.failedRuns { ProjectPageNote(text: failed, warning: true) }
                    if let checkout = gate.checkout { ProjectPageNote(text: checkout, warning: gate.dirty) }
                } else { ProjectPageNote(text: message) }
                Spacer(minLength: 0)
            }
            .frame(minHeight: 182, alignment: .topLeading)
        }
    }
}
