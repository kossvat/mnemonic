import AppKit
import Foundation
import MnemonicShared
import SwiftUI
import XCTest
@testable import MnemonicApp

final class GraphPresentationTests: XCTestCase {
    private var directory: URL {
        URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/Graph")
    }

    func testUnknownHoursStayDifferentFromZeroAndFormattingIsStable() {
        XCTAssertEqual(GraphPresentation.hours(nil), "Unavailable")
        XCTAssertEqual(GraphPresentation.hours(0), "0.0 h")
        XCTAssertEqual(GraphPresentation.hours(42), "4.2 h")
        XCTAssertEqual(GraphPresentation.activity(nil), "Activity unavailable")
        XCTAssertEqual(GraphPresentation.activity(0), "Activity today")
        XCTAssertEqual(GraphPresentation.activity(1), "1 day since activity")
        XCTAssertEqual(GraphPresentation.activity(63), "63 days since activity")
    }

    func testFixtureStatesKeepOmissionsShelvesAndSupportingContent() throws {
        let examples = try GraphFixturePreview.load(from: directory)
        XCTAssertEqual(examples.map(\.name), ["full", "empty", "error", "stale-only", "expanded"])
        let full = examples[0].state
        let snapshot = try XCTUnwrap(full.snapshot)
        XCTAssertEqual(full.selectedProjectSlug, "harbor")
        XCTAssertEqual(full.staleProjects.map(\.label), ["Ledger", "Old workshop"])
        XCTAssertEqual(full.outsideShelf.map(\.label), ["Sketch board"])
        XCTAssertEqual(GraphPresentation.omissions(snapshot.omitted),
                       "Outside this view: 12 nodes · 4 links · 1 stale project · 7 outside entities")
        XCTAssertNil(GraphPresentation.omissions(try XCTUnwrap(examples[1].state.snapshot).omitted))
        XCTAssertTrue(try XCTUnwrap(examples[3].state.snapshot).nodes.isEmpty)
        XCTAssertEqual(examples[3].state.staleProjects.count, 2)
        let expanded = examples[4].state
        XCTAssertEqual(expanded.selectedEdge?.kind, .uses)
        XCTAssertTrue(expanded.evidence.allSatisfy { row in !(row.memory?.content.isEmpty ?? true) })
        XCTAssertEqual(expanded.evidenceNextOffset, 2)
        XCTAssertEqual(expanded.outside.nextOffset, 2)
        XCTAssertTrue(expanded.canUndo)
        XCTAssertTrue(expanded.canRetryMutation)
        XCTAssertEqual(expanded.recovery?.confirmed, false)
    }

    func testShelfSelectionRetainsProjectRouteAndEntityKeysStayOpaque() throws {
        var state = try XCTUnwrap(GraphFixturePreview.load(from: directory).first?.state)
        state.selectedKey = "project:4"
        XCTAssertNil(state.selectedNode)
        XCTAssertEqual(state.selectedShelf?.label, "Ledger")
        XCTAssertEqual(state.selectedProjectSlug, "ledger")
        state.selectedKey = "entity:sketch"
        XCTAssertNil(state.selectedProjectSlug)
        XCTAssertEqual(GraphPresentation.entityID("entity:sketch"), "sketch")
        XCTAssertEqual(GraphPresentation.entityID("entity:opaque:part"), "opaque:part")
        XCTAssertNil(GraphPresentation.entityID("project:4"))
        XCTAssertNil(GraphPresentation.entityID("entity:"))
    }

    func testUnavailableOrLoadingShelvesNeverClaimEmptyCounts() throws {
        let examples = try GraphFixturePreview.load(from: directory)
        let empty = examples[1].state
        XCTAssertNil(empty.shelfNotice)
        XCTAssertEqual(empty.staleProjectCount, 0)
        XCTAssertEqual(empty.outsideEntityCount, 0)
        for state in [GraphPageState(), examples[2].state] {
            XCTAssertNotNil(state.shelfNotice)
            XCTAssertNil(state.staleProjectCount)
            XCTAssertNil(state.outsideEntityCount)
        }
        var state = examples[0].state
        XCTAssertEqual(state.staleProjectCount, 2)
        XCTAssertEqual(state.outsideEntityCount, 1)
        state.loading = true
        XCTAssertEqual(state.shelfNotice, "Loading this list with the graph.")
        XCTAssertNil(state.staleProjectCount)
        XCTAssertNil(state.outsideEntityCount)
        state.loading = false
        state.failure = .unavailable
        XCTAssertNotNil(state.shelfNotice)
        XCTAssertNil(state.staleProjectCount)
        XCTAssertNil(state.outsideEntityCount)
        state.outside = examples[4].state.outside
        XCTAssertEqual(state.outsideEntityCount, 4)
        state.outside.entities = []
        state.outside.total = 0
        XCTAssertEqual(state.outsideEntityCount, 0)
        XCTAssertNil(state.staleProjectCount)
    }

    func testKindsAndRecencyHaveDistinctReadableLegendLabels() {
        let kinds: [CuratedGraphNodeKind] = [.project, .part, .unit, .technology, .person]
        XCTAssertEqual(kinds.map(GraphPresentation.kind), ["Project", "Part", "Plan unit", "Technology", "Person"])
        XCTAssertEqual(Set(kinds.map(GraphPresentation.symbol)).count, kinds.count)
        let states: [CuratedGraphNodeState] = [.active, .quiet, .stale, .archived]
        XCTAssertEqual(states.map(GraphPresentation.state), ["Active", "Quiet", "Stale", "Archived"])
    }

    @MainActor
    func testPurePagePaintsOnlyAvailableNonemptySnapshotsWithoutInvokingActions() throws {
        _ = NSApplication.shared
        for example in try GraphFixturePreview.load(from: directory) {
            var actionsInvoked = 0
            var paints: [CuratedGraphSnapshot] = []
            let actions = GraphPageActions(
                refresh: { actionsInvoked += 1 }, selectNode: { _ in actionsInvoked += 1 },
                expand: { _ in actionsInvoked += 1 }, selectEdge: { _ in actionsInvoked += 1 },
                moreEvidence: { actionsInvoked += 1 }, outside: { _ in actionsInvoked += 1 },
                history: { _ in actionsInvoked += 1 }, archive: { _, _ in actionsInvoked += 1 },
                retry: { actionsInvoked += 1 }, undo: { actionsInvoked += 1 },
                openProject: { _ in actionsInvoked += 1 })
            let page = GraphPage(state: example.state, actions: actions, onDraw: { snapshot in paints.append(snapshot) })
            let host = NSHostingView(rootView: page)
            host.frame = NSRect(x: 0, y: 0, width: 960, height: example.height)
            host.layoutSubtreeIfNeeded()
            let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
            host.cacheDisplay(in: host.bounds, to: bitmap)
            XCTAssertEqual(actionsInvoked, 0, example.name)
            if let snapshot = example.state.snapshot, !snapshot.nodes.isEmpty {
                XCTAssertFalse(paints.isEmpty, example.name)
                XCTAssertTrue(paints.allSatisfy { painted in painted == snapshot }, example.name)
            } else {
                XCTAssertTrue(paints.isEmpty, example.name)
            }
        }
    }

    @MainActor
    func testUnavailableSnapshotNeverProducesAVisiblePaintProof() throws {
        _ = NSApplication.shared
        var state = try XCTUnwrap(GraphFixturePreview.load(from: directory).first?.state)
        state.failure = .unavailable
        var painted = false
        let host = NSHostingView(rootView: GraphPage(state: state, onDraw: { _ in painted = true }))
        host.frame = NSRect(x: 0, y: 0, width: 960, height: 900)
        host.layoutSubtreeIfNeeded()
        let bitmap = try XCTUnwrap(host.bitmapImageRepForCachingDisplay(in: host.bounds))
        host.cacheDisplay(in: host.bounds, to: bitmap)
        XCTAssertFalse(painted)
    }
}
