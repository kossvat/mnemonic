import AppKit
import Foundation
import MnemonicShared
import XCTest
@testable import MnemonicApp

@MainActor
private final class GraphIntegrationFixture {
    let root: URL
    let profile: MnemonicProfile
    let clock = ProjectTestClock()
    let live: ProjectTestSource
    let reads: ProjectPlanRead<CuratedGraphSnapshot>
    let details: ProjectPlanRead<ProjectDetail>
    let model: MnemonicAppModel
    let window = NSObject()

    init() throws {
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-graph-integration-\(UUID().uuidString)")
        let home = root.appendingPathComponent("profile")
        let user = root.appendingPathComponent("user")
        do {
            try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
            try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
            try "[ui]\nenabled = false\nport = 9\n".write(
                to: home.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
            profile = try MnemonicProfile.resolve(
                environment: [MnemonicProfile.homeVariable: home.path],
                arguments: ["fixture-client"], userHome: user)
            let data = try ProjectTestFixtures()
            let live = ProjectTestSource(data)
            let details = ProjectPlanRead(data.detail)
            self.details = details
            self.live = live
            let reads = ProjectPlanRead(try Self.snapshot())
            self.reads = reads
            let graph = AppGraphController(requests: GraphRequests(
                snapshot: { _ in await reads.read().mapError { failure in .local(failure) } },
                project: { _ in await details.read().mapError { failure in .local(failure) } },
                evidence: { _, _, _ in .failure(.notFound) },
                memory: { _ in .failure(.notFound) },
                archives: { _, _, _ in .failure(.notFound) },
                mutate: { _, _ in .failure(.notFound) }))
            model = MnemonicAppModel(profile: profile,
                controller: projectTestController(live, clock: clock), graphController: graph,
                readStatus: { throw AppLegacyReadFailure.unavailable })
            model.activateWindow = {}
            model.route = .graph
        } catch {
            try? FileManager.default.removeItem(at: root)
            throw error
        }
    }

    static func snapshot(marker: String = "first", empty: Bool = false,
                         missingUnit: Bool = false) throws -> CuratedGraphSnapshot {
        let null = NSNull()
        var nodes: [[Any]] = [], links: [[Int]] = []
        if !empty {
            for slug in ["fixture-00", "fixture-01"] {
                let index = nodes.count
                nodes.append([slug, marker, 0, index * 100, 0, [0, 0], 1, 0, 0, 0, false, slug])
                nodes.append([slug + ":unit", "Unit", 2, index * 100, 100,
                              [null, null], 1, 0, 0, 0, false, slug])
                if !missingUnit || index == 0 { links.append([index, index + 1, 0, 0]) }
            }
        }
        let object: [String: Any] = ["v": 1, "n": nodes, "e": links, "s": [], "o": [0, 0, 0, 0]]
        return try JSONDecoder().decode(CuratedGraphSnapshot.self,
                                       from: JSONSerialization.data(withJSONObject: object))
    }

    var marker: URL { profile.home.resolvingSymlinksInPath().appendingPathComponent("fixture-window.state") }
    func state() throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: Data(contentsOf: marker)) as? [String: Any])
    }
    func attachWriter() -> FixtureWindowState {
        let writer = FixtureWindowState(profile: profile)
        let model = model
        model.onWindowStateChange = { [weak model] in
            if let model { writer.update(model: model) }
        }
        return writer
    }
    func close() {
        model.window(ObjectIdentifier(window), visible: false)
        Task { await reads.finishAll(); await details.finishAll(); await live.finishLive() }
        try? FileManager.default.removeItem(at: root)
    }
}

final class AppGraphIntegrationTests: XCTestCase {
    @MainActor
    private func requested(_ fixture: GraphIntegrationFixture, _ count: Int) async {
        let notice = expectation(description: "Graph request arrived")
        await fixture.reads.requested(count) { notice.fulfill() }
        await fulfillment(of: [notice], timeout: 5)
    }

    @MainActor
    func testHiddenGraphMakesNoRequestAndClosingRevokesAnOpeningRead() async throws {
        let fixture = try GraphIntegrationFixture()
        defer { fixture.close() }
        await fixture.model.refreshAll()
        await fixture.model.projects.signalled()
        let initial = await fixture.reads.count
        XCTAssertEqual(initial, 0)
        XCTAssertTrue(fixture.clock.active.isEmpty)
        await fixture.reads.hold()
        fixture.model.window(ObjectIdentifier(fixture.window), visible: true)
        await requested(fixture, 1)
        XCTAssertTrue(fixture.model.curatedGraph.visible)
        fixture.model.window(ObjectIdentifier(fixture.window), visible: false)
        await fixture.reads.finishAll()
        await fixture.model.refreshAll()
        let final = await fixture.reads.count
        XCTAssertEqual(final, 1)
        XCTAssertNil(fixture.model.curatedGraph.snapshot)
        XCTAssertFalse(fixture.model.curatedGraph.loading)
        XCTAssertTrue(fixture.clock.active.isEmpty)
    }

    @MainActor
    func testGraphUsesLiveHealthAndExplicitRefreshWithoutPlanPolling() async throws {
        let fixture = try GraphIntegrationFixture()
        defer { fixture.close() }
        fixture.model.setupMessage = "Synthetic legacy read failure"
        await fixture.model.projects.setVisible(true)
        XCTAssertNotNil(fixture.model.curatedGraph.snapshot)
        XCTAssertTrue(fixture.model.isOnline)
        fixture.model.projects.live.take(.failed(.off))
        XCTAssertFalse(fixture.model.isOnline)
        await fixture.model.projects.coordinator.signalled()
        XCTAssertTrue(fixture.model.isOnline)
        XCTAssertFalse(fixture.clock.active.contains(30))
        XCTAssertFalse(AppRoute.graph.refreshes)
        await fixture.model.refreshAll()
        let count = await fixture.reads.count
        XCTAssertEqual(count, 2)
        fixture.model.route = .search
        XCTAssertFalse(fixture.model.curatedGraph.visible)
        XCTAssertNil(fixture.model.curatedGraph.snapshot)
        XCTAssertNil(fixture.model.graphDrawnSnapshot)
    }

    @MainActor
    func testSignalRevokesGraphBeforeHeldLiveResponseAndReadsOneReplacement() async throws {
        let fixture = try GraphIntegrationFixture()
        defer { fixture.close() }
        await fixture.model.projects.setVisible(true)
        await fixture.reads.hold()
        let reading = Task { await fixture.model.projects.refresh() }
        await requested(fixture, 2)
        await fixture.live.holdNext(.live)
        let signalling = Task { await fixture.model.projects.signalled() }
        let liveNotice = expectation(description: "Live request arrived")
        let noticeID = await fixture.live.notifyWhenRequested(.live, count: 2) { liveNotice.fulfill() }
        await fulfillment(of: [liveNotice], timeout: 5)
        await fixture.live.cancelNotice(noticeID)
        XCTAssertNil(fixture.model.curatedGraph.snapshot)
        await fixture.reads.set(.success(try GraphIntegrationFixture.snapshot(marker: "current")))
        await fixture.reads.finish(2)
        await requested(fixture, 3)
        XCTAssertNil(fixture.model.curatedGraph.snapshot)
        await fixture.reads.finishAll()
        await reading.value
        XCTAssertEqual(fixture.model.curatedGraph.snapshot?.nodes.first?.label, "current")
        await fixture.live.finishLive()
        await signalling.value
        let count = await fixture.reads.count, peak = await fixture.reads.peakActive
        XCTAssertEqual(count, 3)
        XCTAssertEqual(peak, 1)
    }

    @MainActor
    func testMarkerNeedsCurrentCompletedPaintAndFailureRevokesItImmediately() async throws {
        let fixture = try GraphIntegrationFixture()
        defer { fixture.close() }
        let writer = fixture.attachWriter()
        await fixture.model.projects.setVisible(true)
        writer.update(model: fixture.model)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
        let snapshot = try XCTUnwrap(fixture.model.curatedGraph.snapshot)
        fixture.model.graphDidDraw(snapshot)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, true)
        XCTAssertEqual(try fixture.state()["painted"] as? Bool, true)
        let attributes = try FileManager.default.attributesOfItem(atPath: fixture.marker.path)
        fixture.model.graphDidDraw(snapshot)
        writer.update(model: fixture.model)
        let same = try FileManager.default.attributesOfItem(atPath: fixture.marker.path)
        XCTAssertEqual(attributes[.modificationDate] as? Date, same[.modificationDate] as? Date)
        await fixture.reads.set(.failure(.unreachable))
        await fixture.model.curatedGraph.refresh()
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
        fixture.model.graphDidDraw(snapshot)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
    }

    @MainActor
    func testIdenticalResponseAfterFailureOrHideNeedsANewCompletedPaint() async throws {
        let fixture = try GraphIntegrationFixture()
        defer { fixture.close() }
        let writer = fixture.attachWriter()
        await fixture.model.projects.setVisible(true)
        let snapshot = try XCTUnwrap(fixture.model.curatedGraph.snapshot)
        fixture.model.graphDidDraw(snapshot)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, true)
        await fixture.reads.set(.failure(.unreachable))
        await fixture.model.curatedGraph.refresh()
        XCTAssertNil(fixture.model.graphDrawnSnapshot)
        await fixture.reads.set(.success(snapshot))
        await fixture.model.curatedGraph.refresh()
        writer.update(model: fixture.model)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
        fixture.model.graphDidDraw(snapshot)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, true)
        fixture.model.window(ObjectIdentifier(fixture.window), visible: false)
        XCTAssertNil(fixture.model.graphDrawnSnapshot)
        await fixture.model.projects.setVisible(true)
        writer.update(model: fixture.model)
        XCTAssertEqual(fixture.model.curatedGraph.snapshot, snapshot)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
        fixture.model.graphDidDraw(snapshot)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, true)
    }

    @MainActor
    func testCoalescedIdenticalRefreshRepaintsTheSurvivingCanvasBeforeReadiness() async throws {
        let fixture = try GraphIntegrationFixture()
        defer { fixture.close() }
        let writer = fixture.attachWriter()
        await fixture.model.projects.setVisible(true)
        let graph = fixture.model.curatedGraph
        let snapshot = try XCTUnwrap(graph.snapshot)
        let scene = StaticGraphScene.curated(snapshot)
        let view = StaticGraphNativeView()
        view.setFrameSize(CGSize(width: 800, height: 600))
        let window = staticGraphTestWindow(containing: view)
        defer { window.close() }
        XCTAssertFalse(window.isVisible)
        let revision = graph.paintRevision
        let report = { fixture.model.graphDidDraw(snapshot) }
        view.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in },
                    onDraw: report, paintRevision: revision)
        staticGraphTestDisplay(view)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, true)
        XCTAssertGreaterThan(view.completedPaints, 0)
        let viewport = view.viewport

        graph.invalidateSelected()
        await graph.refresh()
        writer.update(model: fixture.model)
        XCTAssertEqual(graph.snapshot, snapshot)
        XCTAssertGreaterThan(graph.paintRevision, revision)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
        let previousPaints = view.completedPaints
        // SwiftUI may coalesce the intermediate nil; update the same native
        // surface directly with the identical final scene and its new revision.
        view.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in },
                    onDraw: report, paintRevision: graph.paintRevision)
        XCTAssertEqual(view.viewport, viewport)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
        staticGraphTestDisplay(view)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, true)
        XCTAssertEqual(view.completedPaints, previousPaints + 1)
        view.update(scene: scene, selected: nil, onNode: { _ in }, onEdge: { _ in },
                    onDraw: report, paintRevision: graph.paintRevision)
        staticGraphTestDisplay(view)
        XCTAssertEqual(view.completedPaints, previousPaints + 1)
        XCTAssertFalse(window.isVisible)
    }

    @MainActor
    func testSignalRevokesHeldSelectedProjectBeforeLiveHealthCompletes() async throws {
        let fixture = try GraphIntegrationFixture()
        defer { fixture.close() }
        await fixture.model.projects.setVisible(true)
        await fixture.details.hold()
        let selecting = Task { await fixture.model.curatedGraph.selectNode("fixture-00") }
        let detailNotice = expectation(description: "Selected project request arrived")
        await fixture.details.requested(1) { detailNotice.fulfill() }
        await fulfillment(of: [detailNotice], timeout: 5)
        await fixture.live.holdNext(.live)
        let signalling = Task { await fixture.model.projects.signalled() }
        let liveNotice = expectation(description: "Live request arrived")
        let noticeID = await fixture.live.notifyWhenRequested(.live, count: 2) { liveNotice.fulfill() }
        await fulfillment(of: [liveNotice], timeout: 5)
        await fixture.live.cancelNotice(noticeID)
        await fixture.details.finishAll()
        await selecting.value
        XCTAssertNil(fixture.model.curatedGraph.project)
        XCTAssertNil(fixture.model.curatedGraph.selectedKey)
        await fixture.live.finishLive()
        await signalling.value
        let count = await fixture.reads.count
        XCTAssertEqual(count, 2)
    }

    @MainActor
    func testWrongRouteHideEndpointAndStalePaintNeverProveReadiness() async throws {
        let fixture = try GraphIntegrationFixture()
        defer { fixture.close() }
        let writer = fixture.attachWriter()
        await fixture.model.projects.setVisible(true)
        let first = try XCTUnwrap(fixture.model.curatedGraph.snapshot)
        fixture.model.graphDidDraw(first)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, true)
        await fixture.reads.set(.success(try GraphIntegrationFixture.snapshot(marker: "changed")))
        await fixture.model.refreshAll()
        fixture.model.graphDidDraw(first)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
        let current = try XCTUnwrap(fixture.model.curatedGraph.snapshot)
        fixture.model.graphDidDraw(current)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, true)
        fixture.model.window(ObjectIdentifier(fixture.window), visible: false)
        fixture.model.graphDidDraw(current)
        XCTAssertEqual(try fixture.state()["visible"] as? Bool, false)
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
        await fixture.model.applyEndpoint(fixture.model.endpoint)
        XCTAssertNil(fixture.model.graphDrawnSnapshot)
        fixture.model.route = .search
        fixture.model.graphDidDraw(current)
        writer.update(model: fixture.model)
        XCTAssertEqual(try fixture.state()["route"] as? String, "search")
        XCTAssertEqual(try fixture.state()["ready"] as? Bool, false)
    }

    @MainActor
    func testEmptyGraphAndMissingProjectUnitCannotSatisfyFixture() throws {
        XCTAssertTrue(FixtureWindowState.graphContainsFixture(try GraphIntegrationFixture.snapshot()))
        XCTAssertFalse(FixtureWindowState.graphContainsFixture(nil))
        XCTAssertFalse(FixtureWindowState.graphContainsFixture(try GraphIntegrationFixture.snapshot(empty: true)))
        XCTAssertFalse(FixtureWindowState.graphContainsFixture(try GraphIntegrationFixture.snapshot(missingUnit: true)))
    }
}
