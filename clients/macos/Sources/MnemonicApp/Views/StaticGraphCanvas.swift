import AppKit
import SwiftUI

/// One static surface for both graph sources. Only native input and invalidation draw it.
struct StaticGraphCanvas: NSViewRepresentable {
    let scene: StaticGraphScene
    let selectedNode: String?
    let onNode: (String?) -> Void
    let onEdge: (Int) -> Void
    var onDraw: (() -> Void)? = nil
    var viewportState: StaticGraphViewportState? = nil
    var paintRevision: UInt64 = 0

    func makeNSView(context: Context) -> StaticGraphNativeView {
        let view = StaticGraphNativeView(viewportState: viewportState)
        view.update(scene: scene, selected: selectedNode, onNode: onNode, onEdge: onEdge,
                    onDraw: onDraw, paintRevision: paintRevision)
        return view
    }

    func updateNSView(_ view: StaticGraphNativeView, context: Context) {
        view.update(scene: scene, selected: selectedNode, onNode: onNode, onEdge: onEdge,
                    onDraw: onDraw, paintRevision: paintRevision)
    }
}

@MainActor
final class StaticGraphNativeView: NSView {
    private(set) var scene = StaticGraphScene.empty
    private let viewportState: StaticGraphViewportState
    private(set) var viewport: StaticGraphViewport {
        get { viewportState.viewport }
        set { viewportState.viewport = newValue }
    }
    private(set) var completedPaints = 0
    private var grid = StaticGraphGrid(nodes: [])
    private let painter = StaticGraphPainter()
    private var selected: String?
    private var hovered: String?
    private var onNode: ((String?) -> Void)?
    private var onEdge: ((Int) -> Void)?
    private var onDraw: (() -> Void)?
    private var lastPaintRevision: UInt64 = 0
    private var didFit: Bool {
        get { viewportState.didFit }
        set { viewportState.didFit = newValue }
    }
    private var mouseOrigin: CGPoint?
    private var mousePrevious: CGPoint?
    private var didDrag = false
    private var hoverTracking: NSTrackingArea?
    private let zoomLabel = NSTextField(labelWithString: "100%")
    private let coreButton = NSButton(title: "Core", target: nil, action: nil)

    override var isFlipped: Bool { true }
    override var isOpaque: Bool { true }
    override var acceptsFirstResponder: Bool { true }

    init(viewportState: StaticGraphViewportState? = nil) {
        self.viewportState = viewportState ?? StaticGraphViewportState()
        super.init(frame: .zero)
        let fitButton = NSButton(title: "Fit all", target: self, action: #selector(fitAll))
        let minusButton = NSButton(title: "−", target: self, action: #selector(zoomOut))
        let plusButton = NSButton(title: "+", target: self, action: #selector(zoomIn))
        coreButton.target = self
        coreButton.action = #selector(fitCore)
        coreButton.toolTip = "Show the project core or expanded focus"
        fitButton.toolTip = "Show every node, including technology and people rings"
        minusButton.toolTip = "Zoom out"
        plusButton.toolTip = "Zoom in"
        zoomLabel.font = .monospacedDigitSystemFont(ofSize: 10, weight: .regular)
        zoomLabel.alignment = .center
        zoomLabel.widthAnchor.constraint(greaterThanOrEqualToConstant: 38).isActive = true
        let controls = NSStackView(views: [coreButton, fitButton, minusButton, zoomLabel, plusButton])
        controls.orientation = .horizontal
        controls.spacing = 4
        controls.edgeInsets = NSEdgeInsets(top: 4, left: 6, bottom: 4, right: 6)
        controls.wantsLayer = true
        controls.layer?.cornerRadius = 6
        for button in [coreButton, fitButton, minusButton, plusButton] {
            button.bezelStyle = .rounded
            button.controlSize = .small
            button.font = .systemFont(ofSize: 10)
        }
        controls.translatesAutoresizingMaskIntoConstraints = false
        addSubview(controls)
        NSLayoutConstraint.activate([
            controls.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -12),
            controls.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -12)
        ])
        setAccessibilityLabel("Interactive static graph")
        setAccessibilityHelp("Drag to pan. Scroll or pinch to zoom. Select a node or relationship.")
        needsDisplay = true
    }

    required init?(coder: NSCoder) { nil }

    func update(scene next: StaticGraphScene, selected: String?,
                onNode: @escaping (String?) -> Void, onEdge: @escaping (Int) -> Void,
                onDraw: (() -> Void)? = nil, paintRevision: UInt64 = 0) {
        self.onNode = onNode
        self.onEdge = onEdge
        self.onDraw = onDraw
        let sceneChanged = scene != next
        let selectionChanged = self.selected != selected
        let paintRequested = lastPaintRevision != paintRevision
        lastPaintRevision = paintRevision
        let resetViewport = !next.nodes.isEmpty && (viewportState.layout != next.layout
            || viewportState.focusKey != next.focusKey)
        self.selected = selected
        if sceneChanged {
            scene = next
            grid = StaticGraphGrid(nodes: next.nodes)
            painter.prepare(next)
            if let key = hovered, !next.nodes.contains(where: { node in node.key == key }) { hovered = nil }
            coreButton.isHidden = next.layout == .memoryMap
        }
        if !next.nodes.isEmpty {
            viewportState.layout = next.layout
            viewportState.focusKey = next.focusKey
            if resetViewport { didFit = false }
        }
        fitIfNeeded()
        if sceneChanged || selectionChanged || paintRequested { needsDisplay = true }
    }

    override func layout() {
        super.layout()
        fitIfNeeded()
    }

    private func fitIfNeeded() {
        guard !didFit, !scene.nodes.isEmpty, bounds.width > 1, bounds.height > 1 else { return }
        viewport.fit(scene, in: bounds)
        didFit = true
        viewportChanged()
    }

    override func draw(_ dirtyRect: NSRect) {
        guard let context = NSGraphicsContext.current?.cgContext else { return }
        fitIfNeeded()
        let stats = painter.paint(scene: scene, viewport: viewport, bounds: bounds,
                                  selected: selected, hovered: hovered, context: context)
        if stats.nodes > 0 {
            completedPaints += 1
            onDraw?()
        }
    }

    override func viewDidChangeEffectiveAppearance() {
        super.viewDidChangeEffectiveAppearance()
        needsDisplay = true
    }

    override func viewDidMoveToWindow() {
        super.viewDidMoveToWindow()
        if window != nil { needsDisplay = true }
    }

    override func updateTrackingAreas() {
        if let tracking = hoverTracking { removeTrackingArea(tracking) }
        let tracking = NSTrackingArea(rect: .zero,
                                      options: [.mouseMoved, .mouseEnteredAndExited, .activeInKeyWindow, .inVisibleRect],
                                      owner: self, userInfo: nil)
        addTrackingArea(tracking)
        hoverTracking = tracking
        super.updateTrackingAreas()
    }

    override func mouseMoved(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        let hit = grid.node(at: point, scene: scene, viewport: viewport, bounds: bounds)
        if hit != hovered { hovered = hit; needsDisplay = true }
    }

    override func mouseExited(with event: NSEvent) {
        if hovered != nil { hovered = nil; needsDisplay = true }
    }

    override func mouseDown(with event: NSEvent) {
        window?.makeFirstResponder(self)
        let point = convert(event.locationInWindow, from: nil)
        mouseOrigin = point
        mousePrevious = point
        didDrag = false
    }

    override func mouseDragged(with event: NSEvent) {
        let point = convert(event.locationInWindow, from: nil)
        guard let origin = mouseOrigin, let previous = mousePrevious else { return }
        if hypot(point.x - origin.x, point.y - origin.y) > 3 { didDrag = true }
        if didDrag {
            viewport.pan(by: CGSize(width: point.x - previous.x, height: point.y - previous.y))
            viewportChanged()
        }
        mousePrevious = point
    }

    override func mouseUp(with event: NSEvent) {
        defer { mouseOrigin = nil; mousePrevious = nil; didDrag = false }
        guard mouseOrigin != nil, !didDrag else { return }
        let point = convert(event.locationInWindow, from: nil)
        if let key = grid.node(at: point, scene: scene, viewport: viewport, bounds: bounds) {
            onNode?(key == selected ? nil : key)
        } else if let edge = StaticGraphGrid.edge(at: point, scene: scene, viewport: viewport, bounds: bounds) {
            onEdge?(edge)
        } else {
            onNode?(nil)
        }
    }

    override func scrollWheel(with event: NSEvent) {
        let delta = event.scrollingDeltaY != 0 ? event.scrollingDeltaY : event.deltaY * 10
        zoom(by: exp(delta * 0.005), anchor: convert(event.locationInWindow, from: nil))
    }

    override func magnify(with event: NSEvent) {
        zoom(by: max(0.01, 1 + event.magnification), anchor: convert(event.locationInWindow, from: nil))
    }

    private func zoom(by factor: CGFloat, anchor: CGPoint? = nil) {
        viewport.zoom(by: factor, anchor: anchor ?? CGPoint(x: bounds.midX, y: bounds.midY), in: bounds)
        viewportChanged()
    }

    private func viewportChanged() {
        zoomLabel.stringValue = "\(Int((viewport.scale * 100).rounded()))%"
        needsDisplay = true
    }

    @objc private func fitCore() {
        viewport.fit(scene, in: bounds)
        viewportChanged()
    }

    @objc private func fitAll() {
        viewport.fit(scene, in: bounds, all: true)
        viewportChanged()
    }

    @objc private func zoomOut() { zoom(by: 1 / 1.2) }
    @objc private func zoomIn() { zoom(by: 1.2) }
}
