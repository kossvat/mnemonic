import AppKit
import Foundation

enum StaticGraphBitmapError: Error { case allocation, emptyMeasurement }

@MainActor
enum StaticGraphBitmap {
    static func context(width: Int, height: Int) throws -> CGContext {
        guard let context = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8,
                                      bytesPerRow: width * 4, space: CGColorSpaceCreateDeviceRGB(),
                                      bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue)
        else { throw StaticGraphBitmapError.allocation }
        return context
    }

    @discardableResult
    static func paint(_ scene: StaticGraphScene, viewport: StaticGraphViewport,
                      painter: StaticGraphPainter, context: CGContext,
                      labelLimit: Int = 100) -> StaticGraphPaintStats {
        let bounds = CGRect(x: 0, y: 0, width: context.width, height: context.height)
        context.saveGState()
        context.translateBy(x: 0, y: bounds.height)
        context.scaleBy(x: 1, y: -1)
        let stats = painter.paint(scene: scene, viewport: viewport, bounds: bounds,
                                  selected: nil, hovered: nil, context: context, labelLimit: labelLimit)
        context.restoreGState()
        context.flush()
        return stats
    }
}

struct StaticGraphBenchmarkResult {
    let frameCount: Int
    let p95Milliseconds: Double
    let maximumMilliseconds: Double
    let minimumPainted: StaticGraphPaintStats
}

/// Every measured frame draws fresh paths and CoreText lines into the same bitmap.
/// The native canvas uses this exact painter; only font shaping is prepared once.
@MainActor
enum StaticGraphBenchmark {
    static func scene() -> StaticGraphScene {
        let kinds: [StaticGraphKind] = [.project, .part, .unit, .technology, .person]
        var nodes: [StaticGraphNode] = []
        for index in 0..<150 {
            let column: Int = index % 15
            let row: Int = index / 15
            let position = CGPoint(x: CGFloat(column) * 86, y: CGFloat(row) * 82)
            let radius: CGFloat = 14 + CGFloat(index % 5) * 4
            let node = StaticGraphNode(
                key: "node-\(index)", label: String(format: "Node %03d", index),
                kind: kinds[index % kinds.count], position: position, radius: radius,
                state: UInt8(index % 3), closed: false, priority: Double(index % 5))
            nodes.append(node)
        }
        let edges = (0..<375).map { index in
            StaticGraphEdge(source: index % 150, target: (index % 150 + index / 150 + 1) % 150,
                            kind: UInt8(index % 4), weight: UInt32(index % 5), originalIndex: index)
        }
        return StaticGraphScene(nodes: nodes, edges: edges, layout: .curated, focusKey: nil, snapshot: nil)
    }

    static func run(warmupFrames: Int = 20, measuredFrames: Int = 120) throws -> StaticGraphBenchmarkResult {
        guard measuredFrames > 0 else { throw StaticGraphBitmapError.emptyMeasurement }
        let scene = scene()
        let painter = StaticGraphPainter()
        painter.prepare(scene)
        let context = try StaticGraphBitmap.context(width: 1400, height: 900)
        let viewport = StaticGraphViewport(center: CGPoint(x: 602, y: 369), scale: 1)
        for _ in 0..<max(0, warmupFrames) {
            StaticGraphBitmap.paint(scene, viewport: viewport, painter: painter, context: context)
        }
        var durations: [Double] = []
        var minimum = StaticGraphPaintStats(nodes: Int.max, edges: Int.max, labels: Int.max)
        for _ in 0..<measuredFrames {
            let start = ProcessInfo.processInfo.systemUptime
            let painted = StaticGraphBitmap.paint(scene, viewport: viewport, painter: painter, context: context)
            durations.append((ProcessInfo.processInfo.systemUptime - start) * 1000)
            minimum.nodes = min(minimum.nodes, painted.nodes)
            minimum.edges = min(minimum.edges, painted.edges)
            minimum.labels = min(minimum.labels, painted.labels)
        }
        durations.sort()
        let percentile = min(durations.count - 1, max(0, Int(ceil(Double(durations.count) * 0.95)) - 1))
        return StaticGraphBenchmarkResult(frameCount: measuredFrames,
                                          p95Milliseconds: durations[percentile],
                                          maximumMilliseconds: durations.last ?? 0,
                                          minimumPainted: minimum)
    }
}
