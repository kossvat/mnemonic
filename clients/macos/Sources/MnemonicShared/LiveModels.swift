import Foundation

/// The projects in work now, as the daemon serves them on `/api/live`.
/// Both clients decode this one model.
public struct LiveSnapshot: Codable, Equatable, Sendable {
    /// One run of the daemon, and the version of the snapshot within it.
    public let boot: String
    public let v: Int
    public let projects: [LiveProject]
    /// Projects that are live and were left out by the cap.
    public let moreProjects: Int
    public let unknown: LiveUnknown
    public let today: LiveTotal
    /// The memories in the store, counted when the snapshot was asked
    /// for. An earlier daemon does not send it.
    public let memories: Int?

    enum CodingKeys: String, CodingKey {
        case boot, v, projects, unknown, today, memories
        case moreProjects = "more_projects"
    }

    /// Whether any session is live: the clients keep a request armed only
    /// while one is.
    public var hasLiveSession: Bool {
        !projects.isEmpty || !unknown.sessions.isEmpty || unknown.moreSessions > 0
    }
}

/// What a session, or the most active session of a project, is doing.
public enum LiveState: String, Codable, Sendable {
    case running, waiting, quiet
}

public struct LiveProject: Codable, Equatable, Sendable, Identifiable {
    public let key: String
    public let name: String
    public let state: LiveState
    public let sessions: [LiveSession]
    public let moreSessions: Int
    public let today: LiveToday
    public let plan: LivePlan?

    public var id: String { key }

    enum CodingKeys: String, CodingKey {
        case key, name, state, sessions, today, plan
        case moreSessions = "more_sessions"
    }
}

public struct LiveSession: Codable, Equatable, Sendable {
    /// The kind of the client: `claude` or `codex`.
    public let agent: String
    /// `owner`, or `program` for a session a program ran.
    public let driver: String
    public let state: LiveState
    /// Seconds since the epoch. The snapshot carries the start, never a
    /// duration: the client counts the elapsed time itself.
    public let startedAt: Int
    /// The time of the session's last record. Only the detail route of a
    /// project serves it; `.missing` elsewhere and from an earlier daemon.
    public let lastRecordAt: FieldPresence<Int>

    public var started: Date { Date(timeIntervalSince1970: TimeInterval(startedAt)) }

    enum CodingKeys: String, CodingKey {
        case agent, driver, state
        case startedAt = "started_at"
        case lastRecordAt = "last_record_at"
    }
}

/// The time of a project today: the owner's is an estimate, the agents'
/// is measured.
public struct LiveToday: Codable, Equatable, Sendable {
    public let ownerSeconds: Int
    public let agentSeconds: Int
    public let confidence: String?

    public init(ownerSeconds: Int, agentSeconds: Int, confidence: String?) {
        self.ownerSeconds = ownerSeconds
        self.agentSeconds = agentSeconds
        self.confidence = confidence
    }

    enum CodingKeys: String, CodingKey {
        case confidence
        case ownerSeconds = "owner_s"
        case agentSeconds = "agent_s"
    }
}

public struct LivePlan: Codable, Equatable, Sendable {
    public let unit: String
    public let done: Int
    public let total: Int
    public let stage: String
    public let task: String?
    public let gate: String?

    public init(
        unit: String, done: Int, total: Int, stage: String, task: String?, gate: String?
    ) {
        self.unit = unit
        self.done = done
        self.total = total
        self.stage = stage
        self.task = task
        self.gate = gate
    }
}

public struct LiveUnknown: Codable, Equatable, Sendable {
    public let sessions: [LiveSession]
    public let moreSessions: Int

    enum CodingKeys: String, CodingKey {
        case sessions
        case moreSessions = "more_sessions"
    }
}

public struct LiveTotal: Codable, Equatable, Sendable {
    public let ownerSeconds: Int
    public let agentSeconds: Int

    public init(ownerSeconds: Int, agentSeconds: Int) {
        self.ownerSeconds = ownerSeconds
        self.agentSeconds = agentSeconds
    }

    enum CodingKeys: String, CodingKey {
        case ownerSeconds = "owner_s"
        case agentSeconds = "agent_s"
    }
}
