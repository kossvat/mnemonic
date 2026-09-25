import Foundation
import CoreGraphics

/// D3-style force simulation for the knowledge graph view.
///
/// Three forces:
///   1. Charge   — every pair repels with inverse-square (Coulomb-like)
///   2. Link     — edges pull connected nodes toward an ideal length (spring)
///   3. Centering — weak pull toward origin (keeps the graph in view)
///
/// Settles fast (kinetic-energy threshold) rather than running forever, so the
/// graph stops "breathing" once it's laid out. Matches our "constellation
/// map, not spider web" design intent.
struct SimNode: Identifiable, Equatable {
    let id: String
    let name: String
    let type: String
    let mentions: Int

    var x: CGFloat
    var y: CGFloat
    var vx: CGFloat = 0
    var vy: CGFloat = 0

    /// Pinned by user drag — physics ignores this node until released.
    var pinned: Bool = false

    /// Radius in pt — derived from mention count. 24..56pt range.
    var radius: CGFloat {
        let base: CGFloat = 12
        let bump = log(CGFloat(max(1, mentions)) + 1) * 6
        return min(28, max(12, base + bump))
    }
}

struct SimEdge: Equatable {
    let source: String
    let target: String
    let weight: Double
}

final class GraphSimulation {
    private(set) var nodes: [SimNode]
    private let edges: [SimEdge]
    private var iterations: Int = 0

    // Tunables
    private let chargeStrength: CGFloat = 800   // repulsion magnitude
    private let centerStrength: CGFloat = 0.02  // pull toward origin
    private let linkDistance:   CGFloat = 120   // ideal edge length
    private let linkStrength:   CGFloat = 0.15  // spring constant
    private let damping:        CGFloat = 0.85
    private let maxIterations             = 240
    private let kineticEnergyStop: CGFloat = 0.05

    init(nodes: [SimNode], edges: [SimEdge]) {
        self.nodes = nodes
        self.edges = edges
    }

    /// True when simulation has settled (or hit iteration cap).
    var isSettled: Bool {
        iterations >= maxIterations || kineticEnergy < kineticEnergyStop
    }

    var kineticEnergy: CGFloat {
        nodes.reduce(0) { $0 + $1.vx * $1.vx + $1.vy * $1.vy }
    }

    /// Run one tick of physics. Returns updated nodes.
    func tick() -> [SimNode] {
        guard !isSettled else { return nodes }
        iterations += 1

        let n = nodes.count
        var fx = Array(repeating: CGFloat(0), count: n)
        var fy = Array(repeating: CGFloat(0), count: n)

        // 1. Charge (all pairs repel)
        for i in 0..<n {
            for j in (i + 1)..<n {
                let dx = nodes[j].x - nodes[i].x
                let dy = nodes[j].y - nodes[i].y
                var dist2 = dx * dx + dy * dy
                if dist2 < 1 { dist2 = 1 }
                let f = chargeStrength / dist2
                let dist = sqrt(dist2)
                let nx = dx / dist
                let ny = dy / dist
                fx[i] -= nx * f
                fy[i] -= ny * f
                fx[j] += nx * f
                fy[j] += ny * f
            }
        }

        // 2. Link springs (only when endpoints exist)
        let indexOf: [String: Int] = Dictionary(uniqueKeysWithValues: nodes.enumerated().map { ($1.id, $0) })
        for e in edges {
            guard let si = indexOf[e.source], let ti = indexOf[e.target] else { continue }
            let dx = nodes[ti].x - nodes[si].x
            let dy = nodes[ti].y - nodes[si].y
            let dist = sqrt(dx * dx + dy * dy)
            guard dist > 0.01 else { continue }
            let displacement = dist - linkDistance
            // Higher-weight edges pull harder, but log-scaled to avoid runaway.
            let weight = CGFloat(min(2.0, log(e.weight + 1) + 1))
            let strength = linkStrength * weight
            let force = displacement * strength
            let nx = dx / dist
            let ny = dy / dist
            fx[si] += nx * force
            fy[si] += ny * force
            fx[ti] -= nx * force
            fy[ti] -= ny * force
        }

        // 3. Centering (very gentle, keeps the cloud near origin)
        for i in 0..<n {
            fx[i] -= nodes[i].x * centerStrength
            fy[i] -= nodes[i].y * centerStrength
        }

        // Integrate
        for i in 0..<n {
            if nodes[i].pinned {
                nodes[i].vx = 0
                nodes[i].vy = 0
                continue
            }
            nodes[i].vx = (nodes[i].vx + fx[i]) * damping
            nodes[i].vy = (nodes[i].vy + fy[i]) * damping
            nodes[i].x += nodes[i].vx
            nodes[i].y += nodes[i].vy
        }

        return nodes
    }

    /// Move a pinned node — used by drag interactions. Pins, sets position,
    /// nudges connected neighbors so the layout feels alive.
    func dragNode(id: String, to point: CGPoint) {
        guard let i = nodes.firstIndex(where: { $0.id == id }) else { return }
        nodes[i].pinned = true
        nodes[i].x = point.x
        nodes[i].y = point.y
        nodes[i].vx = 0
        nodes[i].vy = 0
        // Wake the simulation back up so neighbors follow.
        iterations = max(0, iterations - 30)
    }

    func releaseNode(id: String) {
        if let i = nodes.firstIndex(where: { $0.id == id }) {
            nodes[i].pinned = false
        }
    }

    /// Place nodes in a circle around origin as the starting layout — beats
    /// a random scatter for visual sanity. Larger (more-mentioned) nodes
    /// sit closer to center.
    static func initialLayout(
        nodes: [(id: String, name: String, type: String, mentions: Int)],
        canvasSize: CGSize
    ) -> [SimNode] {
        let sorted = nodes.enumerated().sorted { $0.element.mentions > $1.element.mentions }
        let centerR: CGFloat = 40
        let radiusStep: CGFloat = 22

        var result: [SimNode] = Array(repeating: SimNode(
            id: "", name: "", type: "", mentions: 0, x: 0, y: 0
        ), count: nodes.count)

        for (rankIndex, item) in sorted.enumerated() {
            let originalIndex = item.offset
            let rank = rankIndex
            // Spiral out from origin
            let r = centerR + CGFloat(rank) * radiusStep / max(1, sqrt(CGFloat(rank + 1)))
            let angle = CGFloat(rank) * 2.4 // golden-angle-ish spiral
            let x = cos(angle) * r
            let y = sin(angle) * r
            result[originalIndex] = SimNode(
                id: item.element.id,
                name: item.element.name,
                type: item.element.type,
                mentions: item.element.mentions,
                x: x, y: y
            )
        }
        _ = canvasSize // reserved for future viewport-aware initial sizing
        return result
    }
}
