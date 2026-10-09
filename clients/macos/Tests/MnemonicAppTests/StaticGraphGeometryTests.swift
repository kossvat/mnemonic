import CoreGraphics
import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

final class StaticGraphGeometryTests: XCTestCase {
    func testMemoryMapPreservesLegacyRankSpiralRadiiAndInputOrder() {
        let nodes = [memory("low", count: 0), memory("high", count: 8), memory("tie", count: 8)]
        let scene = StaticGraphScene.memoryMap(nodes: nodes, edges: [])
        XCTAssertEqual(scene.nodes.map(\.key), ["low", "high", "tie"])
        for (index, rank) in [(0, 2), (1, 0), (2, 1)] {
            let radius = 40 + CGFloat(rank) * 22 / max(1, sqrt(CGFloat(rank + 1)))
            XCTAssertEqual(scene.nodes[index].position.x, cos(CGFloat(rank) * 2.4) * radius, accuracy: 0.000_001)
            XCTAssertEqual(scene.nodes[index].position.y, sin(CGFloat(rank) * 2.4) * radius, accuracy: 0.000_001)
            let mentions = max(1, nodes[index].entityCount)
            XCTAssertEqual(scene.nodes[index].radius, min(28, max(12, 12 + log(CGFloat(mentions) + 1) * 6)))
        }
    }

    func testCuratedAdapterPreservesCoordinatesLinkOrderAndExactSnapshotIdentity() throws {
        let snapshot = try graphFixture()
        let scene = StaticGraphScene.curated(snapshot)
        XCTAssertEqual(scene.snapshot, snapshot)
        XCTAssertEqual(scene.nodes.map(\.key), snapshot.nodes.map(\.key))
        XCTAssertEqual(scene.edges.map(\.originalIndex), Array(snapshot.links.indices))
        for (rendered, source) in zip(scene.nodes, snapshot.nodes) {
            XCTAssertEqual(rendered.position, CGPoint(x: CGFloat(source.x), y: CGFloat(source.y)))
            XCTAssertEqual(rendered.radius, 14 + CGFloat(source.sizeBucket) * 4)
        }
        let original = scene
        var viewport = StaticGraphViewport()
        let bounds = CGRect(x: 0, y: 0, width: 800, height: 600)
        viewport.fit(scene, in: bounds)
        let coreScale = viewport.scale
        viewport.fit(scene, in: bounds, all: true)
        XCTAssertLessThan(viewport.scale, coreScale)
        viewport.pan(by: CGSize(width: 90, height: -42))
        viewport.zoom(by: 2, anchor: .zero, in: bounds)
        XCTAssertEqual(scene, original, "Interaction changes only the viewport")
        viewport.fit(.curated(snapshot, focusKey: snapshot.nodes.last?.key), in: bounds)
        XCTAssertEqual(viewport.center, scene.nodes.last?.position)
    }

    func testZoomKeepsCursorAnchorAndPanUsesScreenDistance() {
        let bounds = CGRect(x: 20, y: 10, width: 900, height: 500)
        var viewport = StaticGraphViewport(center: CGPoint(x: -17, y: 31), scale: 0.8)
        let anchor = CGPoint(x: 180, y: 420)
        let before = viewport.world(anchor, in: bounds)
        viewport.zoom(by: 2.4, anchor: anchor, in: bounds)
        let after = viewport.world(anchor, in: bounds)
        XCTAssertEqual(before.x, after.x, accuracy: 0.000_001)
        XCTAssertEqual(before.y, after.y, accuracy: 0.000_001)
        let point = CGPoint(x: 75, y: -31)
        let screenBefore = viewport.screen(point, in: bounds)
        viewport.pan(by: CGSize(width: 20, height: -50))
        let screenAfter = viewport.screen(point, in: bounds)
        XCTAssertEqual(screenAfter.x - screenBefore.x, 20, accuracy: 0.000_001)
        XCTAssertEqual(screenAfter.y - screenBefore.y, -50, accuracy: 0.000_001)
        let restored = viewport.world(screenAfter, in: bounds)
        XCTAssertEqual(restored.x, point.x, accuracy: 0.000_001)
        XCTAssertEqual(restored.y, point.y, accuracy: 0.000_001)
    }

    func testGridUsesVisibleMinimumRadiusAtLowZoomAndStableOverlapWinner() {
        let nodes = [memory("first", count: 1), memory("second", count: 1)]
        let scene = StaticGraphScene.memoryMap(nodes: nodes, edges: [])
        let grid = StaticGraphGrid(nodes: scene.nodes)
        let viewport = StaticGraphViewport(center: scene.nodes[0].position, scale: 0.02)
        let bounds = CGRect(x: 0, y: 0, width: 600, height: 400)
        XCTAssertEqual(grid.node(at: CGPoint(x: 309, y: 200), scene: scene,
                                 viewport: viewport, bounds: bounds), "first")
        XCTAssertNil(grid.node(at: CGPoint(x: 325, y: 200), scene: scene,
                              viewport: viewport, bounds: bounds))
    }

    func testMemoryEdgeHitReturnsOriginalIndexAfterMissingEndpointIsDropped() {
        let nodes = [memory("first", count: 1), memory("second", count: 2)]
        let edges = [MemoryGraphEdge(source: "missing", target: "second", weight: 1, shared: []),
                     MemoryGraphEdge(source: "first", target: "second", weight: 1, shared: [])]
        let scene = StaticGraphScene.memoryMap(nodes: nodes, edges: edges)
        XCTAssertEqual(scene.edges.map(\.originalIndex), [1])
        let viewport = StaticGraphViewport()
        let bounds = CGRect(x: 0, y: 0, width: 600, height: 400)
        let first = viewport.screen(scene.nodes[0].position, in: bounds)
        let second = viewport.screen(scene.nodes[1].position, in: bounds)
        let midpoint = CGPoint(x: (first.x + second.x) / 2, y: (first.y + second.y) / 2)
        XCTAssertEqual(StaticGraphGrid.edge(at: midpoint, scene: scene, viewport: viewport, bounds: bounds), 1)
    }

    func testShapePickingIncludesPaintedUnitCornersAndExcludesEmptyPolygonCorners() {
        let bounds = CGRect(x: 0, y: 0, width: 600, height: 400)
        let viewport = StaticGraphViewport()
        for (kind, delta, expected) in [(StaticGraphKind.unit, CGPoint(x: 16, y: 15), true),
                                       (.part, CGPoint(x: 12, y: 12), false),
                                       (.technology, CGPoint(x: 0, y: 19), false)] {
            let node = StaticGraphNode(key: "shape", label: "Shape", kind: kind, position: .zero,
                                       radius: 20, state: 0, closed: false, priority: 0)
            let scene = StaticGraphScene(nodes: [node], edges: [], layout: .curated, focusKey: nil, snapshot: nil)
            let hit = StaticGraphGrid(nodes: scene.nodes).node(
                at: CGPoint(x: bounds.midX + delta.x, y: bounds.midY + delta.y),
                scene: scene, viewport: viewport, bounds: bounds)
            XCTAssertEqual(hit, expected ? node.key : nil)
        }
    }

    private func memory(_ id: String, count: Int) -> MemoryGraphNode {
        MemoryGraphNode(id: id, title: id, memoryType: "note", timestamp: "", importance: 0.5,
                        entityCount: count, contentPreview: "Synthetic memory")
    }

    private func graphFixture() throws -> CuratedGraphSnapshot {
        let path = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/curated-graph.json")
        return try JSONDecoder().decode(CuratedGraphSnapshot.self, from: Data(contentsOf: path))
    }
}
