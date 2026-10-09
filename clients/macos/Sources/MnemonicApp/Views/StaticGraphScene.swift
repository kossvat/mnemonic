import CoreGraphics
import Foundation
import MnemonicShared

enum StaticGraphKind: Hashable {
    case project, part, unit, technology, person, memory(String)

    var isCore: Bool {
        switch self {
        case .technology, .person: return false
        default: return true
        }
    }
}

enum StaticGraphShape {
    static func add(_ kind: StaticGraphKind, rect: CGRect, to path: CGMutablePath) {
        switch kind {
        case .project, .memory:
            path.addEllipse(in: rect)
        case .person:
            path.addEllipse(in: rect)
            path.addEllipse(in: rect.insetBy(dx: 4, dy: 4))
        case .unit:
            path.addRoundedRect(in: rect, cornerWidth: rect.width * 0.2, cornerHeight: rect.height * 0.2)
        case .part:
            path.move(to: CGPoint(x: rect.midX, y: rect.minY))
            path.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
            path.addLine(to: CGPoint(x: rect.midX, y: rect.maxY))
            path.addLine(to: CGPoint(x: rect.minX, y: rect.midY))
            path.closeSubpath()
        case .technology:
            for corner in 0..<6 {
                let angle = CGFloat(corner) * .pi / 3
                let point = CGPoint(x: rect.midX + cos(angle) * rect.width / 2,
                                    y: rect.midY + sin(angle) * rect.height / 2)
                if corner == 0 { path.move(to: point) } else { path.addLine(to: point) }
            }
            path.closeSubpath()
        }
    }

    static func contains(_ point: CGPoint, kind: StaticGraphKind, center: CGPoint, radius: CGFloat) -> Bool {
        switch kind {
        case .project, .person, .memory:
            return hypot(point.x - center.x, point.y - center.y) <= radius
        default:
            let path = CGMutablePath()
            add(kind, rect: CGRect(x: center.x - radius, y: center.y - radius,
                                  width: radius * 2, height: radius * 2), to: path)
            return path.contains(point)
        }
    }
}

struct StaticGraphNode: Equatable {
    let key: String
    let label: String
    let kind: StaticGraphKind
    let position: CGPoint
    let radius: CGFloat
    let state: UInt8
    let closed: Bool
    let priority: Double

    func screenRadius(at scale: CGFloat) -> CGFloat {
        let minimum: CGFloat
        if case .memory = kind { minimum = 10 } else { minimum = 6 }
        return max(minimum, radius * scale)
    }
}

struct StaticGraphEdge: Equatable {
    let source: Int
    let target: Int
    let kind: UInt8
    let weight: UInt32
    let originalIndex: Int
}

struct StaticGraphScene: Equatable {
    enum Layout: Equatable { case curated, memoryMap }
    let nodes: [StaticGraphNode]
    let edges: [StaticGraphEdge]
    let layout: Layout
    let focusKey: String?
    // Retain the exact response identity, including fields outside the canvas.
    let snapshot: CuratedGraphSnapshot?

    static let empty = StaticGraphScene(
        nodes: [], edges: [], layout: .curated, focusKey: nil, snapshot: nil)

    static func curated(_ snapshot: CuratedGraphSnapshot, focusKey: String? = nil) -> Self {
        let nodes = snapshot.nodes.map { node in
            let kind: StaticGraphKind
            switch node.kind {
            case .project: kind = .project
            case .part: kind = .part
            case .unit: kind = .unit
            case .technology: kind = .technology
            case .person: kind = .person
            }
            return StaticGraphNode(
                key: node.key, label: node.label, kind: kind,
                position: CGPoint(x: CGFloat(node.x), y: CGFloat(node.y)),
                radius: 14 + CGFloat(node.sizeBucket) * 4,
                state: node.state.rawValue, closed: node.isClosed,
                priority: Double(node.sizeBucket) + (node.kind == .project ? 10 : 0))
        }
        let edges = snapshot.links.enumerated().map { entry in
            StaticGraphEdge(source: entry.element.sourceIndex,
                            target: entry.element.targetIndex,
                            kind: entry.element.kind.rawValue,
                            weight: entry.element.memoryCount, originalIndex: entry.offset)
        }
        return Self(nodes: nodes, edges: edges, layout: .curated,
                    focusKey: focusKey, snapshot: snapshot)
    }

    static func memoryMap(nodes: [MemoryGraphNode], edges: [MemoryGraphEdge]) -> Self {
        let ranked = nodes.enumerated().sorted { first, second in
            let left = max(1, first.element.entityCount)
            let right = max(1, second.element.entityCount)
            return left == right ? first.offset < second.offset : left > right
        }
        var positions = Array(repeating: CGPoint.zero, count: nodes.count)
        for (rank, entry) in ranked.enumerated() {
            let radius = 40 + CGFloat(rank) * 22 / max(1, sqrt(CGFloat(rank + 1)))
            let angle = CGFloat(rank) * 2.4
            positions[entry.offset] = CGPoint(x: cos(angle) * radius, y: sin(angle) * radius)
        }
        var degree: [String: Int] = [:]
        for edge in edges {
            degree[edge.source, default: 0] += 1
            degree[edge.target, default: 0] += 1
        }
        let positioned = nodes.enumerated().map { entry in
            let node = entry.element
            let radius = min(28, max(12, 12 + log(CGFloat(max(1, node.entityCount)) + 1) * 6))
            return StaticGraphNode(
                key: node.id, label: node.title, kind: .memory(node.memoryType),
                position: positions[entry.offset], radius: radius, state: 0, closed: false,
                priority: Double(degree[node.id, default: 0]) * 10 + node.importance)
        }
        var index: [String: Int] = [:]
        for (offset, node) in nodes.enumerated() { index[node.id] = offset }
        let links = edges.enumerated().compactMap { entry -> StaticGraphEdge? in
            guard let source = index[entry.element.source], let target = index[entry.element.target]
            else { return nil }
            return StaticGraphEdge(source: source, target: target, kind: 3,
                                   weight: UInt32(clamping: entry.element.weight),
                                   originalIndex: entry.offset)
        }
        return Self(nodes: positioned, edges: links, layout: .memoryMap,
                    focusKey: nil, snapshot: nil)
    }
}
