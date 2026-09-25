import SwiftUI
import MnemonicShared

enum ManageAction: Identifiable {
    case dedupeApply
    case reflectApply
    case reextractRecent
    case reextractAll
    case cleanup

    var id: String { title }

    var title: String {
        switch self {
        case .dedupeApply:      "Apply dedupe"
        case .reflectApply:     "Apply reflection"
        case .reextractRecent:  "Reextract last 30 days"
        case .reextractAll:     "Reextract all"
        case .cleanup:          "Cleanup"
        }
    }

    var description: String {
        switch self {
        case .dedupeApply:
            "merge duplicate graph entities and preserve aliases"
        case .reflectApply:
            "consolidate near-duplicate memories into canonical summaries (sources never deleted, only marked superseded)"
        case .reextractRecent:
            "run graph extraction for memories from the last 30 days"
        case .reextractAll:
            "run graph extraction across the full memory database"
        case .cleanup:
            "delete old low-importance memories with days=30, threshold=0.4 (reflection sources protected)"
        }
    }
}

struct ManageView: View {
    @ObservedObject var model: MnemonicAppModel
    @State private var resultText = ""
    @State private var isWorking = false
    @State private var pendingAction: ManageAction?

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.xl) {
                header

                VStack(alignment: .leading, spacing: Theme.Space.xl) {
                    Group {
                        SectionLabel("Graph maintenance")
                        VStack(spacing: Theme.Space.md) {
                            ActionRow(
                                title: "Run dedupe (dry-run)",
                                subtitle: "Preview canonical entity groups before changing anything.",
                                icon: "doc.text.magnifyingglass",
                                destructive: false
                            ) {
                                Task { await runDedupe(apply: false) }
                            }
                            ActionRow(
                                title: "Apply dedupe",
                                subtitle: "Merge duplicate entities and record aliases. Reversible via merge log.",
                                icon: "arrow.triangle.merge",
                                destructive: true
                            ) {
                                pendingAction = .dedupeApply
                            }
                        }
                    }

                    Group {
                        SectionLabel("Memory consolidation")
                        VStack(spacing: Theme.Space.md) {
                            ActionRow(
                                title: "Run reflection (dry-run)",
                                subtitle: "Preview clusters of near-duplicate memories with proposed canonicals.",
                                icon: "sparkles",
                                destructive: false
                            ) {
                                Task { await runReflect(apply: false) }
                            }
                            ActionRow(
                                title: "Apply reflection",
                                subtitle: "Create canonical memories; sources marked superseded — never deleted.",
                                icon: "sparkles.square.filled.on.square",
                                destructive: true
                            ) {
                                pendingAction = .reflectApply
                            }
                        }
                    }

                    Group {
                        SectionLabel("Reextract")
                        VStack(spacing: Theme.Space.md) {
                            ActionRow(
                                title: "Reextract last 30 days",
                                subtitle: "Dry-run then apply across recent memories.",
                                icon: "arrow.clockwise.circle",
                                destructive: false
                            ) {
                                pendingAction = .reextractRecent
                            }
                            ActionRow(
                                title: "Reextract everything",
                                subtitle: "Full database graph rebuild — slow on first run, cached after.",
                                icon: "arrow.clockwise.icloud",
                                destructive: false
                            ) {
                                pendingAction = .reextractAll
                            }
                        }
                    }

                    Group {
                        SectionLabel("Cleanup")
                        VStack(spacing: Theme.Space.md) {
                            ActionRow(
                                title: "Prune low-importance notes",
                                subtitle: "Removes notes older than 30 days with importance < 0.4. Reflection sources protected.",
                                icon: "trash",
                                destructive: true
                            ) {
                                pendingAction = .cleanup
                            }
                        }
                    }
                }

                if isWorking {
                    HStack(spacing: Theme.Space.sm) {
                        ProgressView().controlSize(.small)
                        Text("Working...")
                            .font(Theme.Font.caption)
                            .foregroundStyle(Theme.Palette.textMuted)
                    }
                }

                if !resultText.isEmpty {
                    Card {
                        VStack(alignment: .leading, spacing: Theme.Space.sm) {
                            SectionLabel("Result")
                            Text(resultText)
                                .font(Theme.Font.mono)
                                .foregroundStyle(Theme.Palette.textPrimary)
                                .textSelection(.enabled)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        }
                    }
                }
            }
            .padding(Theme.Space.xl)
            .frame(maxWidth: 760, alignment: .leading)
        }
        .background(Theme.Palette.bgPrimary)
        .alert("Are you sure?", isPresented: Binding(
            get: { pendingAction != nil },
            set: { if !$0 { pendingAction = nil } }
        )) {
            Button("Cancel", role: .cancel) {}
            Button("Run", role: .destructive) {
                if let action = pendingAction {
                    Task { await run(action) }
                }
            }
        } message: {
            Text("This will: \(pendingAction?.description ?? "run the selected operation").")
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Manage")
                .font(Theme.Font.display)
                .tracking(Theme.Font.trackingDisplay)
                .foregroundStyle(Theme.Palette.textPrimary)
            Text("Maintenance actions for the memory store. Dry-run first; destructive operations confirm.")
                .font(Theme.Font.body)
                .foregroundStyle(Theme.Palette.textMuted)
        }
    }

    private func run(_ action: ManageAction) async {
        switch action {
        case .dedupeApply:     await runDedupe(apply: true)
        case .reflectApply:    await runReflect(apply: true)
        case .reextractRecent: await runReextract(sinceDays: 30, limit: nil)
        case .reextractAll:    await runReextract(sinceDays: nil, limit: nil)
        case .cleanup:         await runCleanup()
        }
    }

    private func runDedupe(apply: Bool) async {
        await perform {
            let report = try await model.client.dedupe(apply: apply)
            var lines = [
                "Dedupe \(report.dryRun ? "plan" : "applied")",
                "Groups: \(report.groups.count)",
                "Merged: \(report.merged) · Renamed: \(report.renamed)",
                "Edges redirected: \(report.edgesRedirected)",
                "Memory links redirected: \(report.memoryLinksRedirected)",
                ""
            ]
            for g in report.groups.prefix(12) {
                lines.append("• \(g.canonical) ← \(g.variants.joined(separator: ", "))")
            }
            return lines.joined(separator: "\n")
        }
    }

    private func runReflect(apply: Bool) async {
        await perform {
            let plan = try await model.client.reflect(apply: apply, threshold: 0.85)
            var lines = [
                "Reflection \(plan.mode)",
                "Pool size: \(plan.poolSize) memories @ threshold \(String(format: "%.2f", plan.threshold))",
                "Clusters: \(plan.clusters.count)",
                ""
            ]
            for (i, c) in plan.clusters.prefix(15).enumerated() {
                let avgCos = c.cosines.isEmpty
                    ? 0.0
                    : c.cosines.reduce(0, +) / Double(c.cosines.count)
                let shortIds = c.sourceIds
                    .map { String($0.prefix(8)) }
                    .joined(separator: ", ")
                lines.append("[\(i + 1)] \(c.draftTitle)")
                lines.append("    \(c.sourceIds.count) members · avg cos \(String(format: "%.3f", avgCos))")
                lines.append("    sources: \(shortIds)")
                if let canonical = c.canonicalId {
                    lines.append("    ↳ canonical: \(canonical.prefix(8))")
                }
            }
            if plan.clusters.count > 15 {
                lines.append("... and \(plan.clusters.count - 15) more clusters")
            }
            if !apply {
                lines.append("")
                lines.append("Dry-run — no writes. Re-run with Apply to consolidate.")
            }
            return lines.joined(separator: "\n")
        }
    }

    private func runReextract(sinceDays: Int?, limit: Int?) async {
        await perform {
            let preview = try await model.client.reextract(sinceDays: sinceDays, limit: limit, dryRun: true)
            let applied = try await model.client.reextract(sinceDays: sinceDays, limit: limit, dryRun: false)
            return """
            Reextract
            Planned (dry-run): \(preview.planned)
            Extractor: \(applied.extractor)
            Processed: \(applied.processed)
            Entities added: \(applied.entitiesAdded)
            Edges added: \(applied.edgesAdded)
            """
        }
    }

    private func runCleanup() async {
        await perform {
            _ = try await model.client.cleanup(days: 30, threshold: 0.4, confirm: false)
            let applied = try await model.client.cleanup(days: 30, threshold: 0.4, confirm: true)
            return "Cleanup deleted \(applied.deleted) memories."
        }
    }

    private func perform(_ operation: () async throws -> String) async {
        isWorking = true
        defer { isWorking = false }
        do {
            resultText = try await operation()
            model.showToast("Action complete")
            await model.refreshAll()
        } catch {
            resultText = error.localizedDescription
            model.showToast("Action failed")
        }
    }
}

struct ActionRow: View {
    let title: String
    let subtitle: String
    let icon: String
    let destructive: Bool
    let action: () -> Void

    var body: some View {
        Card(padding: Theme.Space.md) {
            HStack(spacing: Theme.Space.md) {
                ZStack {
                    RoundedRectangle(cornerRadius: 6, style: .continuous)
                        .fill((destructive ? Theme.Palette.feedback : Theme.Palette.accent).opacity(0.10))
                        .frame(width: 32, height: 32)
                    Image(systemName: icon)
                        .font(.system(size: 13, weight: .regular))
                        .foregroundStyle(destructive ? Theme.Palette.feedback : Theme.Palette.accent)
                }
                VStack(alignment: .leading, spacing: 2) {
                    Text(title)
                        .font(Theme.Font.title)
                        .foregroundStyle(Theme.Palette.textPrimary)
                    Text(subtitle)
                        .font(Theme.Font.caption)
                        .foregroundStyle(Theme.Palette.textMuted)
                        .lineLimit(2)
                }
                Spacer()
                Button("Run", action: action)
                    .buttonStyle(destructive ? AnyButtonStyle(DangerButtonStyle())
                                              : AnyButtonStyle(SubtleButtonStyle()))
            }
        }
    }
}

/// Type-erased wrapper so the same Button can use either subtle or danger style.
struct AnyButtonStyle: ButtonStyle {
    private let _makeBody: (Configuration) -> AnyView
    init<S: ButtonStyle>(_ style: S) {
        _makeBody = { configuration in AnyView(style.makeBody(configuration: configuration)) }
    }
    func makeBody(configuration: Configuration) -> some View {
        _makeBody(configuration)
    }
}
