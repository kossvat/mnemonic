import AppKit
import CoreText
import MnemonicShared

struct StaticGraphPaintStats: Equatable {
    var nodes = 0
    var edges = 0
    var labels = 0
}

/// Shared by the native canvas and the measured offscreen rendering path.
@MainActor
final class StaticGraphPainter {
    private struct Label {
        let line: CTLine
        let width: CGFloat
    }
    private struct NodeStyle: Hashable {
        let kind: StaticGraphKind
        let state: UInt8
        let emphasis: Int
        let closed: Bool
    }
    private struct EdgeStyle: Hashable {
        let kind: UInt8
        let emphasis: Int
        let heavy: Bool
    }
    private struct VisibleNode {
        let node: StaticGraphNode
        let center: CGPoint
        let radius: CGFloat
        let index: Int
    }
    private struct PositionedLabel {
        let line: CTLine
        let rect: CGRect
        let active: Bool
    }
    private var labels: [String: Label] = [:]
    private var preparedLabels: [Label?] = []
    private let font = CTFontCreateWithName(".AppleSystemUIFont" as CFString, 10, nil)

    func prepare(_ scene: StaticGraphScene) {
        let texts = Set(scene.nodes.map { node in Self.title(node.label) })
        labels = labels.filter { entry in texts.contains(entry.key) }
        for text in texts where labels[text] == nil {
            let attributes: [NSAttributedString.Key: Any] = [
                NSAttributedString.Key(kCTFontAttributeName as String): font,
                NSAttributedString.Key(kCTForegroundColorFromContextAttributeName as String): true
            ]
            let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attributes))
            labels[text] = Label(line: line, width: CGFloat(CTLineGetTypographicBounds(line, nil, nil, nil)))
        }
        preparedLabels = scene.nodes.map { node in labels[Self.title(node.label)] }
    }

    @discardableResult
    func paint(scene: StaticGraphScene, viewport: StaticGraphViewport, bounds: CGRect,
               selected: String?, hovered: String?, context: CGContext,
               labelLimit: Int = 100) -> StaticGraphPaintStats {
        var stats = StaticGraphPaintStats()
        guard bounds.width > 0, bounds.height > 0 else { return stats }
        context.saveGState()
        defer { context.restoreGState() }
        context.clip(to: bounds)
        // Resolve the dynamic theme once per frame, including native appearance changes.
        let background = NSColor(Theme.Palette.bgPrimary).usingColorSpace(.deviceRGB)!.cgColor
        let foreground = NSColor(Theme.Palette.textPrimary).cgColor
        let muted = NSColor(Theme.Palette.textMuted).cgColor
        let labelBackground = background.copy(alpha: 0.92) ?? background
        let labelForeground = foreground.copy(alpha: 0.95) ?? foreground
        let labelDimmed = foreground.copy(alpha: 0.45) ?? foreground
        var kindColors: [StaticGraphKind: CGColor] = [:]
        context.setFillColor(background)
        context.fill(bounds)
        let active = selected ?? hovered
        let points = scene.nodes.map { node in viewport.screen(node.position, in: bounds) }
        var edgePaths: [EdgeStyle: CGMutablePath] = [:]
        for edge in scene.edges {
            guard points.indices.contains(edge.source), points.indices.contains(edge.target) else { continue }
            let start = points[edge.source], end = points[edge.target]
            let extent = CGRect(x: min(start.x, end.x), y: min(start.y, end.y),
                                width: max(1, abs(start.x - end.x)), height: max(1, abs(start.y - end.y)))
            guard extent.insetBy(dx: -3, dy: -3).intersects(bounds) else { continue }
            let touches = active == scene.nodes[edge.source].key || active == scene.nodes[edge.target].key
            let style = EdgeStyle(kind: edge.kind, emphasis: touches ? 2 : (active == nil ? 1 : 0),
                                  heavy: edge.weight >= 3)
            let path = edgePaths[style] ?? CGMutablePath()
            path.move(to: start)
            path.addLine(to: end)
            edgePaths[style] = path
            stats.edges += 1
        }
        for (style, path) in edgePaths {
            context.addPath(path)
            let opacity: CGFloat = style.emphasis == 2 ? 0.85 : (style.emphasis == 0 ? 0.1 : 0.38)
            context.setStrokeColor(muted.copy(alpha: opacity) ?? muted)
            context.setLineWidth(style.emphasis == 2 ? 1.8 : (style.heavy ? 1.2 : 0.8))
            let dash: [CGFloat] = style.kind == 1 ? [6, 4] : (style.kind == 2 ? [2, 4] : [])
            context.setLineDash(phase: 0, lengths: dash)
            context.strokePath()
        }
        context.setLineDash(phase: 0, lengths: [])
        var nodePaths: [NodeStyle: CGMutablePath] = [:]
        var visible: [VisibleNode] = []
        for (index, node) in scene.nodes.enumerated() {
            let center = points[index], radius = node.screenRadius(at: viewport.scale)
            let rect = CGRect(x: center.x - radius, y: center.y - radius,
                              width: radius * 2, height: radius * 2)
            guard rect.intersects(bounds) else { continue }
            let style = NodeStyle(kind: node.kind, state: node.state,
                                  emphasis: active == node.key ? 2 : (active == nil ? 1 : 0),
                                  closed: node.closed)
            let path = nodePaths[style] ?? CGMutablePath()
            StaticGraphShape.add(node.kind, rect: rect, to: path)
            nodePaths[style] = path
            visible.append(VisibleNode(node: node, center: center, radius: radius, index: index))
            stats.nodes += 1
        }
        for (style, path) in nodePaths {
            let recency: CGFloat = [1, 0.67, 0.4, 0.25][min(3, Int(style.state))]
            let color = kindColors[style.kind] ?? Self.color(style.kind).cgColor
            kindColors[style.kind] = color
            context.addPath(path)
            let opacity = (style.emphasis == 2 ? 0.3 : 0.13) * recency
            context.setFillColor(Self.blend(color, over: background, opacity: opacity))
            context.setStrokeColor(color.copy(alpha: (style.emphasis == 0 ? 0.3 : 0.95) * recency) ?? color)
            context.setLineWidth(style.emphasis == 2 ? 2.5 : 1.5)
            context.setLineDash(phase: 0, lengths: style.closed ? [3, 2] : [])
            context.drawPath(using: .fillStroke)
        }
        context.setLineDash(phase: 0, lengths: [])
        visible.sort { first, second in
            let firstActive = first.node.key == active
            let secondActive = second.node.key == active
            if firstActive != secondActive { return firstActive }
            if first.radius != second.radius { return first.radius > second.radius }
            if first.node.priority != second.node.priority { return first.node.priority > second.node.priority }
            return first.index < second.index
        }
        var occupied: [CGRect] = []
        var positioned: [PositionedLabel] = []
        for item in visible {
            guard positioned.count < min(100, max(0, labelLimit)) else { break }
            let isActive = item.node.key == active
            guard isActive || item.radius >= 8 else { continue }
            guard preparedLabels.indices.contains(item.index),
                  let label = preparedLabels[item.index], label.width > 0 else { continue }
            let rect = CGRect(x: item.center.x - label.width / 2 - 4,
                              y: item.center.y + item.radius + 4, width: label.width + 8, height: 15)
            guard bounds.contains(rect) else { continue }
            guard isActive || !occupied.contains(where: { other in other.intersects(rect) }) else { continue }
            occupied.append(rect)
            positioned.append(PositionedLabel(line: label.line, rect: rect, active: isActive))
        }
        context.setFillColor(labelBackground)
        context.fill(occupied)
        context.saveGState()
        context.scaleBy(x: 1, y: -1)
        context.textMatrix = .identity
        context.setFillColor(active == nil ? labelForeground : labelDimmed)
        for label in positioned {
            context.textPosition = CGPoint(x: label.rect.minX + 4, y: -label.rect.minY - 11)
            if active != nil { context.setFillColor(label.active ? labelForeground : labelDimmed) }
            CTLineDraw(label.line, context)
            stats.labels += 1
        }
        context.restoreGState()
        return stats
    }

    private static func blend(_ color: CGColor, over background: CGColor, opacity: CGFloat) -> CGColor {
        let space = CGColorSpaceCreateDeviceRGB()
        guard let foreground = color.converted(to: space, intent: .defaultIntent, options: nil)?.components,
              let base = background.components, foreground.count == 4, base.count == 4 else { return color }
        let channels = (0..<3).map { index in foreground[index] * opacity + base[index] * (1 - opacity) }
        return CGColor(colorSpace: space, components: channels + [1]) ?? color
    }

    private static func title(_ value: String) -> String {
        value.count > 36 ? String(value.prefix(33)) + "…" : value
    }

    static func color(_ kind: StaticGraphKind) -> NSColor {
        switch kind {
        case .project: return NSColor(Theme.Palette.entityProject)
        case .part: return NSColor(Theme.Palette.entityModule)
        case .unit: return NSColor(Theme.Palette.entityFile)
        case .technology: return NSColor(Theme.Palette.entityTech)
        case .person: return NSColor(Theme.Palette.entityPerson)
        case .memory(let type):
            return NSColor(MemoryTypeIcon.color(for: type))
        }
    }
}
