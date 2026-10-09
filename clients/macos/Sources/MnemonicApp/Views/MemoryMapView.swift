import SwiftUI
import MnemonicShared
import AppKit

/// Memory Map — the "Obsidian notes graph" view of mnemonic.
///
/// Each node is a **memory**. Edges form between memories that share at
/// least `minShared` entities. Click a node → read full content on the right,
/// plus its linked entities and (if it's a canonical) its consolidated
/// source memories.
struct MemoryMapView: View {
    @ObservedObject var model: MnemonicAppModel

    @State private var graph: MemoryGraphResponse?
    @State private var loading = false
    @State private var selectedId: String?
    @State private var error: String?

    // Filter state
    @State private var searchText: String = ""
    @State private var typeFilter: String = "all"
    @State private var timeRange: TimeRange = .all
    @State private var minShared: Int = 1
    @State private var limit: Int = 40

    // Detail enrichment
    @State private var detailEntity: EntityDetail?
    @State private var detailSources: [MemorySource] = []

    private enum TimeRange: String, CaseIterable, Identifiable {
        case week  = "7d"
        case month = "30d"
        case all   = "all"
        var id: String { rawValue }
        var sinceDays: Int? {
            switch self {
            case .week: return 7
            case .month: return 30
            case .all: return nil
            }
        }
    }

    private let typeOptions = ["all", "decision", "feedback", "note", "security", "session_summary"]

    var body: some View {
        HStack(spacing: 0) {
            mainColumn
            if let id = selectedId,
               let memory = model.memories.first(where: { $0.id == id })
            {
                Divider().background(Theme.Palette.border)
                detailPane(memory: memory).frame(width: 380)
                    .transition(.move(edge: .trailing).combined(with: .opacity))
            }
        }
        .animation(Theme.Motion.standard, value: selectedId)
        .task {
            await ensureMemoriesLoaded()
            await loadGraph()
        }
    }

    // MARK: - Layout

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
                    Text("Memory Map")
                        .font(Theme.Font.heading)
                        .tracking(Theme.Font.trackingHeading)
                        .foregroundStyle(Theme.Palette.textPrimary)
                    if let g = graph {
                        Text("\(g.nodes.count) memories · \(g.edges.count) shared-entity links · top \(limit) by recency")
                            .font(Theme.Font.caption)
                            .foregroundStyle(Theme.Palette.textSubtle)
                    } else {
                        Text("Filter, search, click a node to read.")
                            .font(Theme.Font.caption)
                            .foregroundStyle(Theme.Palette.textSubtle)
                    }
                }
                Spacer()
                Button {
                    Task { await loadGraph() }
                } label: {
                    Label("Refresh", systemImage: "arrow.clockwise")
                        .font(Theme.Font.caption)
                }
                .buttonStyle(SubtleButtonStyle())
            }

            // Search
            HStack(spacing: Theme.Space.sm) {
                Image(systemName: "magnifyingglass")
                    .font(.system(size: 11))
                    .foregroundStyle(Theme.Palette.textSubtle)
                TextField("Search title or content", text: $searchText)
                    .textFieldStyle(.plain)
                    .font(Theme.Font.body)
                    .onSubmit { Task { await loadGraph() } }
                if !searchText.isEmpty {
                    Button { searchText = ""; Task { await loadGraph() } } label: {
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

            // Type + time range + min_shared
            HStack(spacing: Theme.Space.md) {
                segmented(label: "Type", selection: $typeFilter, options: typeOptions, displayMap: { $0.capitalized })
                segmented(
                    label: "Range",
                    selection: Binding(
                        get: { timeRange.rawValue },
                        set: { newVal in
                            if let r = TimeRange(rawValue: newVal) { timeRange = r }
                        }
                    ),
                    options: TimeRange.allCases.map(\.rawValue),
                    displayMap: { $0 }
                )
                HStack(spacing: 4) {
                    Text("Min shared")
                        .font(Theme.Font.caption)
                        .foregroundStyle(Theme.Palette.textSubtle)
                    Stepper("\(minShared)", value: $minShared, in: 1...5)
                        .labelsHidden()
                    Text("\(minShared)")
                        .font(Theme.Font.mono)
                        .foregroundStyle(Theme.Palette.textMuted)
                        .frame(width: 14)
                }
                HStack(spacing: 4) {
                    Text("Show")
                        .font(Theme.Font.caption)
                        .foregroundStyle(Theme.Palette.textSubtle)
                    Stepper("\(limit)", value: $limit, in: 10...200, step: 10)
                        .labelsHidden()
                    Text("\(limit)")
                        .font(Theme.Font.mono)
                        .foregroundStyle(Theme.Palette.textMuted)
                        .frame(width: 30, alignment: .trailing)
                }
                Spacer()
            }
            .onChange(of: typeFilter)  { _, _ in Task { await loadGraph() } }
            .onChange(of: timeRange)   { _, _ in Task { await loadGraph() } }
            .onChange(of: minShared)   { _, _ in Task { await loadGraph() } }
            .onChange(of: limit)       { _, _ in Task { await loadGraph() } }
        }
        .padding(Theme.Space.lg)
        .background(Theme.Palette.bgPrimary)
    }

    private func segmented(
        label: String,
        selection: Binding<String>,
        options: [String],
        displayMap: @escaping (String) -> String
    ) -> some View {
        HStack(spacing: 4) {
            Text(label)
                .font(Theme.Font.caption)
                .foregroundStyle(Theme.Palette.textSubtle)
            HStack(spacing: 1) {
                ForEach(options, id: \.self) { opt in
                    Button {
                        selection.wrappedValue = opt
                    } label: {
                        Text(displayMap(opt))
                            .font(Theme.Font.captionBold)
                            .foregroundStyle(
                                selection.wrappedValue == opt
                                    ? Theme.Palette.textPrimary
                                    : Theme.Palette.textMuted
                            )
                            .padding(.horizontal, 8)
                            .padding(.vertical, 3)
                            .background(
                                RoundedRectangle(cornerRadius: 4, style: .continuous)
                                    .fill(selection.wrappedValue == opt
                                          ? Theme.Palette.bgSurface
                                          : Color.clear)
                            )
                    }
                    .buttonStyle(.plain)
                }
            }
            .padding(2)
            .background(
                RoundedRectangle(cornerRadius: 5, style: .continuous)
                    .fill(Theme.Palette.bgTint)
            )
        }
    }

    @ViewBuilder
    private var content: some View {
        if loading {
            VStack { ProgressView().controlSize(.small) }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if let err = error {
            EmptyStateView(
                icon: "exclamationmark.triangle",
                title: "Couldn't load memory map",
                subtitle: err
            )
        } else if let g = graph, !g.nodes.isEmpty {
            StaticGraphCanvas(
                scene: .memoryMap(nodes: g.nodes, edges: g.edges),
                selectedNode: selectedId,
                onNode: { key in selectedId = key },
                onEdge: { _ in }
            )
        } else {
            EmptyStateView(
                icon: "sparkles",
                title: "No memories match",
                subtitle: searchText.isEmpty && typeFilter == "all" && timeRange == .all
                    ? "As the daemon ingests memories, this map fills in."
                    : "Try widening the filters: change Range to ‘all’ or clear the search."
            )
        }
    }

    private func detailPane(memory: Memory) -> some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.lg) {
                HStack {
                    Spacer()
                    Button {
                        selectedId = nil
                        detailEntity = nil
                        detailSources = []
                    } label: {
                        Image(systemName: "xmark")
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(Theme.Palette.textMuted)
                    }
                    .buttonStyle(.plain)
                }

                HStack(spacing: Theme.Space.sm) {
                    TypeChip(type: memory.memoryType)
                    Text(RelativeTime.string(from: memory.timestamp))
                        .font(Theme.Font.caption)
                        .foregroundStyle(Theme.Palette.textSubtle)
                }

                Text(memory.title)
                    .font(Theme.Font.heading)
                    .tracking(Theme.Font.trackingHeading)
                    .foregroundStyle(Theme.Palette.textPrimary)
                    .textSelection(.enabled)

                Text(memory.content)
                    .font(Theme.Font.body)
                    .foregroundStyle(Theme.Palette.textPrimary)
                    .lineSpacing(3)
                    .textSelection(.enabled)
                    .frame(maxWidth: .infinity, alignment: .leading)

                if !memory.tags.isEmpty {
                    SectionLabel("Tags")
                    FlowLayout(items: memory.tags) { tag in
                        AliasChip(text: "#\(tag)")
                    }
                }

                if !detailSources.isEmpty {
                    SectionLabel("Consolidates \(detailSources.count) memories")
                    VStack(alignment: .leading, spacing: Theme.Space.sm) {
                        ForEach(detailSources) { src in
                            HStack(alignment: .firstTextBaseline, spacing: 6) {
                                Image(systemName: MemoryTypeIcon.icon(for: src.memoryType))
                                    .font(.system(size: 10))
                                    .foregroundStyle(MemoryTypeIcon.color(for: src.memoryType))
                                    .frame(width: 14)
                                VStack(alignment: .leading, spacing: 2) {
                                    Text(src.title)
                                        .font(Theme.Font.body)
                                        .foregroundStyle(Theme.Palette.textPrimary)
                                        .lineLimit(2)
                                    Text("cos \(String(format: "%.2f", src.cosine)) · imp \(String(format: "%.2f", src.importance))")
                                        .font(Theme.Font.mono)
                                        .foregroundStyle(Theme.Palette.textSubtle)
                                }
                                Spacer()
                            }
                        }
                    }
                }

                Spacer(minLength: 0)
            }
            .padding(Theme.Space.lg)
        }
        .background(Theme.Palette.bgSurface)
        .task(id: memory.id) {
            await enrichDetail(for: memory)
        }
    }

    // MARK: - Loading

    private func loadGraph() async {
        loading = true
        error = nil
        defer { loading = false }
        do {
            graph = try await model.client.fetchMemoryGraph(
                limit: limit,
                sinceDays: timeRange.sinceDays,
                type: typeFilter,
                query: searchText.isEmpty ? nil : searchText,
                minShared: minShared
            )
        } catch let err {
            error = err.localizedDescription
        }
    }

    private func ensureMemoriesLoaded() async {
        if model.memories.isEmpty {
            await model.refreshMemories()
        }
    }

    /// When a memory is selected, fetch its consolidated source list
    /// (if it's a canonical). Silently skip on error — non-canonical
    /// memories will just have an empty sources array.
    private func enrichDetail(for memory: Memory) async {
        detailSources = []
        detailEntity = nil
        if let resp = try? await model.client.memorySources(id: memory.id) {
            detailSources = resp.sources
        }
    }
}
