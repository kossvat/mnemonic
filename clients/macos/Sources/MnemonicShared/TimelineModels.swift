import Foundation

/// Full local-day bounds remain intact when today's evidence ends at now.
public struct ProjectTimeline: Codable, Equatable, Sendable {
    public let day: String
    public let from: Double
    public let to: Double
    public let projects: [TimelineProject]
    public let segments: [TimelineSegment]
    public let owner: [TimelineOwnerRange]
    public let ownerAvailable: Bool

    public init(day: String, from: Double, to: Double, projects: [TimelineProject],
                segments: [TimelineSegment], owner: [TimelineOwnerRange], ownerAvailable: Bool) {
        self.day = day
        self.from = from
        self.to = to
        self.projects = projects
        self.segments = segments
        self.owner = owner
        self.ownerAvailable = ownerAvailable
    }

    enum CodingKeys: String, CodingKey {
        case day, from, to, projects, segments, owner
        case ownerAvailable = "owner_available"
    }
}

public struct TimelineProject: Codable, Equatable, Sendable, Identifiable {
    public let key: String
    public let name: String
    public var id: String { key }

    public init(key: String, name: String) { self.key = key; self.name = name }
}

public struct TimelineSegment: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let stream: String
    public let project: String?
    public let agent: String
    public let driver: String
    public let from: Double
    public let to: Double

    public init(id: String, stream: String, project: String?, agent: String, driver: String,
                from: Double, to: Double) {
        self.id = id
        self.stream = stream
        self.project = project
        self.agent = agent
        self.driver = driver
        self.from = from
        self.to = to
    }
}

public struct TimelineOwnerRange: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let from: Double
    public let to: Double
    public let assignments: [TimelineAssignment]

    public init(id: String, from: Double, to: Double, assignments: [TimelineAssignment]) {
        self.id = id
        self.from = from
        self.to = to
        self.assignments = assignments
    }
}

public struct TimelineAssignment: Codable, Equatable, Sendable {
    public let project: String?
    public let share: Double
    public let confidence: String?

    public init(project: String?, share: Double, confidence: String?) {
        self.project = project
        self.share = share
        self.confidence = confidence
    }
}
