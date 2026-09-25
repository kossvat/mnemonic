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
            MemoryMapCanvas(
                nodes: g.nodes,
                edges: g.edges,
                selectedNode: $selectedId
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

// MARK: - Canvas

/// Force-directed canvas where every node is a memory.
struct MemoryMapCanvas: View {
    let nodes: [MemoryGraphNode]
    let edges: [MemoryGraphEdge]
    @Binding var selectedNode: String?

    @State private var sim: GraphSimulation?
    @State private var simNodes: [SimNode] = []
    @State private var hoverNode: String?
    @State private var draggingNode: String?
    @State private var didDragNode = false
    @State private var pan: CGSize = .zero
    @State private var lastDragPan: CGSize = .zero
    @State private var zoom: CGFloat = 1.0
    @State private var pinchStartZoom: CGFloat = 1.0
    @State private var canvasSize: CGSize = .zero
    @State private var simTimer: Timer?
    /// Top-N memory ids that always show their label (most-connected /
    /// most-important nodes). Recomputed when `nodes` changes.
    @State private var topLabelIds: Set<String> = []

    var body: some View {
        GeometryReader { geo in
            ZStack {
                Theme.Palette.bgPrimary

                Canvas(rendersAsynchronously: true) { ctx, _ in
                    drawEdges(ctx: ctx)
                    drawNodes(ctx: ctx)
                }
                .contentShape(Rectangle())
                .gesture(unifiedDragGesture)
                .simultaneousGesture(pinchGesture)
                .onContinuousHover { phase in
                    switch phase {
                    case .active(let p):
                        let id = hitTest(p)
                        if id != hoverNode { hoverNode = id }
                    case .ended:
                        hoverNode = nil
                    }
                }
                .background(
                    GeometryReader { proxy in
                        Color.clear.preference(key: SizeKey.self, value: proxy.size)
                    }
                )
                .onPreferenceChange(SizeKey.self) { size in
                    canvasSize = size
                    if simNodes.isEmpty { bootstrap() }
                }

                VStack {
                    Spacer()
                    HStack {
                        Spacer()
                        zoomHUD
                    }
                }
                .padding(Theme.Space.md)
            }
            .background(
                ScrollWheelHandler { delta, location in
                    let factor = exp(delta * 0.005)
                    adjustZoom(by: factor, anchor: location)
                }
            )
            .onAppear { bootstrap() }
            .onDisappear { stopTimer() }
            .onChange(of: nodes) { _, _ in bootstrap() }
        }
    }

    // MARK: - Bootstrap

    private func bootstrap() {
        guard canvasSize.width > 1, canvasSize.height > 1 else { return }
        let raw = nodes.map {
            (id: $0.id, name: $0.title, type: $0.memoryType, mentions: max(1, $0.entityCount))
        }
        let positioned = GraphSimulation.initialLayout(nodes: raw, canvasSize: canvasSize)
        let simEdges = edges.map {
            SimEdge(source: $0.source, target: $0.target, weight: Double($0.weight))
        }
        let newSim = GraphSimulation(nodes: positioned, edges: simEdges)
        for _ in 0..<160 { _ = newSim.tick() }
        sim = newSim
        simNodes = newSim.tick()

        // Top-N labels: highest importance OR highest connectivity. The user
        // can always hover/click any node to see its label, but these stay
        // permanently visible as anchor points.
        let degree: [String: Int] = edges.reduce(into: [:]) { acc, e in
            acc[e.source, default: 0] += 1
            acc[e.target, default: 0] += 1
        }
        let ranked = nodes.sorted { a, b in
            let da = degree[a.id] ?? 0
            let db = degree[b.id] ?? 0
            if da != db { return da > db }
            return a.importance > b.importance
        }
        let topN = max(5, min(10, nodes.count / 8))
        topLabelIds = Set(ranked.prefix(topN).map(\.id))

        fitToView()
        startTimer()
    }

    private func startTimer() {
        stopTimer()
        simTimer = Timer.scheduledTimer(withTimeInterval: 1.0 / 60.0, repeats: true) { _ in
            guard let s = sim, !s.isSettled else { stopTimer(); return }
            simNodes = s.tick()
        }
    }

    private func stopTimer() {
        simTimer?.invalidate()
        simTimer = nil
    }

    private func fitToView() {
        guard !simNodes.isEmpty, canvasSize.width > 0, canvasSize.height > 0 else { return }
        let minX = simNodes.map(\.x).min() ?? 0
        let maxX = simNodes.map(\.x).max() ?? 0
        let minY = simNodes.map(\.y).min() ?? 0
        let maxY = simNodes.map(\.y).max() ?? 0
        let pad: CGFloat = 80
        let worldW = max(1, maxX - minX) + pad * 2
        let worldH = max(1, maxY - minY) + pad * 2
        let zX = canvasSize.width  / worldW
        let zY = canvasSize.height / worldH
        let newZoom = min(2.0, max(0.35, min(zX, zY)))
        let cx = (minX + maxX) / 2
        let cy = (minY + maxY) / 2
        zoom = newZoom
        pan = CGSize(width: -cx * newZoom, height: -cy * newZoom)
        lastDragPan = pan
    }

    // MARK: - Drawing

    private func transform(_ p: CGPoint) -> CGPoint {
        CGPoint(
            x: canvasSize.width  / 2 + (p.x * zoom) + pan.width,
            y: canvasSize.height / 2 + (p.y * zoom) + pan.height
        )
    }
    private func inverseTransform(_ p: CGPoint) -> CGPoint {
        CGPoint(
            x: (p.x - canvasSize.width  / 2 - pan.width)  / zoom,
            y: (p.y - canvasSize.height / 2 - pan.height) / zoom
        )
    }

    private func drawEdges(ctx: GraphicsContext) {
        let active = selectedNode ?? hoverNode
        let idx = Dictionary(uniqueKeysWithValues: simNodes.enumerated().map { ($1.id, $0) })
        for e in edges {
            guard let si = idx[e.source], let ti = idx[e.target] else { continue }
            let a = simNodes[si], b = simNodes[ti]
            let p1 = transform(CGPoint(x: a.x, y: a.y))
            let p2 = transform(CGPoint(x: b.x, y: b.y))

            let isActive = active == e.source || active == e.target
            let opacity: Double
            let width: CGFloat
            if isActive {
                opacity = 0.85; width = 1.6
            } else if active != nil {
                opacity = 0.08; width = 0.5
            } else {
                opacity = 0.30 + min(0.30, Double(e.weight) * 0.08)
                width = e.weight >= 3 ? 1.4 : 0.9
            }
            var path = Path()
            path.move(to: p1)
            path.addLine(to: p2)
            ctx.stroke(path,
                       with: .color(Theme.Palette.textMuted.opacity(opacity)),
                       lineWidth: width)
        }
    }

    private func drawNodes(ctx: GraphicsContext) {
        let active = selectedNode ?? hoverNode

        for node in simNodes {
            let center = transform(CGPoint(x: node.x, y: node.y))
            let radius = max(10, node.radius * zoom)
            let typeColor = MemoryTypeIcon.color(for: node.type)
            let isActive = active == node.id
            let isDim = active != nil && !isActive

            let fillTint = isActive ? 0.32 : (isDim ? 0.06 : 0.16)
            let stroke = isActive ? 1.0 : (isDim ? 0.32 : 0.78)

            let rect = CGRect(x: center.x - radius, y: center.y - radius,
                              width: radius * 2, height: radius * 2)
            ctx.fill(Path(ellipseIn: rect), with: .color(Theme.Palette.bgPrimary))
            ctx.fill(Path(ellipseIn: rect), with: .color(typeColor.opacity(fillTint)))
            ctx.stroke(Path(ellipseIn: rect),
                       with: .color(typeColor.opacity(stroke)),
                       lineWidth: isActive ? 2.4 : 1.5)

            if radius >= 11 {
                let icon = Text(Image(systemName: MemoryTypeIcon.icon(for: node.type)))
                    .font(.system(size: radius * 0.82, weight: .medium))
                    .foregroundColor(typeColor.opacity(isDim ? 0.45 : 0.95))
                ctx.draw(icon, at: center)
            }

            // Labels are SELECTIVE: hover/selected always, plus top-N "anchor"
            // memories by connectivity+importance. Keeps the canvas calm.
            let shouldLabel = isActive || topLabelIds.contains(node.id)
            if shouldLabel {
                let raw = node.name
                let truncated = raw.count > 36 ? String(raw.prefix(33)) + "…" : raw
                let fontSize: CGFloat = isActive ? 11 : 9
                let label = Text(truncated)
                    .font(.system(size: fontSize, weight: isActive ? .semibold : .regular))
                    .foregroundColor(Theme.Palette.textPrimary.opacity(isDim ? 0.32 : 0.92))
                let w = CGFloat(truncated.count) * fontSize * 0.55 + 10
                let labelRect = CGRect(
                    x: center.x - w / 2,
                    y: center.y + radius + 4,
                    width: w,
                    height: fontSize + 6
                )
                ctx.fill(
                    Path(roundedRect: labelRect, cornerRadius: 3),
                    with: .color(Theme.Palette.bgPrimary.opacity(0.88))
                )
                ctx.draw(label, at: CGPoint(x: center.x, y: labelRect.midY), anchor: .center)
            }
        }
    }

    // MARK: - HUD + interactions

    private var zoomHUD: some View {
        HStack(spacing: 1) {
            Button { adjustZoom(by: 1.0 / 1.15, anchor: nil) } label: {
                Image(systemName: "minus")
                    .font(.system(size: 10, weight: .medium))
                    .frame(width: 24, height: 24)
            }
            .buttonStyle(.plain)
            .foregroundStyle(Theme.Palette.textMuted)
            Text("\(Int(zoom * 100))%")
                .font(Theme.Font.mono)
                .foregroundStyle(Theme.Palette.textMuted)
                .frame(minWidth: 44)
            Button { adjustZoom(by: 1.15, anchor: nil) } label: {
                Image(systemName: "plus")
                    .font(.system(size: 10, weight: .medium))
                    .frame(width: 24, height: 24)
            }
            .buttonStyle(.plain)
            .foregroundStyle(Theme.Palette.textMuted)
            Divider().frame(height: 14)
            Button {
                withAnimation(Theme.Motion.standard) { fitToView() }
            } label: {
                Image(systemName: "arrow.up.left.and.arrow.down.right.magnifyingglass")
                    .font(.system(size: 11))
                    .frame(width: 24, height: 24)
            }
            .buttonStyle(.plain)
            .foregroundStyle(Theme.Palette.textMuted)
            .help("Fit to view")
        }
        .padding(.horizontal, 4)
        .background(
            RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                .fill(Theme.Palette.bgSurface.opacity(0.95))
        )
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                .stroke(Theme.Palette.border, lineWidth: Theme.Stroke.thin)
        )
    }

    private func hitTest(_ p: CGPoint) -> String? {
        let world = inverseTransform(p)
        for node in simNodes.reversed() {
            let dx = world.x - node.x, dy = world.y - node.y
            if dx * dx + dy * dy <= node.radius * node.radius {
                return node.id
            }
        }
        return nil
    }

    private var unifiedDragGesture: some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                if draggingNode == nil
                   && abs(value.translation.width) < 0.1
                   && abs(value.translation.height) < 0.1
                {
                    if let hit = hitTest(value.startLocation) {
                        draggingNode = hit
                        didDragNode = false
                    }
                }
                if let id = draggingNode {
                    if abs(value.translation.width) > 2 || abs(value.translation.height) > 2 {
                        didDragNode = true
                    }
                    if didDragNode {
                        let w = inverseTransform(value.location)
                        sim?.dragNode(id: id, to: w)
                        if let upd = sim?.tick() { simNodes = upd }
                        startTimer()
                    }
                } else {
                    pan = CGSize(
                        width:  lastDragPan.width  + value.translation.width,
                        height: lastDragPan.height + value.translation.height
                    )
                }
            }
            .onEnded { value in
                if let id = draggingNode {
                    if !didDragNode {
                        selectedNode = (selectedNode == id) ? nil : id
                    }
                    sim?.releaseNode(id: id)
                } else {
                    let traveled = abs(value.translation.width) + abs(value.translation.height)
                    if traveled < 3 { selectedNode = nil }
                    lastDragPan = pan
                }
                draggingNode = nil
                didDragNode = false
            }
    }

    private var pinchGesture: some Gesture {
        MagnificationGesture()
            .onChanged { v in
                if pinchStartZoom == 0 { pinchStartZoom = zoom }
                zoom = min(3.5, max(0.25, pinchStartZoom * v))
            }
            .onEnded { _ in pinchStartZoom = zoom }
    }

    private func adjustZoom(by factor: CGFloat, anchor: CGPoint?) {
        let newZoom = min(3.5, max(0.25, zoom * factor))
        guard let pivot = anchor else {
            withAnimation(Theme.Motion.quick) { zoom = newZoom }
            return
        }
        let world = inverseTransform(pivot)
        zoom = newZoom
        let after = transform(world)
        pan.width  -= after.x - pivot.x
        pan.height -= after.y - pivot.y
        lastDragPan = pan
    }

    private struct SizeKey: PreferenceKey {
        static var defaultValue: CGSize = .zero
        static func reduce(value: inout CGSize, nextValue: () -> CGSize) {
            value = nextValue()
        }
    }
}

private struct ScrollWheelHandler: NSViewRepresentable {
    let onScroll: (CGFloat, CGPoint) -> Void

    func makeNSView(context: Context) -> CatcherView {
        let v = CatcherView()
        v.onScroll = onScroll
        return v
    }
    func updateNSView(_ nsView: CatcherView, context: Context) {
        nsView.onScroll = onScroll
    }

    final class CatcherView: NSView {
        var onScroll: ((CGFloat, CGPoint) -> Void)?
        override var acceptsFirstResponder: Bool { true }
        override func scrollWheel(with event: NSEvent) {
            let delta = event.scrollingDeltaY != 0 ? event.scrollingDeltaY : event.deltaY * 10
            let local = self.convert(event.locationInWindow, from: nil)
            onScroll?(delta, CGPoint(x: local.x, y: bounds.height - local.y))
        }
    }
}
