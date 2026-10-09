import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicApp

struct ProjectTestFixtures: Sendable {
    let board: ProjectBoard
    let detail: ProjectDetail
    let live: LiveSnapshot

    init() throws {
        func read<T: Decodable>(_ type: T.Type, _ name: String) throws -> T {
            let path = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
                .deletingLastPathComponent().appendingPathComponent("Fixtures/\(name).json")
            return try JSONDecoder().decode(type, from: Data(contentsOf: path))
        }
        board = try read(ProjectBoard.self, "project-board")
        detail = try read(ProjectDetail.self, "project-detail")
        live = try read(LiveSnapshot.self, "live")
    }
}

enum ProjectTestRequest: Hashable, Sendable { case live, board, detail }

struct ProjectTestCounts: Equatable, Sendable {
    var live = 0
    var board = 0
    var details: [String] = []

    func number(_ kind: ProjectTestRequest) -> Int {
        switch kind {
        case .live: return live
        case .board: return board
        case .detail: return details.count
        }
    }
}

/// Each injected source counts reads and can suspend its next answer.
actor ProjectTestSource {
    private struct RequestNotice {
        let kind: ProjectTestRequest
        let count: Int
        let notify: @Sendable () -> Void
    }

    private let fixture: ProjectTestFixtures
    private(set) var counts = ProjectTestCounts()
    private var paused: Set<ProjectTestRequest> = []
    private var liveReply: CheckedContinuation<LiveAnswer, Never>?
    private var boardReply: CheckedContinuation<Result<ProjectBoard, LocalAPIFailure>, Never>?
    private var detailReply: CheckedContinuation<Result<ProjectDetail, LocalAPIFailure>, Never>?
    private var detailFailure: LocalAPIFailure?
    private var notices: [UUID: RequestNotice] = [:]

    init(_ fixture: ProjectTestFixtures) { self.fixture = fixture }

    func holdNext(_ kind: ProjectTestRequest) { paused.insert(kind) }
    func failDetail(_ failure: LocalAPIFailure?) { detailFailure = failure }

    /// Registration also sees requests that arrived before the test resumed.
    func notifyWhenRequested(_ kind: ProjectTestRequest, count: Int,
                             _ notify: @escaping @Sendable () -> Void) -> UUID {
        let id = UUID()
        if counts.number(kind) >= count {
            notify()
        } else {
            notices[id] = RequestNotice(kind: kind, count: count, notify: notify)
        }
        return id
    }

    func cancelNotice(_ id: UUID) { notices.removeValue(forKey: id) }

    private func announceRequests() {
        let ready = notices.filter { _, notice in counts.number(notice.kind) >= notice.count }
        for (id, notice) in ready {
            notices.removeValue(forKey: id)
            notice.notify()
        }
    }

    func askLive() async -> LiveAnswer {
        counts.live += 1
        if paused.remove(.live) != nil {
            return await withCheckedContinuation { continuation in
                liveReply = continuation
                announceRequests()
            }
        }
        announceRequests()
        return .snapshot(fixture.live)
    }

    func askBoard() async -> Result<ProjectBoard, LocalAPIFailure> {
        counts.board += 1
        if paused.remove(.board) != nil {
            return await withCheckedContinuation { continuation in
                boardReply = continuation
                announceRequests()
            }
        }
        announceRequests()
        return .success(fixture.board)
    }

    func askDetail(_ key: String) async -> Result<ProjectDetail, LocalAPIFailure> {
        counts.details.append(key)
        if paused.remove(.detail) != nil {
            return await withCheckedContinuation { continuation in
                detailReply = continuation
                announceRequests()
            }
        }
        announceRequests()
        return detailFailure.map { failure in .failure(failure) } ?? .success(fixture.detail)
    }

    func finishLive() {
        let reply = liveReply
        liveReply = nil
        reply?.resume(returning: .snapshot(fixture.live))
    }

    func finishBoard(_ failure: LocalAPIFailure? = nil) {
        let reply = boardReply
        boardReply = nil
        reply?.resume(returning: failure.map { reason in .failure(reason) } ?? .success(fixture.board))
    }

    func finishDetail() {
        let reply = detailReply
        detailReply = nil
        reply?.resume(returning: .success(fixture.detail))
    }
}

/// Canceled deliveries stay callable, just as already queued native timers do.
@MainActor
final class ProjectTestClock: LiveClock {
    final class Ticket: LiveTimer {
        let seconds: TimeInterval
        let fire: @MainActor () -> Void
        var queued = false
        var cancelled = false

        init(seconds: TimeInterval, fire: @escaping @MainActor () -> Void) {
            self.seconds = seconds
            self.fire = fire
        }

        func cancel() { cancelled = true }
    }

    private var tickets: [Ticket] = []
    var now = Date(timeIntervalSince1970: 1_800_000_000)
    var onArm: ((TimeInterval) -> Void)?

    func arm(after seconds: TimeInterval, tolerance: TimeInterval,
             _ fire: @escaping @MainActor () -> Void) -> LiveTimer {
        let ticket = Ticket(seconds: seconds, fire: fire)
        tickets.append(ticket)
        onArm?(seconds)
        return ticket
    }

    var active: [TimeInterval] {
        tickets.filter { ticket in !ticket.queued && !ticket.cancelled }.map(\.seconds).sorted()
    }

    var starts: Int { tickets.count }

    func queue(_ seconds: TimeInterval) throws -> @MainActor () -> Void {
        let ticket = try XCTUnwrap(tickets.first { ticket in
            ticket.seconds == seconds && !ticket.queued && !ticket.cancelled
        })
        ticket.queued = true
        return ticket.fire
    }
}

@MainActor
func projectTestController(_ source: ProjectTestSource, clock: ProjectTestClock) -> AppProjectController {
    AppProjectController(clock: clock, date: { clock.now },
                         ask: { await source.askLive() },
                         board: { await source.askBoard() },
                         detail: { key in await source.askDetail(key) })
}
