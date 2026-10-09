import CoreGraphics
import Foundation

/// A page can retain this across a temporary loading state without publishing draws.
@MainActor
final class StaticGraphViewportState {
    var viewport = StaticGraphViewport()
    var didFit = false
    var layout: StaticGraphScene.Layout?
    var focusKey: String?
}

struct StaticGraphViewport: Equatable {
    var center = CGPoint.zero
    var scale: CGFloat = 1

    func screen(_ world: CGPoint, in bounds: CGRect) -> CGPoint {
        CGPoint(x: bounds.midX + (world.x - center.x) * scale,
                y: bounds.midY + (world.y - center.y) * scale)
    }

    func world(_ point: CGPoint, in bounds: CGRect) -> CGPoint {
        CGPoint(x: center.x + (point.x - bounds.midX) / scale,
                y: center.y + (point.y - bounds.midY) / scale)
    }

    mutating func pan(by delta: CGSize) {
        center.x -= delta.width / scale
        center.y -= delta.height / scale
    }

    mutating func zoom(by factor: CGFloat, anchor: CGPoint, in bounds: CGRect) {
        guard factor.isFinite, factor > 0 else { return }
        let pivot = world(anchor, in: bounds)
        scale = min(4, max(0.02, scale * factor))
        center.x = pivot.x - (anchor.x - bounds.midX) / scale
        center.y = pivot.y - (anchor.y - bounds.midY) / scale
    }

    mutating func fit(_ scene: StaticGraphScene, in bounds: CGRect, all: Bool = false) {
        guard !scene.nodes.isEmpty, bounds.width > 1, bounds.height > 1 else { return }
        if !all, let focus = scene.focusKey,
           let node = scene.nodes.first(where: { node in node.key == focus }) {
            center = node.position
            scale = min(2, max(0.02, min(bounds.width, bounds.height) / 440))
            return
        }
        let core = scene.nodes.filter { node in node.kind.isCore }
        let visible = all || scene.layout == .memoryMap || core.isEmpty ? scene.nodes : core
        let minimumX = visible.map { node in node.position.x }.min() ?? 0
        let maximumX = visible.map { node in node.position.x }.max() ?? 0
        let minimumY = visible.map { node in node.position.y }.min() ?? 0
        let maximumY = visible.map { node in node.position.y }.max() ?? 0
        center = CGPoint(x: (minimumX + maximumX) / 2, y: (minimumY + maximumY) / 2)
        let width = max(1, maximumX - minimumX) + 160
        let height = max(1, maximumY - minimumY) + 160
        let minimumScale: CGFloat = scene.layout == .memoryMap ? 0.35 : 0.02
        scale = min(2, max(minimumScale, min(bounds.width / width, bounds.height / height)))
    }
}

/// Immutable world-space buckets. Zoom changes the query radius, never the index.
struct StaticGraphGrid {
    private struct Cell: Hashable { let x: Int; let y: Int }
    private var cells: [Cell: [Int]] = [:]
    private let cellSize: CGFloat = 128
    private let maximumRadius: CGFloat

    init(nodes: [StaticGraphNode]) {
        maximumRadius = nodes.map { node in node.radius }.max() ?? 0
        for (index, node) in nodes.enumerated() {
            let cell = Cell(x: Int(floor(node.position.x / cellSize)),
                            y: Int(floor(node.position.y / cellSize)))
            cells[cell, default: []].append(index)
        }
    }

    func node(at point: CGPoint, scene: StaticGraphScene,
              viewport: StaticGraphViewport, bounds: CGRect) -> String? {
        let world = viewport.world(point, in: bounds)
        let reach = max(maximumRadius, 10 / viewport.scale)
        let lowerX = Int(floor((world.x - reach) / cellSize))
        let upperX = Int(floor((world.x + reach) / cellSize))
        let lowerY = Int(floor((world.y - reach) / cellSize))
        let upperY = Int(floor((world.y + reach) / cellSize))
        var best: (distance: CGFloat, index: Int)?
        for x in lowerX...upperX {
            for y in lowerY...upperY {
                for index in cells[Cell(x: x, y: y), default: []] {
                    let node = scene.nodes[index]
                    let center = viewport.screen(node.position, in: bounds)
                    let radius = node.screenRadius(at: viewport.scale)
                    let distance = hypot(center.x - point.x, center.y - point.y) / radius
                    guard StaticGraphShape.contains(point, kind: node.kind, center: center, radius: radius)
                    else { continue }
                    if best == nil || distance < best!.distance
                        || (distance == best!.distance && index < best!.index) {
                        best = (distance, index)
                    }
                }
            }
        }
        return best.map { match in scene.nodes[match.index].key }
    }

    static func edge(at point: CGPoint, scene: StaticGraphScene,
                     viewport: StaticGraphViewport, bounds: CGRect) -> Int? {
        var best: (distance: CGFloat, index: Int)?
        for edge in scene.edges {
            guard scene.nodes.indices.contains(edge.source), scene.nodes.indices.contains(edge.target)
            else { continue }
            let start = viewport.screen(scene.nodes[edge.source].position, in: bounds)
            let end = viewport.screen(scene.nodes[edge.target].position, in: bounds)
            let dx = end.x - start.x, dy = end.y - start.y
            let length = dx * dx + dy * dy
            let progress = length == 0 ? 0 : min(1, max(0,
                ((point.x - start.x) * dx + (point.y - start.y) * dy) / length))
            let distance = hypot(point.x - start.x - progress * dx,
                                 point.y - start.y - progress * dy)
            if distance <= 6 && (best == nil || distance < best!.distance) {
                best = (distance, edge.originalIndex)
            }
        }
        return best?.index
    }
}
