import SwiftUI
import MnemonicShared

struct MemoriesView: View {
    @ObservedObject var model: MnemonicAppModel
    @State private var filter = ""
    @State private var selected: Memory?
    @State private var pendingForget: Memory?

    private var filtered: [Memory] {
        guard !filter.isEmpty else { return model.memories }
        return model.memories.filter {
            $0.title.localizedCaseInsensitiveContains(filter)
                || $0.content.localizedCaseInsensitiveContains(filter)
                || $0.tags.contains { $0.localizedCaseInsensitiveContains(filter) }
        }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Theme.Palette.border)
            HSplitView {
                listColumn.frame(minWidth: 420)
                MemoryDetailView(memory: selected ?? filtered.first, onForget: { memory in
                    pendingForget = memory
                })
                .frame(minWidth: 420)
                .background(Theme.Palette.bgSurface)
            }
        }
        .background(Theme.Palette.bgPrimary)
        .task {
            if model.memories.isEmpty {
                await model.refreshMemories()
            }
        }
        .alert("Forget this memory?", isPresented: Binding(
            get: { pendingForget != nil },
            set: { if !$0 { pendingForget = nil } }
        )) {
            Button("Cancel", role: .cancel) { pendingForget = nil }
            Button("Forget", role: .destructive) {
                if let memory = pendingForget {
                    Task { await forget(memory) }
                }
            }
        } message: {
            Text("This removes the memory plus its graph links. Reflection sources cannot be forgotten this way.")
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Memories")
                    .font(Theme.Font.display)
                    .tracking(Theme.Font.trackingDisplay)
                    .foregroundStyle(Theme.Palette.textPrimary)
                Text("\(model.memories.count) total")
                    .font(Theme.Font.caption)
                    .foregroundStyle(Theme.Palette.textSubtle)
            }
            Spacer()
            Button {
                Task { await model.refreshMemories() }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
                    .font(Theme.Font.caption)
            }
            .buttonStyle(SubtleButtonStyle())
        }
        .padding(Theme.Space.xl)
    }

    private var listColumn: some View {
        VStack(spacing: 0) {
            searchBar
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(filtered) { memory in
                        MemoryRow(
                            memory: memory,
                            isSelected: selected?.id == memory.id
                        )
                        .contentShape(Rectangle())
                        .onTapGesture { selected = memory }
                        .contextMenu {
                            Button("Forget", role: .destructive) {
                                pendingForget = memory
                            }
                        }
                        Divider().background(Theme.Palette.border.opacity(0.5))
                    }
                }
            }
        }
    }

    private var searchBar: some View {
        HStack(spacing: Theme.Space.sm) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 11))
                .foregroundStyle(Theme.Palette.textSubtle)
            TextField("Filter title, content, tags", text: $filter)
                .textFieldStyle(.plain)
                .font(Theme.Font.body)
            if !filter.isEmpty {
                Button { filter = "" } label: {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 11))
                        .foregroundStyle(Theme.Palette.textSubtle)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, Theme.Space.md)
        .padding(.vertical, Theme.Space.sm)
        .background(
            RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                .fill(Theme.Palette.bgTint)
        )
        .padding(.horizontal, Theme.Space.lg)
        .padding(.vertical, Theme.Space.md)
    }

    private func forget(_ memory: Memory) async {
        do {
            _ = try await model.client.forgetMemory(id: memory.id)
            model.showToast("Memory removed")
            pendingForget = nil
            selected = nil
            await model.refreshMemories()
        } catch {
            model.showToast(error.localizedDescription)
        }
    }
}

struct MemoryRow: View {
    let memory: Memory
    var isSelected: Bool = false

    var body: some View {
        HStack(alignment: .top, spacing: Theme.Space.md) {
            // Active indicator
            Rectangle()
                .fill(isSelected ? Theme.Palette.accent : Color.clear)
                .frame(width: 2)
                .padding(.vertical, 2)

            VStack(alignment: .leading, spacing: 6) {
                HStack(alignment: .firstTextBaseline, spacing: Theme.Space.sm) {
                    Text(memory.title)
                        .font(Theme.Font.title)
                        .foregroundStyle(Theme.Palette.textPrimary)
                        .lineLimit(2)
                    Spacer()
                    Text(RelativeTime.string(from: memory.timestamp))
                        .font(Theme.Font.caption)
                        .foregroundStyle(Theme.Palette.textSubtle)
                }

                Text(memory.content.split(separator: "\n").first.map(String.init) ?? memory.content)
                    .font(Theme.Font.body)
                    .foregroundStyle(Theme.Palette.textMuted)
                    .lineLimit(2)

                HStack(spacing: Theme.Space.sm) {
                    TypeChip(type: memory.memoryType)
                    Spacer()
                    ImportanceMeter(value: memory.importance)
                }
            }
            .padding(.vertical, Theme.Space.md)
            .padding(.trailing, Theme.Space.lg)
        }
        .padding(.leading, Theme.Space.lg - 2)
        .background(isSelected ? Theme.Palette.bgTint : Color.clear)
    }
}

/// A tiny dot-meter (5 dots) — calmer than a ProgressView bar.
private struct ImportanceMeter: View {
    let value: Double // 0..1

    var body: some View {
        HStack(spacing: 3) {
            ForEach(0..<5, id: \.self) { i in
                Circle()
                    .fill(i < lit ? Theme.Palette.textMuted : Theme.Palette.bgTint)
                    .frame(width: 4, height: 4)
            }
        }
    }

    private var lit: Int {
        max(0, min(5, Int((value * 5).rounded())))
    }
}
