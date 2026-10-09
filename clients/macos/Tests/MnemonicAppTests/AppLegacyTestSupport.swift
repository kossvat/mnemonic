import Foundation
import MnemonicShared
@testable import MnemonicApp

enum AppLegacyReadFailure: Error { case unavailable }

/// Answers are captured when requested, so a held read can predate a mutation.
@MainActor
final class AppLegacyRead<Value> {
    var answer: Result<Value, Error>
    var held = false
    private(set) var count = 0
    private(set) var active = 0
    private(set) var peakActive = 0
    private var notices: [Int: () -> Void] = [:]
    private var replies: [(CheckedContinuation<Value, Error>, Result<Value, Error>)] = []

    init(_ value: Value) { answer = .success(value) }

    func requested(_ number: Int, _ notice: @escaping () -> Void) {
        if count >= number { notice() } else { notices[number] = notice }
    }

    func read() async throws -> Value {
        count += 1
        active += 1
        peakActive = max(peakActive, active)
        defer { active -= 1 }
        let captured = answer
        let notice = notices.removeValue(forKey: count)
        if held {
            return try await withCheckedThrowingContinuation { reply in
                replies.append((reply, captured))
                notice?()
            }
        }
        notice?()
        return try captured.get()
    }

    func finishNext() {
        guard !replies.isEmpty else { return }
        let (reply, answer) = replies.removeFirst()
        reply.resume(with: answer)
    }

    func finishAll() {
        held = false
        while !replies.isEmpty { finishNext() }
    }
}

@MainActor
final class AppLegacyFixture {
    let root: URL
    let fixture: ProjectTestFixtures
    let source: ProjectTestSource
    let clock = ProjectTestClock()
    let memories = AppLegacyRead(MemoryListResponse(results: [], count: 0))
    let entities = AppLegacyRead(EntitiesResponse(results: []))
    let status = AppLegacyRead(StatusResponse(
        totalMemories: 1, byType: [], entities: 1, edges: 0, lastActivity: nil))
    let model: MnemonicAppModel

    init(_ route: AppRoute) throws {
        root = FileManager.default.temporaryDirectory
            .appendingPathComponent("mn-legacy-reads-\(UUID().uuidString)")
        fixture = try ProjectTestFixtures()
        source = ProjectTestSource(fixture)
        let home = root.appendingPathComponent("profile")
        let user = root.appendingPathComponent("user")
        do {
            try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true)
            try FileManager.default.createDirectory(at: user, withIntermediateDirectories: true)
            try "[ui]\nenabled = false\nport = 9\n".write(
                to: home.appendingPathComponent("config.toml"), atomically: true, encoding: .utf8)
            let profile = try MnemonicProfile.resolve(
                environment: [MnemonicProfile.homeVariable: home.path],
                arguments: ["fixture-client"], userHome: user)
            let source = source, clock = clock
            let memories = memories, entities = entities, status = status
            let controller = AppProjectController(
                clock: clock, date: { clock.now }, ask: { await source.askLive() },
                board: { await source.askBoard() }, detail: { key in await source.askDetail(key) })
            model = MnemonicAppModel(
                profile: profile, controller: controller,
                readMemories: { try await memories.read() },
                readEntities: { try await entities.read() }, readStatus: { try await status.read() })
            model.activateWindow = {}
            model.route = route
            controller.present(board: nil, detail: nil, live: .snapshot(fixture.live), route: route)
        } catch {
            try? FileManager.default.removeItem(at: root)
            throw error
        }
    }

    func close() {
        model.projects.hide()
        memories.finishAll()
        entities.finishAll()
        status.finishAll()
        try? FileManager.default.removeItem(at: root)
    }

    static func memory(_ id: String) -> Memory {
        Memory(id: id, timestamp: "2026-10-04T12:00:00Z", title: "Fixture memory",
               content: "Synthetic content", memoryType: "decision", tags: [], importance: 0.5)
    }

    static func entity(_ name: String) -> EntitySummary {
        EntitySummary(name: name, type: "technology", mentions: 1)
    }
}
