import Foundation
import SwiftUI

struct TypeCount: Decodable, Identifiable, Hashable {
    let memoryType: String
    let count: Int

    var id: String { memoryType }

    enum CodingKeys: String, CodingKey {
        case memoryType = "memory_type"
        case count
    }
}

struct StatusResponse: Decodable, Hashable {
    let totalMemories: Int
    let byType: [TypeCount]
    let entities: Int
    let edges: Int
    let lastActivity: String?

    enum CodingKeys: String, CodingKey {
        case totalMemories = "total_memories"
        case byType = "by_type"
        case entities
        case edges
        case lastActivity = "last_activity"
    }

    func count(for type: String) -> Int {
        byType.first { $0.memoryType == type }?.count ?? 0
    }
}

struct MemoryListResponse: Decodable {
    let results: [Memory]
    let count: Int
}

struct Memory: Decodable, Identifiable, Hashable {
    let id: String
    let timestamp: String
    let title: String
    let content: String
    let memoryType: String
    let tags: [String]
    let importance: Double

    enum CodingKeys: String, CodingKey {
        case id
        case timestamp
        case title
        case content
        case memoryType = "memory_type"
        case tags
        case importance
    }
}

struct EntitiesResponse: Decodable {
    let results: [EntitySummary]
}

struct EntitySummary: Decodable, Identifiable, Hashable {
    let name: String
    let type: String
    let mentions: Int

    var id: String { name }
}

struct EntityDetail: Decodable, Hashable {
    let entityName: String
    let entityType: String
    let mentionCount: Int
    let firstSeen: String
    let lastSeen: String
    let aliases: [String]
    let edges: [GraphEdge]
    let memories: [GraphMemory]
    let neighbors: [GraphNeighbor]
    let found: Bool

    enum CodingKeys: String, CodingKey {
        case entityName = "entity_name"
        case entityType = "entity_type"
        case mentionCount = "mention_count"
        case firstSeen = "first_seen"
        case lastSeen = "last_seen"
        case aliases
        case edges
        case memories
        case neighbors
        case found
    }
}

struct GraphResponse: Decodable {
    let nodes: [GraphNode]
    let edges: [GraphEdge]
}

struct GraphNode: Decodable, Identifiable, Hashable {
    let name: String
    let type: String
    let mentions: Int

    var id: String { name }
}

struct GraphEdge: Decodable, Identifiable, Hashable {
    let source: String
    let target: String
    let relation: String
    let weight: Double

    var id: String { "\(source)|\(relation)|\(target)" }
}

struct GraphMemory: Decodable, Identifiable, Hashable {
    let id: String
    let title: String
    let memoryType: String
    let importance: Double
    let timestamp: String

    enum CodingKeys: String, CodingKey {
        case id
        case title
        case memoryType = "memory_type"
        case importance
        case timestamp
    }
}

struct GraphNeighbor: Decodable, Identifiable, Hashable {
    let name: String
    let entityType: String
    let mentionCount: Int

    var id: String { name }

    enum CodingKeys: String, CodingKey {
        case name
        case entityType = "entity_type"
        case mentionCount = "mention_count"
    }
}

struct DailyResponse: Decodable {
    let days: [DailyCount]
}

struct DailyCount: Decodable, Identifiable, Hashable {
    let date: String
    let count: Int

    var id: String { date }
}

struct SearchResponse: Decodable {
    let results: [SearchHit]
    let count: Int
}

struct SearchHit: Decodable, Identifiable, Hashable {
    let id: String
    let title: String
    let contentPreview: String
    let memoryType: String
    let timestamp: String
    let rrfScore: Double
    let sources: String

    enum CodingKeys: String, CodingKey {
        case id
        case title
        case contentPreview = "content_preview"
        case memoryType = "memory_type"
        case timestamp
        case rrfScore = "rrf_score"
        case sources
    }
}

struct DedupeReport: Decodable, Hashable {
    let groups: [DedupeGroup]
    let dryRun: Bool
    let merged: Int
    let renamed: Int
    let edgesRedirected: Int
    let memoryLinksRedirected: Int

    enum CodingKeys: String, CodingKey {
        case groups
        case dryRun = "dry_run"
        case merged
        case renamed
        case edgesRedirected = "edges_redirected"
        case memoryLinksRedirected = "memory_links_redirected"
    }
}

struct DedupeGroup: Decodable, Identifiable, Hashable {
    let canonical: String
    let variants: [String]

    var id: String { canonical }
}

struct ReextractReport: Decodable, Hashable {
    let planned: Int
    let processed: Int
    let entitiesAdded: Int
    let edgesAdded: Int
    let dryRun: Bool
    let extractor: String

    enum CodingKeys: String, CodingKey {
        case planned
        case processed
        case entitiesAdded = "entities_added"
        case edgesAdded = "edges_added"
        case dryRun = "dry_run"
        case extractor
    }
}

struct CleanupReport: Decodable, Hashable {
    let deleted: Int
    let confirmed: Bool
    let note: String?
}

struct MergeReport: Decodable, Hashable {
    let action: String
    let alias: String?
    let canonical: String?
    let from: String?
    let to: String?
    let aliasDropped: Bool?
    let edgesRedirected: Int?
    let memoryLinksRedirected: Int?

    enum CodingKeys: String, CodingKey {
        case action
        case alias
        case canonical
        case from
        case to
        case aliasDropped = "alias_dropped"
        case edgesRedirected = "edges_redirected"
        case memoryLinksRedirected = "memory_links_redirected"
    }
}

struct ForgetReport: Decodable, Hashable {
    let id: String
    let removed: Bool
}

struct APIErrorResponse: Decodable {
    let error: String
}

// MARK: - Reflection (Phase 5)

struct ReflectionPlan: Decodable, Hashable {
    let runId: String
    let mode: String
    let threshold: Double
    let poolSize: Int
    let clusters: [PlannedCluster]

    enum CodingKeys: String, CodingKey {
        case runId = "run_id"
        case mode
        case threshold
        case poolSize = "pool_size"
        case clusters
    }
}

struct PlannedCluster: Decodable, Hashable, Identifiable {
    let sourceIds: [String]
    let cosines: [Double]
    let draftTitle: String
    let draftContent: String
    let applied: Bool
    let canonicalId: String?

    var id: String {
        canonicalId ?? sourceIds.first ?? UUID().uuidString
    }

    enum CodingKeys: String, CodingKey {
        case sourceIds = "source_ids"
        case cosines
        case draftTitle = "draft_title"
        case draftContent = "draft_content"
        case applied
        case canonicalId = "canonical_id"
    }
}

struct MemorySourcesResponse: Decodable {
    let canonicalId: String
    let sources: [MemorySource]
    let count: Int

    enum CodingKeys: String, CodingKey {
        case canonicalId = "canonical_id"
        case sources
        case count
    }
}

struct MemorySource: Decodable, Identifiable, Hashable {
    let id: String
    let title: String
    let timestamp: String
    let memoryType: String
    let importance: Double
    let cosine: Double

    enum CodingKeys: String, CodingKey {
        case id, title, timestamp, importance, cosine
        case memoryType = "memory_type"
    }
}

// MARK: - Memory Graph (Phase 4d)

struct MemoryGraphResponse: Decodable {
    let nodes: [MemoryGraphNode]
    let edges: [MemoryGraphEdge]
}

struct MemoryGraphNode: Decodable, Identifiable, Hashable {
    let id: String
    let title: String
    let memoryType: String
    let timestamp: String
    let importance: Double
    let entityCount: Int
    let contentPreview: String

    enum CodingKeys: String, CodingKey {
        case id, title, timestamp, importance
        case memoryType = "memory_type"
        case entityCount = "entity_count"
        case contentPreview = "content_preview"
    }
}

struct MemoryGraphEdge: Decodable, Identifiable, Hashable {
    let source: String
    let target: String
    let weight: Int
    let shared: [String]

    var id: String { "\(source)|\(target)" }
}
