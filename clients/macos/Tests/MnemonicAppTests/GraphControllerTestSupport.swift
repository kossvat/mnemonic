import Foundation
import XCTest
import MnemonicShared
@testable import MnemonicApp

actor GraphTestReplies<Argument: Sendable, Value: Sendable> {
    private var answer: @Sendable (Argument) -> Result<Value, CuratedGraphFailure>
    private var held = false
    private var calls: [Argument] = []
    private var replies: [Int: (CheckedContinuation<Result<Value, CuratedGraphFailure>, Never>,
                                Result<Value, CuratedGraphFailure>)] = [:]
    private var notices: [Int: @Sendable () -> Void] = [:]
    private var active = 0
    private(set) var peakActive = 0

    init(_ answer: @escaping @Sendable (Argument) -> Result<Value, CuratedGraphFailure>) { self.answer = answer }
    var count: Int { calls.count }
    func arguments() -> [Argument] { calls }
    func hold() { held = true }
    func set(_ result: Result<Value, CuratedGraphFailure>) { answer = { _ in result } }
    func reset(_ answer: @escaping @Sendable (Argument) -> Result<Value, CuratedGraphFailure>) { self.answer = answer }
    func requested(_ number: Int, _ notify: @escaping @Sendable () -> Void) {
        if calls.count >= number { notify() } else { notices[number] = notify }
    }
    func ask(_ argument: Argument) async -> Result<Value, CuratedGraphFailure> {
        calls.append(argument)
        let number = calls.count, captured = answer(argument)
        active += 1
        peakActive = max(peakActive, active)
        defer { active -= 1 }
        let notify = notices.removeValue(forKey: number)
        guard held else { notify?(); return captured }
        return await withCheckedContinuation { reply in
            replies[number] = (reply, captured)
            notify?()
        }
    }
    func finish(_ number: Int) {
        guard let (reply, value) = replies.removeValue(forKey: number) else { return }
        reply.resume(returning: value)
    }
    func finishAll() {
        held = false
        for number in Array(replies.keys) { finish(number) }
    }
}

struct GraphTestPage: Sendable {
    let archived: Bool
    let offset: Int
    let limit: Int
}
struct GraphTestEdgePage: Sendable {
    let edge: GraphEdgeSelection
    let offset: Int
    let limit: Int
}
struct GraphTestMutation: Sendable {
    let target: GraphArchiveTarget
    let archived: Bool
}

enum GraphTestValues {
    static func decode<Value: Decodable>(_ type: Value.Type, _ object: Any) -> Value {
        try! JSONDecoder().decode(type, from: JSONSerialization.data(withJSONObject: object))
    }
    static func snapshot(_ version: Int = 1) -> CuratedGraphSnapshot {
        decode(CuratedGraphSnapshot.self, ["v": 1, "n": [
            ["project:1", "Version \(version)", 0, 0, 0, [10, 30], NSNull(), NSNull(), 0, 1, false, "harbor"],
            ["entity:rust", "Rust", 3, 5000, 0, [NSNull(), NSNull()], NSNull(), NSNull(), 0, 0, false, NSNull()],
        ], "e": [[0, 1, 1, 60]], "s": [
            ["project:2", "Ledger", 0, 2, NSNull(), NSNull(), "ledger"],
            ["entity:phantom", "Phantom", 1, 2, NSNull(), NSNull(), NSNull()],
        ], "o": [0, 0, 0, 100]])
    }
    static func project() -> ProjectDetail {
        let url = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
            .deletingLastPathComponent().appendingPathComponent("Fixtures/project-detail.json")
        return try! JSONDecoder().decode(ProjectDetail.self, from: Data(contentsOf: url))
    }
    static func memory(_ id: String) -> CuratedGraphMemory {
        decode(CuratedGraphMemory.self, ["id": id, "title": "Evidence \(id)", "content": "Fixture content",
            "timestamp": "2026-09-30T12:00:00Z", "memory_type": "fact", "tags": [], "importance": 0.7])
    }
    static func evidence(_ offset: Int, limit: Int = 10) -> CuratedGraphEvidencePage {
        decode(CuratedGraphEvidencePage.self, ["memory_ids": (offset..<(offset + limit)).map { index in "memory-\(index)" },
            "offset": offset, "limit": limit, "next_offset": offset + limit])
    }
    static func archives(_ page: GraphTestPage) -> CuratedGraphArchivePage {
        let rows: [[String: Any]] = (page.offset..<(page.offset + page.limit)).map { index in
            ["id": "outside-\(index)", "name": "Outside \(index)", "memory_count": 1,
             "last_memory": NSNull(), "archived_at": page.archived ? 1 : NSNull()]
        }
        return decode(CuratedGraphArchivePage.self, ["entities": rows, "offset": page.offset,
            "limit": page.limit, "total": 140, "next_offset": page.offset + page.limit])
    }
}

@MainActor
final class GraphControllerFixture {
    let reads = GraphTestReplies<String?, CuratedGraphSnapshot> { _ in .success(GraphTestValues.snapshot()) }
    let projects = GraphTestReplies<String, ProjectDetail> { _ in .success(GraphTestValues.project()) }
    let edges = GraphTestReplies<GraphTestEdgePage, CuratedGraphEvidencePage> { page in
        .success(GraphTestValues.evidence(page.offset, limit: page.limit))
    }
    let memories = GraphTestReplies<String, CuratedGraphMemory> { id in .success(GraphTestValues.memory(id)) }
    let archives = GraphTestReplies<GraphTestPage, CuratedGraphArchivePage> { page in
        .success(GraphTestValues.archives(page))
    }
    let mutations = GraphTestReplies<GraphTestMutation, Void> { _ in .success(()) }
    let controller: AppGraphController

    init(visible: Bool = true) {
        let reads = reads, projects = projects, edges = edges, memories = memories
        let archives = archives, mutations = mutations
        controller = AppGraphController(requests: GraphRequests(
            snapshot: { key in await reads.ask(key) }, project: { key in await projects.ask(key) },
            evidence: { edge, offset, limit in await edges.ask(.init(edge: edge, offset: offset, limit: limit)) },
            memory: { id in await memories.ask(id) },
            archives: { archived, offset, limit in await archives.ask(.init(archived: archived, offset: offset, limit: limit)) },
            mutate: { target, archived in await mutations.ask(.init(target: target, archived: archived)) }))
        controller.setContext(visible: visible)
    }
    func close() {
        controller.setContext(visible: false)
        let reads = reads, projects = projects, edges = edges, memories = memories
        let archives = archives, mutations = mutations
        Task {
            await reads.finishAll()
            await projects.finishAll()
            await edges.finishAll()
            await memories.finishAll()
            await archives.finishAll()
            await mutations.finishAll()
        }
    }
}

extension XCTestCase {
    @MainActor
    func graphArrived<Argument: Sendable, Value: Sendable>(_ source: GraphTestReplies<Argument, Value>, _ count: Int) async {
        let expected = expectation(description: "Graph fixture request \(count) arrived")
        await source.requested(count) { expected.fulfill() }
        await fulfillment(of: [expected], timeout: 5)
    }
}
