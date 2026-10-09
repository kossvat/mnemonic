import Foundation

/// The summary of one day of the recall log, as `mnemonic recall --json`
/// prints it and as GET /api/recall answers: what memory returned on one
/// local calendar day. It is decoded by hand from the bytes, so that a
/// reply that is not the object, or misses a field, is refused as a whole
/// rather than shown with a zero where a number was missing.
public struct RecallDaySummary: Equatable {
    public struct ToolCount: Equatable {
        public let tool: String
        public let events: Int
        public let memories: Int

        public init(tool: String, events: Int, memories: Int) {
            self.tool = tool
            self.events = events
            self.memories = memories
        }
    }

    public struct ClientCount: Equatable {
        public let client: String
        public let events: Int

        public init(client: String, events: Int) {
            self.client = client
            self.events = events
        }
    }

    public struct TopMemory: Equatable {
        public let memoryId: String
        public let title: String
        public let memoryType: String
        public let times: Int

        public init(memoryId: String, title: String, memoryType: String, times: Int) {
            self.memoryId = memoryId
            self.title = title
            self.memoryType = memoryType
            self.times = times
        }
    }

    public let day: String
    public let logEnabled: Bool
    public let contextsGenerated: Int
    public let callsAnswered: Int
    public let memoriesReturned: Int
    public let correctionsReturned: Int
    public let hiddenAsNoise: Int
    public let distinctMemories: Int
    public let byTool: [ToolCount]
    public let byClient: [ClientCount]
    public let top: [TopMemory]

    public init(day: String, logEnabled: Bool, contextsGenerated: Int, callsAnswered: Int,
                memoriesReturned: Int, correctionsReturned: Int, hiddenAsNoise: Int,
                distinctMemories: Int, byTool: [ToolCount], byClient: [ClientCount],
                top: [TopMemory]) {
        self.day = day
        self.logEnabled = logEnabled
        self.contextsGenerated = contextsGenerated
        self.callsAnswered = callsAnswered
        self.memoriesReturned = memoriesReturned
        self.correctionsReturned = correctionsReturned
        self.hiddenAsNoise = hiddenAsNoise
        self.distinctMemories = distinctMemories
        self.byTool = byTool
        self.byClient = byClient
        self.top = top
    }

    /// Whether the day has any event.
    public var isEmpty: Bool { contextsGenerated == 0 && callsAnswered == 0 }

    /// The object in `bytes`, or nil when the bytes are not the object:
    /// not JSON, not an object, a field missing or of another type.
    public static func decode(_ bytes: Data) -> RecallDaySummary? {
        guard let json = try? JSONSerialization.jsonObject(with: bytes),
              let object = json as? [String: Any]
        else { return nil }
        return decode(object)
    }

    /// The summary in a parsed object; nil when a field is missing or of
    /// another type. A client that is null is "unknown".
    public static func decode(_ object: [String: Any]) -> RecallDaySummary? {
        guard let day = object["day"] as? String,
              let logEnabled = bool(object["log_enabled"]),
              let contextsGenerated = count(object["contexts_generated"]),
              let callsAnswered = count(object["calls_answered"]),
              let memoriesReturned = count(object["memories_returned"]),
              let correctionsReturned = count(object["corrections_returned"]),
              let hiddenAsNoise = count(object["hidden_as_noise"]),
              let distinctMemories = count(object["distinct_memories"]),
              let toolRows = object["by_tool"] as? [Any],
              let clientRows = object["by_client"] as? [Any],
              let topRows = object["top"] as? [Any]
        else { return nil }

        var byTool: [ToolCount] = []
        for row in toolRows {
            guard let r = row as? [String: Any],
                  let tool = r["tool"] as? String,
                  let events = count(r["events"]),
                  let memories = count(r["memories"])
            else { return nil }
            byTool.append(ToolCount(tool: tool, events: events, memories: memories))
        }

        var byClient: [ClientCount] = []
        for row in clientRows {
            guard let r = row as? [String: Any],
                  let client = clientName(r["client"]),
                  let events = count(r["events"])
            else { return nil }
            byClient.append(ClientCount(client: client, events: events))
        }

        var top: [TopMemory] = []
        for row in topRows {
            guard let r = row as? [String: Any],
                  let memoryId = r["memory_id"] as? String,
                  let title = r["title"] as? String,
                  let memoryType = r["memory_type"] as? String,
                  let times = count(r["times"])
            else { return nil }
            top.append(TopMemory(memoryId: memoryId, title: title, memoryType: memoryType, times: times))
        }

        return RecallDaySummary(
            day: day, logEnabled: logEnabled,
            contextsGenerated: contextsGenerated, callsAnswered: callsAnswered,
            memoriesReturned: memoriesReturned, correctionsReturned: correctionsReturned,
            hiddenAsNoise: hiddenAsNoise, distinctMemories: distinctMemories,
            byTool: byTool, byClient: byClient, top: top
        )
    }

    /// A client's name: null is a client that gave none; a missing field
    /// or another type is a reply that is not the object.
    private static func clientName(_ v: Any?) -> String? {
        if v is NSNull { return "unknown" }
        return v as? String
    }

    /// A JSON boolean. A number is not one, whatever its value.
    private static func bool(_ v: Any?) -> Bool? {
        guard let n = v as? NSNumber, CFGetTypeID(n) == CFBooleanGetTypeID() else { return nil }
        return n.boolValue
    }

    /// A JSON number that is a whole count. A boolean is not one.
    private static func count(_ v: Any?) -> Int? {
        guard let n = v as? NSNumber, CFGetTypeID(n) != CFBooleanGetTypeID() else { return nil }
        let d = n.doubleValue
        guard d >= 0, d == d.rounded(), d <= 9_007_199_254_740_992 else { return nil }
        return Int(d)
    }
}

/// What the card of memory today shows.
public enum RecallDayState: Equatable {
    /// A valid object with events: the numbers and the top.
    case filled(RecallDaySummary)
    /// A valid object of zeros.
    case empty(RecallDaySummary)
    /// A valid object with the log off.
    case off(RecallDaySummary)
    /// No source answered with the object.
    case unavailable

    public init(_ summary: RecallDaySummary) {
        if !summary.logEnabled {
            self = .off(summary)
        } else if summary.isEmpty {
            self = .empty(summary)
        } else {
            self = .filled(summary)
        }
    }
}

/// Where the card asks for the day. Each is a closure, so that a test can
/// put its own in place of the route and of the command line.
public enum RecallSource {
    /// The route of the local API: nil when it could not be reached; else
    /// its status and body. A status outside 200..<300 is no answer.
    case route(() -> (status: Int, body: Data)?)
    /// The command line: nil when it could not run; else its output.
    case command(() -> Data?)
}

/// The state the sources gave, and which source gave it.
public struct RecallDayLoad: Equatable {
    public let state: RecallDayState
    /// The index of the source that answered with the object, if any.
    public let source: Int?

    public init(state: RecallDayState, source: Int?) {
        self.state = state
        self.source = source
    }
}

public enum RecallDay {
    /// The first source that answers with the object gives the state, and
    /// the sources after it are not asked. When none does, the state is
    /// unavailable: the card does not depend on the fallback of the whole
    /// widget, and an older daemon whose API lacks the route still ends in
    /// the command line.
    public static func load(_ sources: [RecallSource]) -> RecallDayLoad {
        for (index, source) in sources.enumerated() {
            let body: Data?
            switch source {
            case .route(let ask):
                guard let reply = ask(), (200..<300).contains(reply.status) else { continue }
                body = reply.body
            case .command(let run):
                body = run()
            }
            if let body, let summary = RecallDaySummary.decode(body) {
                return RecallDayLoad(state: RecallDayState(summary), source: index)
            }
        }
        return RecallDayLoad(state: .unavailable, source: nil)
    }
}
