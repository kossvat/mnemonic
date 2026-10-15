import Foundation

/// The project board returned by the local project collection route.
public struct ProjectBoard: Codable, Equatable, Sendable {
    public let projects: [ProjectBoardRow]
    /// The owner's and the agents' time over the board's seven days, once
    /// for the board; `.missing` from a daemon of an earlier build.
    public let week: FieldPresence<ProjectWeek>

    enum CodingKeys: String, CodingKey {
        case projects, week
    }
}

/// Registry state differs from the activity state of a live session.
public enum ProjectState: String, Codable, Sendable {
    case active, archived, ignored
}

public struct ProjectBoardRow: Codable, Equatable, Sendable, Identifiable {
    public let key: String
    public let name: String
    public let state: ProjectState
    /// The canonical project key when this row has been merged.
    public let mergedInto: String?
    public let lastActiveAt: Int?
    public let week: ProjectWeek
    public let plan: LivePlan?
    public let planMessage: String?
    /// Values a daemon of an earlier build does not serve: `.missing` then.
    public let gateAt: FieldPresence<Int>
    public let planProvider: FieldPresence<ProjectPlanProvider>
    public let planCounts: FieldPresence<[String: Int]>
    public let planWeeklyCount: FieldPresence<Int>
    public let openUnits: FieldPresence<Int>
    /// Reasons, the owner's own text or titles of tasks, shown verbatim.
    public let waitingOnOwner: FieldPresence<[String]>
    public let lastClosed: FieldPresence<ProjectLastClosed>

    public var id: String { key }

    enum CodingKeys: String, CodingKey {
        case key, name, state, week, plan
        case mergedInto = "merged_into"
        case lastActiveAt = "last_active_at"
        case planMessage = "plan_message"
        case gateAt = "gate_at"
        case planProvider = "plan_provider"
        case planCounts = "plan_counts"
        case planWeeklyCount = "plan_weekly_count"
        case openUnits = "open_units"
        case waitingOnOwner = "waiting_on_owner"
        case lastClosed = "last_closed"
    }
}

/// The closed unit the reader holds, and its closure time when one is
/// recorded.
public struct ProjectLastClosed: Codable, Equatable, Sendable {
    public let title: String
    /// A recorded null says that no closure time is recorded.
    public let closedAt: FieldPresence<Int>

    enum CodingKeys: String, CodingKey {
        case title
        case closedAt = "closed_at"
    }
}

public struct ProjectWeek: Codable, Equatable, Sendable {
    /// Missing attribution is unknown, rather than zero owner time.
    public let ownerSeconds: Double?
    public let agentSeconds: Int
    public let ownerEstimate: Bool

    enum CodingKeys: String, CodingKey {
        case ownerSeconds = "owner_s"
        case agentSeconds = "agent_s"
        case ownerEstimate = "owner_estimate"
    }
}

/// The detail route keeps the complete plan beside sessions, time and history.
public struct ProjectDetail: Codable, Equatable, Sendable, Identifiable {
    public let key: String
    public let name: String
    public let state: ProjectState
    public let plan: ProjectPlan
    public let sessions: [LiveSession]
    public let gateDetail: [String: ProjectGateDetail]?
    public let time: [ProjectDay]
    public let ownerEstimate: Bool
    public let history: [ProjectHistoryEntry]
    public let topTerms: [ProjectTerm]?
    public let topTermsPresence: FieldPresence<[ProjectTerm]>

    public var id: String { key }

    enum CodingKeys: String, CodingKey {
        case key, name, state, plan, sessions, time, history
        case topTerms = "top_terms"
        case gateDetail = "gate_detail"
        case ownerEstimate = "owner_estimate"
    }
}

public struct ProjectPlan: Codable, Equatable, Sendable {
    public let units: [ProjectPlanUnit]
    public let message: String?
}

public enum ProjectPlanStage: String, Codable, Sendable {
    case proposed, planned, building, review, blocked, done
}

public enum ProjectPlanProvider: String, Codable, Sendable {
    case registry, checkbox, funnel
}

public struct ProjectPlanUnit: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let title: String
    public let done: Int
    public let total: Int
    public let stage: ProjectPlanStage
    public let stale: Bool
    public let currentTask: ProjectCurrentTask?
    public let counts: [String: Int]
    public let waitingOnOwner: [String]
    public let weeklyCount: Int
    public let gate: ProjectGate
    public let provider: ProjectPlanProvider
    public let checkouts: Int
    public let asOf: Int
    public let tasks: [ProjectTask]
    public let error: String?
    /// Whether the reader holds the unit as closed; `.missing` from a
    /// daemon of an earlier build, which does not say.
    public let closed: FieldPresence<Bool>
    public let closedAt: FieldPresence<Int>

    /// Status tables have counts, never a task completion percentage.
    public var completionFraction: Double? {
        guard provider != .funnel, total > 0 else { return nil }
        return Double(done) / Double(total)
    }

    enum CodingKeys: String, CodingKey {
        case id, title, done, total, stage, stale, counts, gate, provider, checkouts, tasks, error
        case currentTask = "current_task"
        case waitingOnOwner = "waiting_on_owner"
        case weeklyCount = "weekly_count"
        case asOf = "as_of"
        case closed
        case closedAt = "closed_at"
    }
}

public struct ProjectCurrentTask: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let title: String
    /// The task registry, the session branch, or the first open task.
    public let source: String
}

public struct ProjectTask: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let title: String
    public let status: String
    public let owner: String?
}

public enum ProjectGateStatus: String, Codable, Sendable {
    case passed, failed, stale, none
}

/// What the newest receipt says of its own run.
public enum ProjectGateResult: String, Codable, Sendable {
    case passed, failed
}

public struct ProjectGate: Codable, Equatable, Sendable {
    public let status: ProjectGateStatus
    public let at: Int?
    public let failedRuns: Int
    public let result: FieldPresence<ProjectGateResult>

    enum CodingKeys: String, CodingKey {
        case status, at, result
        case failedRuns = "failed_runs"
    }
}

/// A bounded checkout inspection can leave either measurement unknown.
public struct ProjectGateDetail: Codable, Equatable, Sendable {
    public let dirty: Bool?
    public let commitsSince: Int?

    enum CodingKeys: String, CodingKey {
        case dirty
        case commitsSince = "commits_since"
    }
}

public struct ProjectDay: Codable, Equatable, Sendable, Identifiable {
    public let day: String
    public let ownerSeconds: Double?
    public let agentSeconds: Int
    public let confidence: String?

    public var id: String { day }

    enum CodingKeys: String, CodingKey {
        case day, confidence
        case ownerSeconds = "owner_s"
        case agentSeconds = "agent_s"
    }
}

public struct ProjectHistoryEntry: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let title: String
    public let at: Int
    /// "decision", or "correction" for a memory the store types as feedback.
    public let kind: FieldPresence<String>

    enum CodingKeys: String, CodingKey {
        case id, title, at, kind
    }
}

public struct ProjectTerm: Codable, Equatable, Sendable {
    public let name: String
    public let memoryCount: Int

    enum CodingKeys: String, CodingKey {
        case name
        case memoryCount = "memory_count"
    }
}
