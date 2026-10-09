import Foundation
import MnemonicShared
import XCTest

/// The model of the card of memory today, on the file the command printed
/// for the fixture store of the daemon's tests, and on replies bent out
/// of it. The sources are closures the tests put in place.
final class RecallDayTests: XCTestCase {
    /// The file the daemon's test compares with the bytes of the command.
    static let fixtureURL = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent()
        .deletingLastPathComponent()
        .appendingPathComponent("Fixtures/recall-day.json")

    private func fixture() throws -> Data {
        try Data(contentsOf: Self.fixtureURL)
    }

    private func object(_ bytes: Data) throws -> [String: Any] {
        try XCTUnwrap(JSONSerialization.jsonObject(with: bytes) as? [String: Any])
    }

    private func bytes(_ object: [String: Any]) throws -> Data {
        try JSONSerialization.data(withJSONObject: object)
    }

    private func expected() -> RecallDaySummary {
        RecallDaySummary(
            day: "2026-03-08",
            logEnabled: true,
            contextsGenerated: 2,
            callsAnswered: 1,
            memoriesReturned: 6,
            correctionsReturned: 1,
            hiddenAsNoise: 2,
            distinctMemories: 3,
            byTool: [
                .init(tool: "context", events: 1, memories: 3),
                .init(tool: "memory_context", events: 1, memories: 1),
                .init(tool: "memory_search", events: 1, memories: 2),
            ],
            byClient: [
                .init(client: "claude code", events: 2),
                .init(client: "unknown", events: 1),
            ],
            top: [
                .init(memoryId: "a1", title: "Decision: use SQLite for the index",
                      memoryType: "decision", times: 3),
                .init(memoryId: "c3", title: "Decision: keep the smaller index",
                      memoryType: "decision", times: 2),
                .init(memoryId: "b2", title: "stop, use the other parser",
                      memoryType: "feedback", times: 1),
            ]
        )
    }

    func testTheFixtureFileDecodesToEveryValue() throws {
        let summary = try XCTUnwrap(RecallDaySummary.decode(try fixture()))
        XCTAssertEqual(summary, expected())
        XCTAssertFalse(summary.isEmpty)
        XCTAssertEqual(RecallDayState(summary), .filled(summary))
    }

    func testAnObjectOfZerosIsTheStateEmpty() throws {
        var zeros = try object(fixture())
        for key in ["contexts_generated", "calls_answered", "memories_returned",
                    "corrections_returned", "hidden_as_noise", "distinct_memories"] {
            zeros[key] = 0
        }
        for key in ["by_tool", "by_client", "top"] {
            zeros[key] = [Any]()
        }
        let summary = try XCTUnwrap(RecallDaySummary.decode(try bytes(zeros)))
        XCTAssertTrue(summary.isEmpty)
        XCTAssertEqual(summary.day, "2026-03-08")
        XCTAssertEqual(summary.byTool, [])
        XCTAssertEqual(summary.top, [])
        XCTAssertEqual(RecallDayState(summary), .empty(summary))
    }

    func testAnObjectWithTheLogOffIsTheStateOff() throws {
        // With events, and without: the log being off is what matters.
        var off = try object(fixture())
        off["log_enabled"] = false
        let withEvents = try XCTUnwrap(RecallDaySummary.decode(try bytes(off)))
        XCTAssertEqual(RecallDayState(withEvents), .off(withEvents))
        XCTAssertEqual(withEvents.memoriesReturned, 6)

        off["contexts_generated"] = 0
        off["calls_answered"] = 0
        let withoutEvents = try XCTUnwrap(RecallDaySummary.decode(try bytes(off)))
        XCTAssertTrue(withoutEvents.isEmpty)
        XCTAssertEqual(RecallDayState(withoutEvents), .off(withoutEvents))
    }

    func testAMalformedReplyIsNotTheObject() throws {
        for text in ["", "{", "{\"day\": \"2026-03-08\", ", "[1, 2]", "\"2026-03-08\"", "null", "7"] {
            XCTAssertNil(RecallDaySummary.decode(Data(text.utf8)), text)
        }
    }

    func testAReplyWithAFieldMissingIsNotTheObject() throws {
        let fields = ["day", "log_enabled", "contexts_generated", "calls_answered",
                      "memories_returned", "corrections_returned", "hidden_as_noise",
                      "distinct_memories", "by_tool", "by_client", "top"]
        for field in fields {
            var missing = try object(fixture())
            missing.removeValue(forKey: field)
            XCTAssertNil(RecallDaySummary.decode(try bytes(missing)), field)
        }
        // A field of a row, too.
        for (array, field) in [("by_tool", "memories"), ("by_client", "events"),
                               ("by_client", "client"), ("top", "times"), ("top", "title")] {
            var missing = try object(fixture())
            var rows = try XCTUnwrap(missing[array] as? [[String: Any]])
            rows[0].removeValue(forKey: field)
            missing[array] = rows
            XCTAssertNil(RecallDaySummary.decode(try bytes(missing)), "\(array).\(field)")
        }
    }

    func testAFieldOfAnotherTypeIsNotTheObject() throws {
        let wrong: [(String, Any)] = [
            ("log_enabled", 1), ("log_enabled", "true"),
            ("calls_answered", true), ("calls_answered", -1), ("calls_answered", 1.5),
            ("calls_answered", "1"), ("day", 20_260_308), ("by_tool", "context"),
            ("top", [["memory_id": "a1"]]),
        ]
        for (field, value) in wrong {
            var bent = try object(fixture())
            bent[field] = value
            XCTAssertNil(RecallDaySummary.decode(try bytes(bent)), "\(field) = \(value)")
        }
    }

    func testAClientThatIsNullIsUnknown() throws {
        var bent = try object(fixture())
        bent["by_client"] = [["client": NSNull(), "events": 3]]
        let summary = try XCTUnwrap(RecallDaySummary.decode(try bytes(bent)))
        XCTAssertEqual(summary.byClient, [.init(client: "unknown", events: 3)])
    }

    func testBothSourcesFailingIsUnavailable() throws {
        let malformed = Data("{\"day\": \"2026-03-08\", ".utf8)
        var missing = try object(fixture())
        missing.removeValue(forKey: "top")

        let none = RecallDay.load([.route { nil }, .command { nil }])
        XCTAssertEqual(none, RecallDayLoad(state: .unavailable, source: nil))

        let malformedTwice = RecallDay.load([
            .route { (status: 200, body: malformed) },
            .command { malformed },
        ])
        XCTAssertEqual(malformedTwice.state, .unavailable)

        let missingThenNothing = RecallDay.load([
            .route { (status: 200, body: try! self.bytes(missing)) },
            .command { Data("[]".utf8) },
        ])
        XCTAssertEqual(missingThenNothing.state, .unavailable)

        XCTAssertEqual(RecallDay.load([]).state, .unavailable)
    }

    func testARouteThatFailsOrAnswers404OrNotTheObjectEndsInTheCommand() throws {
        let file = try fixture()
        let filled = RecallDayLoad(state: .filled(expected()), source: 1)
        let routes: [(String, () -> (status: Int, body: Data)?)] = [
            ("fails", { nil }),
            ("404", { (status: 404, body: Data("not found".utf8)) }),
            ("500 with the object", { (status: 500, body: file) }),
            ("an array", { (status: 200, body: Data("[1, 2]".utf8)) }),
            ("a string", { (status: 200, body: Data("\"ok\"".utf8)) }),
            ("an empty body", { (status: 200, body: Data()) }),
        ]
        for (name, ask) in routes {
            let load = RecallDay.load([.route(ask), .command { file }])
            XCTAssertEqual(load, filled, name)
        }
    }

    func testARouteThatAnswersIsTakenAndTheCommandIsNotRun() throws {
        let file = try fixture()
        var ran = false
        let load = RecallDay.load([
            .route { (status: 200, body: file) },
            .command { ran = true; return file },
        ])
        XCTAssertEqual(load, RecallDayLoad(state: .filled(expected()), source: 0))
        XCTAssertFalse(ran)
    }

    func testAnItemMarkedAsNoiseIsSkippedByTheCardOfTheLatestMemory() {
        let noise: [String: Any] = ["id": "n", "type": "decision", "title": "| Option | SQLite |",
                                    "content": "", "noise": "table-row"]
        let clean: [String: Any] = ["id": "c", "type": "decision",
                                    "title": "Decision: use SQLite for the index", "content": ""]
        let nullMark: [String: Any] = ["id": "m", "memory_type": "feedback",
                                       "title": "stop, use the other parser", "noise": NSNull()]
        let session: [String: Any] = ["id": "s", "type": "session_summary", "title": "Session summary"]
        let event: [String: Any] = ["id": "e", "type": "note", "title": "New file: main.rs", "content": ""]

        XCTAssertTrue(MemoryPick.isNoise(noise))
        XCTAssertFalse(MemoryPick.isNoise(nullMark))
        XCTAssertFalse(MemoryPick.isMeaningful(noise))
        XCTAssertTrue(MemoryPick.isMeaningful(clean))
        XCTAssertTrue(MemoryPick.isMeaningful(nullMark))
        XCTAssertFalse(MemoryPick.isMeaningful(session))
        XCTAssertFalse(MemoryPick.isMeaningful(event))

        XCTAssertEqual(MemoryPick.latest(of: [noise, clean])?["id"] as? String, "c")
        XCTAssertEqual(MemoryPick.latest(of: [noise, session, nullMark])?["id"] as? String, "m")
        // What is not meaningful is still shown when nothing else is there;
        // what is marked as noise is not.
        XCTAssertEqual(MemoryPick.latest(of: [noise, session])?["id"] as? String, "s")
        XCTAssertEqual(MemoryPick.latest(of: [event])?["id"] as? String, "e")
        XCTAssertNil(MemoryPick.latest(of: [noise]))
        XCTAssertNil(MemoryPick.latest(of: []))
    }
}
