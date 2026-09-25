import SwiftUI
import MnemonicShared
import AppKit

/// Force-directed canvas for the knowledge graph.
///
/// Obsidian-like interactions:
///   - Drag empty space → pan
///   - Drag node → grab and move it; physics keeps neighbors in step
///   - Click node → select; sidebar reads selection via binding
///   - Scroll wheel → zoom in/out around cursor
///   - Pinch trackpad → zoom
///   - Hover → highlight node + connected edges
struct GraphCanvas: View {
    let nodes: [GraphNode]
    let edges: [GraphEdge]
    let typeFilter: Set<String>
    let searchText: String
    @Binding var selectedNode: String?

    @State private var sim: GraphSimulation?
    @State private var simNodes: [SimNode] = []
    @State private var hoverNode: String?
    @State private var draggingNode: String?
    @State private var didDragNode: Bool = false
    @State private var pan: CGSize = .zero
    @State private var lastDragPan: CGSize = .zero
    @State private var zoom: CGFloat = 1.0
    @State private var pinchStartZoom: CGFloat = 1.0
    @State private var canvasSize: CGSize = .zero
    @State private var simTimer: Timer?

    var body: some View {
        GeometryReader { geo in
            ZStack {
                BackgroundGrid()

                Canvas(rendersAsynchronously: true) { ctx, _ in
                    drawEdges(ctx: ctx)
                    drawNodes(ctx: ctx)
                }
                .contentShape(Rectangle())
                .gesture(unifiedDragGesture)
                .simultaneousGesture(magnificationGesture)
                .onContinuousHover { phase in
                    switch phase {
                    case .active(let location):
                        let id = hitTest(location)
                        if id != hoverNode { hoverNode = id }
                    case .ended:
                        hoverNode = nil
                    }
                }
                .background(
                    GeometryReader { proxy in
                        Color.clear
                            .preference(key: SizeKey.self, value: proxy.size)
                    }
                )
                .onPreferenceChange(SizeKey.self) { size in
                    canvasSize = size
                    if simNodes.isEmpty { bootstrap(canvasSize: size) }
                }

                // Bottom-right zoom indicator + reset
                VStack {
                    Spacer()
                    HStack {
                        Spacer()
                        zoomControls
                    }
                }
                .padding(Theme.Space.md)
            }
            .background(
                ScrollWheelCatcher { delta, location in
                    handleScroll(delta: delta, at: location)
                }
            )
            .onAppear {
                bootstrap(canvasSize: geo.size)
                startTimer()
            }
            .onDisappear { stopTimer() }
            .onChange(of: nodes) { _ in
                bootstrap(canvasSize: canvasSize)
                startTimer()
            }
            .background(Theme.Palette.bgPrimary)
            .accessibilityLabel("Knowledge graph canvas")
        }
    }

    // MARK: - Zoom controls

    private var zoomControls: some View {
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
                withAnimation(Theme.Motion.standard) {
                    zoom = 1
                    pan = .zero
                    lastDragPan = .zero
                }
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

    // MARK: - Bootstrap & timer

    private func bootstrap(canvasSize: CGSize) {
        guard canvasSize.width > 1, canvasSize.height > 1 else { return }
        let raw = nodes.map { (id: $0.name, name: $0.name, type: $0.type, mentions: $0.mentions) }
        let positioned = GraphSimulation.initialLayout(nodes: raw, canvasSize: canvasSize)
        let simEdges = edges.map { SimEdge(source: $0.source, target: $0.target, weight: $0.weight) }
        let newSim = GraphSimulation(nodes: positioned, edges: simEdges)
        // Run physics for a chunk of frames headlessly so the layout is
        // ready when the canvas first paints — avoids the "everything in a
        // pile" first impression that scared the user.
        for _ in 0..<120 { _ = newSim.tick() }
        sim = newSim
        simNodes = newSim.tick()
        // After settling, fit-to-view so the whole graph is visible.
        fitToView()
    }

    /// Compute world bounding box of currently-visible nodes and adjust
    /// zoom + pan so everything fits with margin.
    private func fitToView() {
        let visible = simNodes.filter { isNodeVisible($0) }
        guard !visible.isEmpty, canvasSize.width > 0, canvasSize.height > 0 else { return }
        let minX = visible.map(\.x).min() ?? 0
        let maxX = visible.map(\.x).max() ?? 0
        let minY = visible.map(\.y).min() ?? 0
        let maxY = visible.map(\.y).max() ?? 0
        let pad: CGFloat = 80
        let worldW = max(1, maxX - minX) + pad * 2
        let worldH = max(1, maxY - minY) + pad * 2
        let zX = canvasSize.width  / worldW
        let zY = canvasSize.height / worldH
        let newZoom = min(2.0, max(0.4, min(zX, zY)))
        let cx = (minX + maxX) / 2
        let cy = (minY + maxY) / 2
        zoom = newZoom
        pan = CGSize(width: -cx * newZoom, height: -cy * newZoom)
        lastDragPan = pan
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

    // MARK: - Transforms

    private func transform(_ point: CGPoint) -> CGPoint {
        CGPoint(
            x: canvasSize.width  / 2 + (point.x * zoom) + pan.width,
            y: canvasSize.height / 2 + (point.y * zoom) + pan.height
        )
    }

    private func inverseTransform(_ screen: CGPoint) -> CGPoint {
        CGPoint(
            x: (screen.x - canvasSize.width  / 2 - pan.width)  / zoom,
            y: (screen.y - canvasSize.height / 2 - pan.height) / zoom
        )
    }

    private func isNodeVisible(_ node: SimNode) -> Bool {
        if !typeFilter.isEmpty && !typeFilter.contains(node.type.lowercased()) { return false }
        if !searchText.isEmpty {
            return node.name.lowercased().contains(searchText.lowercased())
        }
        return true
    }

    // MARK: - Drawing

    private func drawEdges(ctx: GraphicsContext) {
        let activeId = selectedNode ?? hoverNode
        let idIndex = Dictionary(uniqueKeysWithValues: simNodes.enumerated().map { ($1.id, $0) })

        for e in edges {
            guard let si = idIndex[e.source], let ti = idIndex[e.target] else { continue }
            let a = simNodes[si], b = simNodes[ti]
            guard isNodeVisible(a), isNodeVisible(b) else { continue }

            let p1 = transform(CGPoint(x: a.x, y: a.y))
            let p2 = transform(CGPoint(x: b.x, y: b.y))

            let isActive = activeId == e.source || activeId == e.target
            // Width does not depend on zoom — edges stay visible at any scale.
            let opacity: Double
            let width: CGFloat
            if isActive {
                opacity = 0.85; width = 1.8
            } else if activeId != nil {
                opacity = 0.12; width = 0.7
            } else {
                opacity = 0.45 + min(0.30, e.weight * 0.06)
                width = e.weight >= 3 ? 1.4 : 1.0
            }

            var path = Path()
            path.move(to: p1)
            path.addLine(to: p2)
            ctx.stroke(path, with: .color(Theme.Palette.textMuted.opacity(opacity)), lineWidth: width)
        }
    }

    private func drawNodes(ctx: GraphicsContext) {
        let activeId = selectedNode ?? hoverNode
        // Show labels by default only when zoom is decent OR node is active.
        // At far-out zoom, labels are illegible spam — hide them.
        let showAllLabels = zoom >= 0.85

        for node in simNodes {
            guard isNodeVisible(node) else { continue }
            let center = transform(CGPoint(x: node.x, y: node.y))
            // Node radius keeps a floor so nodes don't shrink past visibility.
            let radius = max(8, node.radius * zoom)
            let typeColor = EntityIcon.color(for: node.type)

            let isActive = activeId == node.id
            let isDim = activeId != nil && !isActive

            // Type-tinted fill so the graph actually looks colorful.
            // 18% tint at rest, 30% on active, 8% on dim.
            let fillTint: Double
            let strokeOpacity: Double
            if isActive {
                fillTint = 0.30; strokeOpacity = 1.0
            } else if isDim {
                fillTint = 0.08; strokeOpacity = 0.35
            } else {
                fillTint = 0.18; strokeOpacity = 0.85
            }

            let rect = CGRect(x: center.x - radius, y: center.y - radius,
                              width: radius * 2, height: radius * 2)
            // Background plate to prevent edges showing through tint.
            ctx.fill(Path(ellipseIn: rect),
                     with: .color(Theme.Palette.bgPrimary))
            ctx.fill(Path(ellipseIn: rect),
                     with: .color(typeColor.opacity(fillTint)))
            ctx.stroke(Path(ellipseIn: rect),
                       with: .color(typeColor.opacity(strokeOpacity)),
                       lineWidth: isActive ? 2.4 : 1.6)

            // Icon
            if radius >= 10 {
                let symbol = EntityIcon.symbol(for: node.type)
                let iconText = Text(Image(systemName: symbol))
                    .font(.system(size: radius * 0.85, weight: .medium))
                    .foregroundColor(typeColor.opacity(isDim ? 0.45 : 0.95))
                ctx.draw(iconText, at: center)
            }

            // Label visibility:
            //   - always for active (hover/selected)
            //   - on zoom ≥ 0.85 for everyone else
            //   - never when far zoomed out (avoids label spam in screenshot)
            let showLabel = isActive || showAllLabels
            if showLabel {
                let fontSize: CGFloat = isActive ? 11 : (zoom >= 1.2 ? 10 : 9)
                let label = Text(node.name)
                    .font(.system(size: fontSize, weight: isActive ? .semibold : .regular))
                    .foregroundColor(Theme.Palette.textPrimary.opacity(isDim ? 0.35 : 0.92))
                let labelPoint = CGPoint(x: center.x, y: center.y + radius + 9)
                // Background pill under label so it reads on top of edges.
                let labelSize = CGSize(width: CGFloat(node.name.count) * fontSize * 0.55 + 8, height: fontSize + 6)
                let labelRect = CGRect(
                    x: labelPoint.x - labelSize.width / 2,
                    y: labelPoint.y - labelSize.height / 2 + 1,
                    width: labelSize.width,
                    height: labelSize.height
                )
                ctx.fill(
                    Path(roundedRect: labelRect, cornerRadius: 3),
                    with: .color(Theme.Palette.bgPrimary.opacity(0.85))
                )
                ctx.draw(label, at: labelPoint, anchor: .center)
            }
        }
    }

    // MARK: - Hit-testing & unified gesture

    private func hitTest(_ screen: CGPoint) -> String? {
        let world = inverseTransform(screen)
        for node in simNodes.reversed() {
            guard isNodeVisible(node) else { continue }
            let dx = world.x - node.x, dy = world.y - node.y
            if dx * dx + dy * dy <= node.radius * node.radius {
                return node.id
            }
        }
        return nil
    }

    /// One DragGesture that decides: if the press began on a node → drag node,
    /// otherwise → pan canvas. Avoids two-gesture conflict.
    private var unifiedDragGesture: some Gesture {
        DragGesture(minimumDistance: 0)
            .onChanged { value in
                if draggingNode == nil && abs(value.translation.width) < 0.1 && abs(value.translation.height) < 0.1 {
                    // first event — decide route
                    if let hit = hitTest(value.startLocation) {
                        draggingNode = hit
                        didDragNode = false
                    }
                }
                if let id = draggingNode {
                    // moved more than threshold → really a drag
                    if abs(value.translation.width) > 2 || abs(value.translation.height) > 2 {
                        didDragNode = true
                    }
                    if didDragNode {
                        let world = inverseTransform(value.location)
                        sim?.dragNode(id: id, to: world)
                        if let updated = sim?.tick() { simNodes = updated }
                        startTimer()
                    }
                } else {
                    // pan canvas
                    pan = CGSize(
                        width:  lastDragPan.width  + value.translation.width,
                        height: lastDragPan.height + value.translation.height
                    )
                }
            }
            .onEnded { value in
                if let id = draggingNode {
                    if !didDragNode {
                        // It was a click, not a drag — toggle selection.
                        selectedNode = (selectedNode == id) ? nil : id
                    }
                    sim?.releaseNode(id: id)
                } else {
                    // Click on empty canvas — clear selection.
                    let traveled = abs(value.translation.width) + abs(value.translation.height)
                    if traveled < 3 { selectedNode = nil }
                    lastDragPan = pan
                }
                draggingNode = nil
                didDragNode = false
            }
    }

    /// Trackpad pinch zoom — anchors on the canvas center.
    private var magnificationGesture: some Gesture {
        MagnificationGesture()
            .onChanged { value in
                if pinchStartZoom == 0 { pinchStartZoom = zoom }
                let candidate = pinchStartZoom * value
                zoom = min(3.5, max(0.25, candidate))
            }
            .onEnded { _ in
                pinchStartZoom = zoom
            }
    }

    private func adjustZoom(by factor: CGFloat, anchor: CGPoint?) {
        let newZoom = min(3.5, max(0.25, zoom * factor))
        guard let pivot = anchor else {
            withAnimation(Theme.Motion.quick) { zoom = newZoom }
            return
        }
        // Zoom around `pivot` (screen coords) so the world point under the
        // cursor stays under the cursor.
        let world = inverseTransform(pivot)
        zoom = newZoom
        let after = transform(world)
        pan.width  -= after.x - pivot.x
        pan.height -= after.y - pivot.y
        lastDragPan = pan
    }

    private func handleScroll(delta: CGFloat, at location: CGPoint) {
        // Treat scroll delta as zoom — natural for mouse wheel users.
        // Trackpad two-finger scroll also routes here on macOS.
        let factor = exp(delta * 0.005)
        adjustZoom(by: factor, anchor: location)
    }

    // MARK: - Background dotted worksheet

    private struct BackgroundGrid: View {
        var body: some View {
            Canvas { ctx, size in
                let spacing: CGFloat = 24
                let cols = Int(size.width / spacing) + 1
                let rows = Int(size.height / spacing) + 1
                for r in 0..<rows {
                    for c in 0..<cols {
                        let x = CGFloat(c) * spacing
                        let y = CGFloat(r) * spacing
                        let dotRect = CGRect(x: x - 0.5, y: y - 0.5, width: 1, height: 1)
                        ctx.fill(Path(ellipseIn: dotRect),
                                 with: .color(Theme.Palette.textSubtle.opacity(0.18)))
                    }
                }
            }
        }
    }

    private struct SizeKey: PreferenceKey {
        static var defaultValue: CGSize = .zero
        static func reduce(value: inout CGSize, nextValue: () -> CGSize) {
            value = nextValue()
        }
    }
}

// MARK: - ScrollWheel catcher (NSViewRepresentable for raw NSEvent access)

/// Captures scrollWheel events anywhere over the canvas. SwiftUI's
/// `.scrollDisabled` modifier won't help — we need the raw delta so we
/// can route it into zoom + cursor anchor.
private struct ScrollWheelCatcher: NSViewRepresentable {
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
            // Negative deltaY = scroll up → zoom in.
            let delta = event.scrollingDeltaY != 0 ? event.scrollingDeltaY : event.deltaY * 10
            let local = self.convert(event.locationInWindow, from: nil)
            // Flip Y because AppKit origin is bottom-left.
            let point = CGPoint(x: local.x, y: bounds.height - local.y)
            onScroll?(delta, point)
        }
    }
}
