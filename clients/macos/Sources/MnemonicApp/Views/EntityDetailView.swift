import SwiftUI
import MnemonicShared

struct EntityDetailView: View {
    let detail: EntityDetail?
    let isLoading: Bool
    @ObservedObject var model: MnemonicAppModel
    let onMerged: () -> Void

    @State private var showMergeSheet = false
    @State private var mergeTarget = ""
    @State private var confirmMerge = false

    var body: some View {
        Group {
            if isLoading {
                ProgressView()
                    .controlSize(.small)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
            } else if let detail, detail.found {
                ScrollView {
                    VStack(alignment: .leading, spacing: Theme.Space.xl) {
                        headerBlock(detail)
                        if !detail.neighbors.isEmpty {
                            neighborsBlock(detail)
                        }
                        if !detail.edges.isEmpty {
                            relationsBlock(detail)
                        }
                        if !detail.memories.isEmpty {
                            memoriesBlock(detail)
                        }
                    }
                    .padding(Theme.Space.xxl)
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else {
                EmptyStateView(
                    icon: "circle.dotted",
                    title: "Select an entity",
                    subtitle: "Click an entity on the left to see its aliases, neighbors, and linked memories."
                )
            }
        }
        .sheet(isPresented: $showMergeSheet) { mergeSheet }
        .alert("Merge entities?", isPresented: $confirmMerge) {
            Button("Cancel", role: .cancel) {}
            Button("Merge", role: .destructive) { Task { await merge() } }
        } message: {
            Text("This merges the selected entity into \(mergeTarget) and preserves the original name as an alias.")
        }
    }

    private func headerBlock(_ detail: EntityDetail) -> some View {
        let color = EntityIcon.color(for: detail.entityType)
        return VStack(alignment: .leading, spacing: Theme.Space.lg) {
            HStack(alignment: .top, spacing: Theme.Space.md) {
                ZStack {
                    Circle()
                        .fill(color.opacity(0.12))
                        .frame(width: 44, height: 44)
                    Image(systemName: EntityIcon.symbol(for: detail.entityType))
                        .font(.system(size: 18, weight: .regular))
                        .foregroundStyle(color)
                }
                VStack(alignment: .leading, spacing: 4) {
                    Text(detail.entityName)
                        .font(Theme.Font.heading)
                        .tracking(Theme.Font.trackingHeading)
                        .foregroundStyle(Theme.Palette.textPrimary)
                        .textSelection(.enabled)
                    Text("\(detail.entityType) · \(detail.mentionCount) mention\(detail.mentionCount == 1 ? "" : "s")")
                        .font(Theme.Font.caption)
                        .foregroundStyle(Theme.Palette.textMuted)
                }
                Spacer()
                Button {
                    mergeTarget = ""
                    showMergeSheet = true
                } label: {
                    Label("Merge", systemImage: "arrow.triangle.merge")
                        .font(Theme.Font.caption)
                }
                .buttonStyle(SubtleButtonStyle())
            }

            if !detail.aliases.isEmpty {
                VStack(alignment: .leading, spacing: Theme.Space.sm) {
                    SectionLabel("Also known as")
                    FlowLayout(items: detail.aliases) { alias in
                        AliasChip(text: alias)
                    }
                }
            }
        }
    }

    private func neighborsBlock(_ detail: EntityDetail) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            SectionLabel("Neighbors")
            VStack(spacing: 0) {
                ForEach(detail.neighbors) { neighbor in
                    HStack(spacing: Theme.Space.sm) {
                        Image(systemName: EntityIcon.symbol(for: neighbor.entityType))
                            .font(.system(size: 11))
                            .frame(width: 16)
                            .foregroundStyle(EntityIcon.color(for: neighbor.entityType))
                        Text(neighbor.name)
                            .font(Theme.Font.body)
                            .foregroundStyle(Theme.Palette.textPrimary)
                        Spacer()
                        Text(neighbor.entityType)
                            .font(Theme.Font.caption)
                            .foregroundStyle(Theme.Palette.textSubtle)
                        Text("\(neighbor.mentionCount)")
                            .font(Theme.Font.mono)
                            .foregroundStyle(Theme.Palette.textSubtle)
                            .frame(width: 28, alignment: .trailing)
                    }
                    .padding(.vertical, Theme.Space.sm)
                    Divider().background(Theme.Palette.border.opacity(0.5))
                }
            }
        }
    }

    private func relationsBlock(_ detail: EntityDetail) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            SectionLabel("Relations")
            VStack(spacing: 0) {
                ForEach(detail.edges) { edge in
                    HStack(spacing: Theme.Space.sm) {
                        Text(edge.source)
                            .font(Theme.Font.body)
                            .foregroundStyle(Theme.Palette.textPrimary)
                        Text(edge.relation)
                            .font(Theme.Font.captionBold)
                            .foregroundStyle(Theme.Palette.textMuted)
                            .padding(.horizontal, 6)
                            .padding(.vertical, 2)
                            .background(
                                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                                    .fill(Theme.Palette.bgTint)
                            )
                        Text(edge.target)
                            .font(Theme.Font.body)
                            .foregroundStyle(Theme.Palette.textPrimary)
                        Spacer()
                        Text(String(format: "%.1f", edge.weight))
                            .font(Theme.Font.mono)
                            .foregroundStyle(Theme.Palette.textSubtle)
                    }
                    .padding(.vertical, Theme.Space.sm)
                    Divider().background(Theme.Palette.border.opacity(0.5))
                }
            }
        }
    }

    private func memoriesBlock(_ detail: EntityDetail) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            SectionLabel("Linked memories")
            VStack(spacing: 0) {
                ForEach(detail.memories) { memory in
                    VStack(alignment: .leading, spacing: 4) {
                        HStack(alignment: .firstTextBaseline) {
                            Text(memory.title)
                                .font(Theme.Font.body)
                                .foregroundStyle(Theme.Palette.textPrimary)
                                .lineLimit(2)
                            Spacer()
                            Text(RelativeTime.string(from: memory.timestamp))
                                .font(Theme.Font.caption)
                                .foregroundStyle(Theme.Palette.textSubtle)
                        }
                        HStack(spacing: Theme.Space.sm) {
                            TypeChip(type: memory.memoryType)
                            Text(String(format: "%.2f", memory.importance))
                                .font(Theme.Font.mono)
                                .foregroundStyle(Theme.Palette.textSubtle)
                        }
                    }
                    .padding(.vertical, Theme.Space.md)
                    Divider().background(Theme.Palette.border.opacity(0.5))
                }
            }
        }
    }

    private var mergeSheet: some View {
        VStack(alignment: .leading, spacing: Theme.Space.lg) {
            Text("Merge entity")
                .font(Theme.Font.heading)
                .tracking(Theme.Font.trackingHeading)
                .foregroundStyle(Theme.Palette.textPrimary)
            Text("Enter the canonical entity name. The selected entity becomes an alias; mentions and edges are reassigned.")
                .font(Theme.Font.body)
                .foregroundStyle(Theme.Palette.textMuted)
            TextField("canonical-name", text: $mergeTarget)
                .textFieldStyle(.plain)
                .font(Theme.Font.body)
                .padding(.horizontal, Theme.Space.md)
                .padding(.vertical, Theme.Space.sm)
                .background(
                    RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                        .fill(Theme.Palette.bgTint)
                )
            HStack {
                Spacer()
                Button("Cancel") { showMergeSheet = false }
                    .buttonStyle(SubtleButtonStyle())
                Button("Continue") {
                    showMergeSheet = false
                    confirmMerge = true
                }
                .buttonStyle(SubtleButtonStyle())
                .disabled(mergeTarget.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
        }
        .padding(Theme.Space.xl)
        .frame(width: 460)
        .background(Theme.Palette.bgSurface)
    }

    private func merge() async {
        guard let detail else { return }
        do {
            let report = try await model.client.mergeEntity(name: detail.entityName, into: mergeTarget)
            model.showToast("\(report.action.capitalized): \(mergeTarget)")
            onMerged()
        } catch {
            model.showToast(error.localizedDescription)
        }
    }
}
