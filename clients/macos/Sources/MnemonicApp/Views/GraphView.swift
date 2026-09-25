import SwiftUI
import MnemonicShared

/// Knowledge-graph dashboard tab.
///
/// Layout: thin toolbar with filter chips + search, big canvas in the middle,
/// optional side panel sliding in from the right when a node is selected.
struct GraphView: View {
    @ObservedObject var model: MnemonicAppModel

    @State private var typeFilter: Set<String> = []
    @State private var searchText: String = ""
    @State private var selectedNode: String?
    @State private var selectedDetail: EntityDetail?
    @State private var fetching = false

    private var allTypes: [String] {
        guard let g = model.graph else { return [] }
        return Array(Set(g.nodes.map { $0.type.lowercased() })).sorted()
    }

    var body: some View {
        HStack(spacing: 0) {
            mainColumn
            if selectedNode != nil {
                Divider().background(Theme.Palette.border)
                sidePanel
                    .frame(width: 320)
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            }
        }
        .animation(Theme.Motion.standard, value: selectedNode)
        .task {
            if model.graph == nil { await model.refreshAll() }
        }
        .onChange(of: selectedNode) { _ in
            Task { await loadDetail() }
        }
    }

    // MARK: - Main column

    private var mainColumn: some View {
        VStack(spacing: 0) {
            toolbar
            Divider().background(Theme.Palette.border)
            content
        }
    }

    private var toolbar: some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            HStack(alignment: .firstTextBaseline) {
                VStack(alignment: .leading, spacing: 2) {
                    Text("Graph")
                        .font(Theme.Font.heading)
                        .tracking(Theme.Font.trackingHeading)
                        .foregroundStyle(Theme.Palette.textPrimary)
                    if let g = model.graph {
                        Text("\(g.nodes.count) entities · \(g.edges.count) relations")
                            .font(Theme.Font.caption)
                            .foregroundStyle(Theme.Palette.textSubtle)
                    }
                }
                Spacer()
                Button {
                    Task { await model.refreshAll() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                        .font(Theme.Font.caption)
                }
                .buttonStyle(SubtleButtonStyle())
            }

            HStack(spacing: Theme.Space.sm) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.Palette.textSubtle)
                TextField("Filter entities by name", text: $searchText)
                    .textFieldStyle(.plain)
                    .font(Theme.Font.body)
            }
            .padding(.horizontal, Theme.Space.md)
            .padding(.vertical, Theme.Space.sm)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .fill(Theme.Palette.bgTint)
            )

            if !allTypes.isEmpty {
                ScrollView(.horizontal, showsIndicators: false) {
                    HStack(spacing: 6) {
                        ForEach(allTypes, id: \.self) { type in
                            FilterChip(
                                label: type.capitalized,
                                color: EntityIcon.color(for: type),
                                icon: EntityIcon.symbol(for: type),
                                isSelected: typeFilter.contains(type)
                            ) {
                                if typeFilter.contains(type) {
                                    typeFilter.remove(type)
                                } else {
                                    typeFilter.insert(type)
                                }
                            }
                        }
                    }
                }
            }
        }
        .padding(Theme.Space.lg)
        .background(Theme.Palette.bgPrimary)
    }

    @ViewBuilder
    private var content: some View {
        if let graph = model.graph, !graph.nodes.isEmpty {
            GraphCanvas(
                nodes: graph.nodes,
                edges: graph.edges,
                typeFilter: typeFilter,
                searchText: searchText,
                selectedNode: $selectedNode
            )
        } else {
            EmptyStateView(
                icon: "sparkles",
                title: "No constellation yet",
                subtitle: "Use mnemonic in a project for a bit. Entities and edges form automatically as the daemon watches your work."
            )
            .background(Theme.Palette.bgPrimary)
        }
    }

    // MARK: - Side panel

    private var sidePanel: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.lg) {
                HStack {
                    Spacer()
                    Button {
                        selectedNode = nil
                        selectedDetail = nil
                    } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(Theme.Palette.textMuted)
                    }
                    .buttonStyle(.plain)
                }

                if let detail = selectedDetail {
                    panelHeader(detail)
                    if !detail.aliases.isEmpty {
                        SectionLabel("Also known as")
                        WrapLayout(spacing: 6) {
                            ForEach(detail.aliases, id: \.self) { alias in
                                AliasChip(text: alias)
                            }
                        }
                    }
                    if !detail.neighbors.isEmpty {
                        SectionLabel("Connected to")
                        VStack(alignment: .leading, spacing: Theme.Space.xs) {
                            ForEach(detail.neighbors.prefix(8)) { neighbor in
                                neighborRow(neighbor)
                            }
                        }
                    }
                    if !detail.memories.isEmpty {
                        SectionLabel("Recent memories")
                        VStack(alignment: .leading, spacing: Theme.Space.sm) {
                            ForEach(detail.memories.prefix(6)) { memory in
                                memoryRow(memory)
                            }
                        }
                    }
                } else {
                    HStack {
                        ProgressView().controlSize(.small)
                        Text("Loading...")
                            .font(Theme.Font.caption)
                            .foregroundStyle(Theme.Palette.textMuted)
                    }
                }

                Spacer(minLength: 0)
            }
            .padding(Theme.Space.lg)
        }
        .background(Theme.Palette.bgSurface)
    }

    private func panelHeader(_ detail: EntityDetail) -> some View {
        let color = EntityIcon.color(for: detail.entityType)
        return HStack(spacing: Theme.Space.md) {
            ZStack {
                Circle()
                    .fill(color.opacity(0.12))
                    .frame(width: 36, height: 36)
                Image(systemName: EntityIcon.symbol(for: detail.entityType))
                    .font(.system(size: 16, weight: .regular))
                    .foregroundStyle(color)
            }
            VStack(alignment: .leading, spacing: 2) {
                Text(detail.entityName)
                    .font(Theme.Font.title)
                    .foregroundStyle(Theme.Palette.textPrimary)
                Text("\(detail.entityType) · \(detail.mentionCount) mentions")
                    .font(Theme.Font.caption)
                    .foregroundStyle(Theme.Palette.textMuted)
            }
            Spacer()
        }
    }

    private func neighborRow(_ neighbor: GraphNeighbor) -> some View {
        let color = EntityIcon.color(for: neighbor.entityType)
        return Button {
            selectedNode = neighbor.name
        } label: {
            HStack(spacing: Theme.Space.sm) {
                Image(systemName: EntityIcon.symbol(for: neighbor.entityType))
                    .font(.system(size: 11))
                    .foregroundStyle(color)
                    .frame(width: 16)
                Text(neighbor.name)
                    .font(Theme.Font.body)
                    .foregroundStyle(Theme.Palette.textPrimary)
                Spacer()
                Text("\(neighbor.mentionCount)")
                    .font(Theme.Font.caption)
                    .foregroundStyle(Theme.Palette.textSubtle)
            }
            .padding(.horizontal, Theme.Space.sm)
            .padding(.vertical, Theme.Space.xs)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .fill(Color.clear)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    private func memoryRow(_ memory: GraphMemory) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(memory.title)
                .font(Theme.Font.body)
                .foregroundStyle(Theme.Palette.textPrimary)
                .lineLimit(2)
            HStack(spacing: 6) {
                TypeChip(type: memory.memoryType)
                Text(RelativeTime.string(from: memory.timestamp))
                    .font(Theme.Font.caption)
                    .foregroundStyle(Theme.Palette.textSubtle)
            }
        }
    }

    private func loadDetail() async {
        guard let id = selectedNode else {
            selectedDetail = nil
            return
        }
        if fetching { return }
        fetching = true
        defer { fetching = false }
        do {
            selectedDetail = try await model.client.fetchEntity(name: id)
        } catch {
            selectedDetail = nil
        }
    }
}

// MARK: - Filter Chip

private struct FilterChip: View {
    let label: String
    let color: Color
    let icon: String
    let isSelected: Bool
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            HStack(spacing: 4) {
                Image(systemName: icon)
                    .font(.system(size: 9, weight: .semibold))
                Text(label)
                    .font(Theme.Font.captionBold)
            }
            .foregroundStyle(isSelected ? color : Theme.Palette.textMuted)
            .padding(.horizontal, Theme.Space.sm)
            .padding(.vertical, 4)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .fill(isSelected ? color.opacity(0.15) : Theme.Palette.bgTint)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .stroke(isSelected ? color.opacity(0.3) : Theme.Palette.border,
                            lineWidth: Theme.Stroke.hairline)
            )
        }
        .buttonStyle(.plain)
    }
}

// MARK: - WrapLayout (for alias chips wrapping)

private struct WrapLayout: Layout {
    var spacing: CGFloat = 6

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let maxWidth = proposal.width ?? .infinity
        var height: CGFloat = 0
        var x: CGFloat = 0
        var rowHeight: CGFloat = 0
        for sv in subviews {
            let size = sv.sizeThatFits(.unspecified)
            if x + size.width > maxWidth {
                height += rowHeight + spacing
                rowHeight = 0
                x = 0
            }
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
        height += rowHeight
        return CGSize(width: maxWidth.isFinite ? maxWidth : x, height: height)
    }

    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        var x = bounds.minX
        var y = bounds.minY
        var rowHeight: CGFloat = 0
        for sv in subviews {
            let size = sv.sizeThatFits(.unspecified)
            if x + size.width > bounds.maxX {
                x = bounds.minX
                y += rowHeight + spacing
                rowHeight = 0
            }
            sv.place(at: CGPoint(x: x, y: y), proposal: .unspecified)
            x += size.width + spacing
            rowHeight = max(rowHeight, size.height)
        }
    }
}
