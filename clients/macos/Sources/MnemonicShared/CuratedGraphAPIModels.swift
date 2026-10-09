import Foundation

public enum CuratedGraphFailure: Error, Equatable, Sendable {
    case local(LocalAPIFailure)
    case notFound
    case unavailable

    public var message: String {
        switch self {
        case .notFound: return "This graph item is no longer available."
        case .unavailable: return "The graph is currently unavailable."
        case .local(.off): return "The local API is disabled."
        case .local(.unreachable): return "The local daemon could not be reached."
        case .local(.unauthorised): return "The local API token was refused."
        case .local(.earlierDaemon): return "This daemon does not provide the curated graph."
        case .local(.undecodable): return "The graph response could not be read."
        }
    }
}

public struct CuratedGraphEvidencePage: Codable, Equatable, Sendable {
    public let memoryIDs: [String]
    public let offset: Int
    public let limit: Int
    public let nextOffset: Int?
    enum CodingKeys: String, CodingKey {
        case memoryIDs = "memory_ids", offset, limit, nextOffset = "next_offset"
    }
}

public struct CuratedGraphArchiveItem: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let name: String
    public let memoryCount: Int
    public let lastMemory: Int64?
    public let archivedAt: Int64?
    enum CodingKeys: String, CodingKey {
        case id, name, memoryCount = "memory_count", lastMemory = "last_memory"
        case archivedAt = "archived_at"
    }
}

public struct CuratedGraphArchivePage: Codable, Equatable, Sendable {
    public let entities: [CuratedGraphArchiveItem]
    public let offset: Int
    public let limit: Int
    public let total: Int
    public let nextOffset: Int?
    enum CodingKeys: String, CodingKey {
        case entities, offset, limit, total, nextOffset = "next_offset"
    }
}

public struct CuratedGraphMemory: Codable, Equatable, Sendable, Identifiable {
    public let id: String
    public let title: String
    public let content: String
    public let timestamp: String
    public let memoryType: String
    public let tags: [String]
    public let importance: Double
    enum CodingKeys: String, CodingKey {
        case id, title, content, timestamp, tags, importance, memoryType = "memory_type"
    }
}

extension CuratedGraphLinkKind {
    public var queryValue: String {
        switch self {
        case .contains: return "contains"
        case .uses: return "uses"
        case .involves: return "involves"
        case .relates: return "relates"
        }
    }
}
