import AppKit
import MnemonicShared
import SwiftUI
import XCTest
@testable import MnemonicApp

final class BoardPaintProbeTests: XCTestCase {
    private struct Report: Equatable {
        let paint: BoardPaint
        let card: String
        let revision: UInt64
    }

    private func paints() throws -> (BoardPaint, BoardPaint) {
        let live = try FixtureWire.live(count: 2)
        let board = try FixtureWire.board([FixtureWire.row(0), FixtureWire.row(1)])
        let other = try FixtureWire.live(count: 1)
        return (BoardPaint(board: board, live: live), BoardPaint(board: board, live: other))
    }

    /// Reports wait for the commit of the pass that drew them, at the end of a turn of the run loop.
    @MainActor
    private func settle() {
        RunLoop.main.run(until: Date(timeIntervalSinceNow: 0.05))
    }

    @MainActor
    private func draw(_ view: NSView) throws {
        view.frame = NSRect(x: 0, y: 0, width: 4, height: 4)
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        settle()
    }

    @MainActor
    func testTheProbeReportsItsCardAfterThePassAndAsksForAPaintOnlyWhenSomethingChanged() throws {
        let (first, second) = try paints()
        var reports: [Report] = []
        let view = BoardPaintProbeView()
        let record: (BoardPaint, String, UInt64) -> Void = { paint, card, revision in
            reports.append(Report(paint: paint, card: card, revision: revision))
        }
        view.update(paint: first, card: "card-a", revision: 0, visible: true, onDraw: record)
        XCTAssertEqual(view.paintRequests, 1)
        view.frame = NSRect(x: 0, y: 0, width: 4, height: 4)
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        view.cacheDisplay(in: view.bounds, to: bitmap)
        XCTAssertEqual(reports, [], "nothing is reported inside the pass")
        settle()
        XCTAssertEqual(reports, [Report(paint: first, card: "card-a", revision: 0)])

        view.update(paint: first, card: "card-a", revision: 0, visible: true, onDraw: record)
        XCTAssertEqual(view.paintRequests, 1, "an equal board, snapshot and revision ask for no paint")
        view.update(paint: second, card: "card-a", revision: 0, visible: true, onDraw: record)
        XCTAssertEqual(view.paintRequests, 2, "a new snapshot asks for its own paint")
        view.update(paint: second, card: "card-a", revision: 1, visible: true, onDraw: record)
        XCTAssertEqual(view.paintRequests, 3, "a proof taken back asks for a paint of the same snapshot")
        try draw(view)
        XCTAssertEqual(reports.last, Report(paint: second, card: "card-a", revision: 1))
    }

    @MainActor
    func testAHiddenWindowReportsNothingAndShowingItAgainAsksForAPaint() throws {
        let (first, _) = try paints()
        var reports: [Report] = []
        let view = BoardPaintProbeView()
        let record: (BoardPaint, String, UInt64) -> Void = { paint, card, revision in
            reports.append(Report(paint: paint, card: card, revision: revision))
        }
        view.update(paint: first, card: "card-a", revision: 0, visible: false, onDraw: record)
        try draw(view)
        XCTAssertEqual(reports, [])

        let requests = view.paintRequests
        view.update(paint: first, card: "card-a", revision: 0, visible: true, onDraw: record)
        XCTAssertEqual(view.paintRequests, requests + 1, "the same snapshot is painted again after a reopen")
        try draw(view)
        XCTAssertEqual(reports, [Report(paint: first, card: "card-a", revision: 0)])
    }

    /// Core Animation draws inside an earlier observer of the pass; the report must
    /// still come without waiting for input or a timer, once, after that callback.
    @MainActor
    func testAPaintInsideAnEarlierObserverIsReportedOnceWithoutWaitingForInput() throws {
        let (first, _) = try paints()
        let view = BoardPaintProbeView()
        view.frame = NSRect(x: 0, y: 0, width: 4, height: 4)
        let bitmap = try XCTUnwrap(view.bitmapImageRepForCachingDisplay(in: view.bounds))
        var reports: [Date] = []
        var drawnAt: Date?
        view.update(paint: first, card: "card-a", revision: 0, visible: true) { _, _, _ in
            reports.append(Date())
            CFRunLoopStop(CFRunLoopGetMain())
        }
        let earlier = CFRunLoopObserverCreateWithHandler(
            kCFAllocatorDefault, CFRunLoopActivity.beforeWaiting.rawValue, false, 0
        ) { _, _ in
            MainActor.assumeIsolated {
                view.cacheDisplay(in: view.bounds, to: bitmap)
                drawnAt = Date()
                XCTAssertEqual(reports, [], "nothing is reported inside the earlier callback")
            }
        }
        CFRunLoopAddObserver(CFRunLoopGetMain(), earlier, .defaultMode)
        CFRunLoopRunInMode(.defaultMode, 5, false)
        let drawn = try XCTUnwrap(drawnAt)
        XCTAssertEqual(reports.count, 1)
        XCTAssertLessThan(try XCTUnwrap(reports.first).timeIntervalSince(drawn), 1, "the loop did not sleep until other input")
        settle()
        XCTAssertEqual(reports.count, 1, "the observer fired once and is gone")
    }

    /// The fixture keys of the budget run, on a board and a snapshot with two cards.
    private func fixtureValues() throws -> (ProjectBoard, [String: Any]) {
        let keys = ["fixture-00", "fixture-01"]
        let board = try FixtureWire.board([FixtureWire.row(0), FixtureWire.row(1)])
        var boardObject = try XCTUnwrap(JSONSerialization.jsonObject(with: JSONEncoder().encode(board)) as? [String: Any])
        let boardRows = try XCTUnwrap(boardObject["projects"] as? [[String: Any]])
        boardObject["projects"] = boardRows.enumerated().map { index, row -> [String: Any] in
            var value = row
            value["key"] = keys[index]
            return value
        }
        let plan = try XCTUnwrap(boardRows.first?["plan"])
        var live = try XCTUnwrap(JSONSerialization.jsonObject(
            with: JSONEncoder().encode(FixtureWire.live(count: 2))) as? [String: Any])
        let liveRows = try XCTUnwrap(live["projects"] as? [[String: Any]])
        live["projects"] = liveRows.enumerated().map { index, row -> [String: Any] in
            var value = row
            value["key"] = keys[index]
            value["plan"] = plan
            return value
        }
        let fixtureBoard = try JSONDecoder().decode(ProjectBoard.self, from: JSONSerialization.data(withJSONObject: boardObject))
        return (fixtureBoard, live)
    }

    /// The real board in a window, its probes, the model and the marker, through
    /// the window's own display: decoded is not ready, painted is; a hide, a change
    /// of route and a new snapshot each take it back until the next paint.
    @MainActor
    func testTheMarkerFollowsTheBoardThroughTheDisplayOfAWindow() throws {
        _ = NSApplication.shared
        let scratch = FileManager.default.temporaryDirectory.appendingPathComponent("mn-board-paint-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: scratch) }
        let home = scratch.appendingPathComponent("profile"), user = scratch.appendingPathComponent("user")
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
        try "[ui]\nenabled = false\nport = 9\n".write(to: home.appendingPathComponent("config.toml"),
                                                      atomically: true, encoding: .utf8)
        let profile = try MnemonicProfile.resolve(environment: [MnemonicProfile.homeVariable: home.path],
                                                  arguments: ["fixture-client"], userHome: user)
        let model = MnemonicAppModel(profile: profile, controller: AppProjectController(
            clock: ProjectTestClock(), ask: { .failed(.off) }, board: { .failure(.off) }, detail: { _ in .failure(.off) }))
        model.activateWindow = {}
        defer { model.projects.hide() }
        let writer = FixtureWindowState(profile: profile)
        let marker = profile.home.resolvingSymlinksInPath().appendingPathComponent("fixture-window.state")
        func ready() throws -> Bool {
            writer.update(model: model)
            let value = try JSONSerialization.jsonObject(with: Data(contentsOf: marker)) as? [String: Any]
            return value?["ready"] as? Bool ?? false
        }
        let (board, fixtureLive) = try fixtureValues()
        var liveObject = fixtureLive
        let live = try JSONDecoder().decode(LiveSnapshot.self, from: JSONSerialization.data(withJSONObject: liveObject))

        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 1_200, height: 900),
                              styleMask: [.borderless], backing: .buffered, defer: false)
        window.isReleasedWhenClosed = false
        defer { window.close() }
        let host = NSHostingView(rootView: AnyView(EmptyView()))
        window.contentView = host
        func show(_ visible: Bool) {
            host.rootView = AnyView(ProjectsView(
                board: model.projects.board, live: model.projects.live.shown, failure: nil, loading: false,
                now: FixtureWire.now, onOpen: { _ in }, paintRevision: model.boardPaintRevision,
                onDraw: { paint, card, revision in model.boardDidDraw(paint, card: card, revision: revision) })
                .environment(\.mnemonicWindowVisible, visible))
            host.layoutSubtreeIfNeeded()
            window.display()
            settle()
        }

        model.projects.present(board: board, detail: nil, live: .snapshot(live), route: .projects)
        model.projects.onPresentationChange?()
        XCTAssertFalse(try ready(), "decoded and not yet painted")
        show(true)
        XCTAssertEqual(model.boardPaintedCards, ["fixture-00", "fixture-01"])
        XCTAssertTrue(try ready())

        model.projects.hide()
        show(false)
        XCTAssertFalse(try ready())
        model.projects.present(board: board, detail: nil, live: .snapshot(live), route: .projects)
        model.projects.onPresentationChange?()
        XCTAssertFalse(try ready(), "the paint before the hide does not count")
        show(true)
        XCTAssertTrue(try ready())

        model.projects.hide()
        model.route = .timeline(TimelineDay.today(now: FixtureWire.now))
        model.route = .projects
        model.projects.present(board: board, detail: nil, live: .snapshot(live), route: .projects)
        model.projects.onPresentationChange?()
        XCTAssertFalse(try ready(), "a change of route takes the paint back")
        show(true)
        XCTAssertTrue(try ready())

        liveObject["v"] = ((liveObject["v"] as? Int) ?? 0) + 1
        let next = try JSONDecoder().decode(LiveSnapshot.self, from: JSONSerialization.data(withJSONObject: liveObject))
        model.projects.present(board: board, detail: nil, live: .snapshot(next), route: .projects)
        model.projects.onPresentationChange?()
        XCTAssertFalse(try ready(), "a new snapshot waits for its own paint")
        show(true)
        XCTAssertTrue(try ready())
    }
}
