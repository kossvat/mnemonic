import SwiftUI
import MnemonicShared

struct MemoryDetailView: View {
    let memory: Memory?
    let onForget: (Memory) -> Void

    var body: some View {
        if let memory {
            ScrollView {
                VStack(alignment: .leading, spacing: Theme.Space.xl) {
                    headerBlock(memory)
                    contentBlock(memory)
                    if !memory.tags.isEmpty {
                        tagsBlock(memory)
                    }
                    metadataBlock(memory)
                }
                .padding(Theme.Space.xxl)
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .background(Theme.Palette.bgSurface)
        } else {
            EmptyStateView(
                icon: "tray",
                title: "No memory selected",
                subtitle: "Pick one from the list to see its full content, tags, and metadata."
            )
            .background(Theme.Palette.bgSurface)
        }
    }

    private func headerBlock(_ memory: Memory) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            HStack(spacing: Theme.Space.sm) {
                TypeChip(type: memory.memoryType)
                Text(RelativeTime.string(from: memory.timestamp))
                    .font(Theme.Font.caption)
                    .foregroundStyle(Theme.Palette.textSubtle)
                Spacer()
                Button(role: .destructive) {
                    onForget(memory)
                } label: {
                    Label("Forget", systemImage: "trash")
                        .font(Theme.Font.caption)
                }
                .buttonStyle(DangerButtonStyle())
            }
            Text(memory.title)
                .font(Theme.Font.heading)
                .tracking(Theme.Font.trackingHeading)
                .foregroundStyle(Theme.Palette.textPrimary)
                .textSelection(.enabled)
        }
    }

    private func contentBlock(_ memory: Memory) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            SectionLabel("Content")
            Text(memory.content)
                .font(Theme.Font.body)
                .foregroundStyle(Theme.Palette.textPrimary)
                .lineSpacing(3)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }

    private func tagsBlock(_ memory: Memory) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            SectionLabel("Tags")
            FlowLayout(items: memory.tags) { tag in
                AliasChip(text: "#\(tag)")
            }
        }
    }

    private func metadataBlock(_ memory: Memory) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            SectionLabel("Metadata")
            VStack(alignment: .leading, spacing: 4) {
                metadataRow(label: "ID", value: memory.id, mono: true)
                metadataRow(label: "Timestamp", value: memory.timestamp, mono: true)
                metadataRow(label: "Importance", value: String(format: "%.2f", memory.importance), mono: true)
            }
        }
    }

    private func metadataRow(label: String, value: String, mono: Bool) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: Theme.Space.md) {
            Text(label)
                .font(Theme.Font.caption)
                .foregroundStyle(Theme.Palette.textSubtle)
                .frame(width: 80, alignment: .leading)
            Text(value)
                .font(mono ? Theme.Font.mono : Theme.Font.body)
                .foregroundStyle(Theme.Palette.textMuted)
                .textSelection(.enabled)
        }
    }
}

struct FlowLayout<Data: RandomAccessCollection, Content: View>: View where Data.Element: Hashable {
    let items: Data
    let content: (Data.Element) -> Content

    var body: some View {
        LazyVGrid(columns: [GridItem(.adaptive(minimum: 90), spacing: 6)], alignment: .leading, spacing: 6) {
            ForEach(Array(items), id: \.self) { item in
                content(item)
            }
        }
    }
}
