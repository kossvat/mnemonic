import AppKit
import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

@MainActor
final class StaticGraphPaintingTests: XCTestCase {
    func testOffscreenEdgesCannotPaintOutsideCanvasBounds() throws {
        let context = try StaticGraphBitmap.context(width: 100, height: 80)
        context.setFillColor(CGColor(red: 1, green: 0, blue: 1, alpha: 1))
        context.fill(CGRect(x: 0, y: 0, width: 100, height: 80))
        let before = try pixels(context)
        let nodes = [-100, 100].enumerated().map { entry in
            StaticGraphNode(key: "outside-\(entry.offset)", label: "Outside", kind: .project,
                            position: CGPoint(x: entry.element, y: 0), radius: 14,
                            state: 0, closed: false, priority: 0)
        }
        let edge = StaticGraphEdge(source: 0, target: 1, kind: 0, weight: 1, originalIndex: 0)
        let scene = StaticGraphScene(nodes: nodes, edges: [edge], layout: .curated, focusKey: nil, snapshot: nil)
        let painter = StaticGraphPainter()
        painter.prepare(scene)
        let bounds = CGRect(x: 20, y: 20, width: 60, height: 40)
        let originalClip = context.boundingBoxOfClipPath
        let stats = painter.paint(scene: scene, viewport: StaticGraphViewport(), bounds: bounds,
                                  selected: nil, hovered: nil, context: context)
        let after = try pixels(context)
        var outsideChanges = 0
        var insideChanges = 0
        for y in 0..<80 {
            for x in 0..<100 {
                let offset = (y * 100 + x) * 4
                guard before[offset..<(offset + 4)] != after[offset..<(offset + 4)] else { continue }
                if bounds.contains(CGPoint(x: CGFloat(x) + 0.5, y: CGFloat(y) + 0.5)) {
                    insideChanges += 1
                } else {
                    outsideChanges += 1
                }
            }
        }
        XCTAssertEqual(stats.edges, 1, "The crossing edge must reach the actual painter")
        XCTAssertEqual(stats.nodes, 0, "Both endpoints lie beyond the canvas")
        XCTAssertGreaterThan(insideChanges, 0)
        XCTAssertEqual(outsideChanges, 0, "Clipping must protect neighboring hosted views")
        XCTAssertEqual(context.boundingBoxOfClipPath, originalClip, "Restore the caller's clipping state")
    }

    func testBothAdaptersProduceActualPixelsAndLabelDrawingChangesTheBitmap() throws {
        let path = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/curated-graph.json")
        let snapshot = try JSONDecoder().decode(CuratedGraphSnapshot.self, from: Data(contentsOf: path))
        let nodes = (0..<6).map { index in
            MemoryGraphNode(id: "memory-\(index)", title: "Memory \(index)", memoryType: "decision",
                            timestamp: "", importance: 0.5, entityCount: index + 1,
                            contentPreview: "Synthetic memory")
        }
        let memory = StaticGraphScene.memoryMap(nodes: nodes,
            edges: [MemoryGraphEdge(source: "memory-0", target: "memory-1", weight: 2, shared: [])])
        for scene in [StaticGraphScene.curated(snapshot), memory] {
            let painter = StaticGraphPainter()
            painter.prepare(scene)
            let context = try StaticGraphBitmap.context(width: 1000, height: 700)
            var viewport = StaticGraphViewport()
            viewport.fit(scene, in: CGRect(x: 0, y: 0, width: 1000, height: 700))
            let noLabels = StaticGraphBitmap.paint(scene, viewport: viewport, painter: painter,
                                                   context: context, labelLimit: 0)
            let before = try pixels(context)
            let withLabels = StaticGraphBitmap.paint(scene, viewport: viewport, painter: painter, context: context)
            let after = try pixels(context)
            XCTAssertGreaterThan(withLabels.nodes, 0)
            XCTAssertGreaterThan(withLabels.edges, 0)
            XCTAssertEqual(noLabels.labels, 0)
            XCTAssertGreaterThan(withLabels.labels, 0)
            XCTAssertNotEqual(before, after, "CoreText lines must alter the actual bitmap")
            let empty = StaticGraphPainter()
            empty.prepare(.empty)
            StaticGraphBitmap.paint(.empty, viewport: viewport, painter: empty, context: context)
            XCTAssertNotEqual(after, try pixels(context), "Drawing counts cannot replace painted pixels")
        }
    }

    func testNativeCallbackRequiresCompletedNonemptyPaintAndEqualUpdateStaysIdle() throws {
        let view = StaticGraphNativeView()
        view.setFrameSize(CGSize(width: 1000, height: 700))
        let window = staticGraphTestWindow(containing: view)
        defer { window.close() }
        XCTAssertFalse(window.isVisible)
        var callbacks = 0
        let scene = StaticGraphBenchmark.scene()
        view.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in }, onDraw: { callbacks += 1 })
        XCTAssertEqual(callbacks, 0)
        XCTAssertEqual(view.completedPaints, 0)
        staticGraphTestDisplay(view)
        XCTAssertGreaterThan(callbacks, 0)
        XCTAssertEqual(view.completedPaints, callbacks)
        let initialPaints = view.completedPaints
        let initialCallbacks = callbacks
        let originalViewport = view.viewport
        view.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in }, onDraw: { callbacks += 1 })
        staticGraphTestDisplay(view)
        XCTAssertEqual(view.completedPaints, initialPaints, "Identical data must not paint another frame")
        XCTAssertEqual(view.viewport, originalViewport)
        XCTAssertEqual(callbacks, initialCallbacks)
        view.update(scene: .empty, selected: nil, onNode: { _ in }, onEdge: { _ in }, onDraw: { callbacks += 1 })
        staticGraphTestDisplay(view)
        XCTAssertEqual(callbacks, initialCallbacks, "An empty paint is not graph readiness")
        let replacement = StaticGraphNativeView()
        replacement.setFrameSize(CGSize(width: 1000, height: 700))
        let replacementWindow = staticGraphTestWindow(containing: replacement)
        defer { replacementWindow.close() }
        XCTAssertFalse(replacementWindow.isVisible)
        replacement.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in }, onDraw: { callbacks += 1 })
        XCTAssertEqual(callbacks, initialCallbacks, "Recreating a surface is not itself a paint")
        staticGraphTestDisplay(replacement)
        XCTAssertEqual(callbacks, initialCallbacks + 1, "A recreated graph must report its first actual paint")
    }

    func testLabelsAreCappedAndMeasuredFramesDrawTheFullRequiredWorkload() throws {
        let scene = StaticGraphBenchmark.scene()
        let painter = StaticGraphPainter()
        painter.prepare(scene)
        let context = try StaticGraphBitmap.context(width: 1400, height: 900)
        let viewport = StaticGraphViewport(center: CGPoint(x: 602, y: 369), scale: 1)
        let stats = StaticGraphBitmap.paint(scene, viewport: viewport, painter: painter,
                                           context: context, labelLimit: 1000)
        XCTAssertEqual(stats, StaticGraphPaintStats(nodes: 150, edges: 375, labels: 100))
    }

    func testRevisionOnlyRefreshRequestsOnePaintWithoutMovingViewport() throws {
        let state = StaticGraphViewportState()
        let view = StaticGraphNativeView(viewportState: state)
        view.setFrameSize(CGSize(width: 1000, height: 700))
        let window = staticGraphTestWindow(containing: view)
        defer { window.close() }
        XCTAssertFalse(window.isVisible)
        let scene = StaticGraphBenchmark.scene()
        var callbacks = 0
        view.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in },
                    onDraw: { callbacks += 1 }, paintRevision: 7)
        staticGraphTestDisplay(view)
        XCTAssertGreaterThan(callbacks, 0)
        let initialPaints = view.completedPaints
        let initialCallbacks = callbacks
        state.viewport.pan(by: CGSize(width: 60, height: -20))
        let chosen = view.viewport

        // A fast refresh can publish only its final, byte-identical snapshot to this view.
        view.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in },
                    onDraw: { callbacks += 1 }, paintRevision: 8)
        XCTAssertEqual(view.viewport, chosen)
        XCTAssertEqual(callbacks, initialCallbacks, "A repaint request is not completed paint evidence")
        staticGraphTestDisplay(view)
        XCTAssertEqual(callbacks, initialCallbacks + 1)
        XCTAssertEqual(view.completedPaints, initialPaints + 1)
        view.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in },
                    onDraw: { callbacks += 1 }, paintRevision: 8)
        staticGraphTestDisplay(view)
        XCTAssertEqual(view.completedPaints, initialPaints + 1, "One revision must not create recurring frames")
        XCTAssertEqual(view.viewport, chosen)
        XCTAssertEqual(callbacks, initialCallbacks + 1)
    }

    func testSessionSummaryUsesCanonicalMemoryColorInsteadOfNoteFallback() throws {
        let expected = try XCTUnwrap(NSColor(Theme.Palette.session).usingColorSpace(.deviceRGB))
        let note = try XCTUnwrap(NSColor(Theme.Palette.note).usingColorSpace(.deviceRGB))
        for type in ["session_summary", "SESSION_SUMMARY"] {
            let actual = try XCTUnwrap(StaticGraphPainter.color(.memory(type)).usingColorSpace(.deviceRGB))
            XCTAssertEqual(actual.cgColor, expected.cgColor)
            XCTAssertNotEqual(actual.cgColor, note.cgColor)
        }
        let unknown = try XCTUnwrap(StaticGraphPainter.color(.memory("unknown")).usingColorSpace(.deviceRGB))
        XCTAssertEqual(unknown.cgColor, note.cgColor)
    }

    func testRefreshRemountRetainsViewportButExpandedFocusRefits() {
        let state = StaticGraphViewportState()
        let scene = StaticGraphBenchmark.scene()
        let first = StaticGraphNativeView(viewportState: state)
        first.setFrameSize(CGSize(width: 1000, height: 700))
        first.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in })
        state.viewport.pan(by: CGSize(width: 180, height: -65))
        state.viewport.zoom(by: 1.7, anchor: CGPoint(x: 150, y: 200), in: first.bounds)
        let chosen = state.viewport
        first.update(scene: .empty, selected: nil, onNode: { _ in }, onEdge: { _ in })
        let replacement = StaticGraphNativeView(viewportState: state)
        replacement.setFrameSize(first.bounds.size)
        replacement.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in })
        XCTAssertEqual(replacement.viewport, chosen, "Same-scope refresh must survive a loading remount")
        let focused = StaticGraphScene(nodes: scene.nodes, edges: scene.edges, layout: .curated,
                                       focusKey: scene.nodes[0].key, snapshot: nil)
        replacement.update(scene: focused, selected: nil, onNode: { _ in }, onEdge: { _ in })
        XCTAssertEqual(replacement.viewport.center, scene.nodes[0].position)
        XCTAssertNotEqual(replacement.viewport, chosen)
    }

    func testWarmRealRendererBudget() throws {
        let result = try StaticGraphBenchmark.run()
        XCTAssertEqual(result.minimumPainted, StaticGraphPaintStats(nodes: 150, edges: 375, labels: 100))
        XCTAssertEqual(result.frameCount, 120)
        print(String(format: "Static graph renderer: warm p95 %.3f ms, max %.3f ms, frames %d, nodes %d, edges %d, labels %d",
                     result.p95Milliseconds, result.maximumMilliseconds, result.frameCount,
                     result.minimumPainted.nodes, result.minimumPainted.edges, result.minimumPainted.labels))
        XCTAssertLessThanOrEqual(result.p95Milliseconds, 8)
        XCTAssertLessThanOrEqual(result.maximumMilliseconds, 8)
    }

    private func pixels(_ context: CGContext) throws -> Data {
        let image = try XCTUnwrap(context.makeImage())
        let provider = try XCTUnwrap(image.dataProvider)
        return try XCTUnwrap(provider.data) as Data
    }

}
