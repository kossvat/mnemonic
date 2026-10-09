import Foundation

/// Compact daemon graph v1, decoded into named fields before the UI uses it.
/// Links are validated against this snapshot's nodes during decoding.
public struct CuratedGraphSnapshot: Codable, Equatable, Sendable {
    public let schemaVersion: UInt8
    public let nodes: [CuratedGraphNode]
    public let links: [CuratedGraphLink]
    public let shelf: [CuratedGraphShelfItem]
    public let omitted: CuratedGraphOmissions

    enum CodingKeys: String, CodingKey {
        case schemaVersion = "v", nodes = "n", links = "e", shelf = "s", omitted = "o"
    }

    public init(from decoder: Decoder) throws {
        let values = try decoder.container(keyedBy: CodingKeys.self)
        schemaVersion = try values.decode(UInt8.self, forKey: .schemaVersion)
        guard schemaVersion == 1 else {
            throw DecodingError.dataCorruptedError(
                forKey: .schemaVersion, in: values, debugDescription: "Unsupported graph schema")
        }
        nodes = try values.decode([CuratedGraphNode].self, forKey: .nodes)
        links = try values.decode([CuratedGraphLink].self, forKey: .links)
        shelf = try values.decode([CuratedGraphShelfItem].self, forKey: .shelf)
        omitted = try values.decode(CuratedGraphOmissions.self, forKey: .omitted)
        guard Set(nodes.map(\.key)).count == nodes.count else {
            throw DecodingError.dataCorruptedError(
                forKey: .nodes, in: values, debugDescription: "Duplicate graph node key")
        }
        guard links.allSatisfy({ link in nodes.indices.contains(link.sourceIndex)
            && nodes.indices.contains(link.targetIndex) }) else {
            throw DecodingError.dataCorruptedError(
                forKey: .links, in: values, debugDescription: "Graph link index is out of range")
        }
    }
}

public enum CuratedGraphNodeKind: UInt8, Codable, Equatable, Sendable {
    case project = 0, part, unit, technology, person
}

public enum CuratedGraphNodeState: UInt8, Codable, Equatable, Sendable {
    case active = 0, quiet, stale, archived
}

public enum CuratedGraphLinkKind: UInt8, Codable, Equatable, Sendable {
    case contains = 0, uses, involves, relates
}

public enum CuratedGraphShelfReason: UInt8, Codable, Equatable, Sendable {
    case staleProject = 0, outsideRegistry
}

/// Two separate measures in tenths of an hour. Null remains unknown.
public struct CuratedGraphHours: Codable, Equatable, Sendable {
    public let ownerTenths: UInt32?
    public let agentTenths: UInt32?

    public var ownerHours: Double? { ownerTenths.map { tenths in Double(tenths) / 10 } }
    public var agentHours: Double? { agentTenths.map { tenths in Double(tenths) / 10 } }

    public init(from decoder: Decoder) throws {
        var values = try decoder.unkeyedContainer()
        ownerTenths = try values.decode(UInt32?.self)
        agentTenths = try values.decode(UInt32?.self)
        try requireGraphTupleEnd(values)
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.unkeyedContainer()
        try values.encode(ownerTenths)
        try values.encode(agentTenths)
    }
}

/// key, label, kind, x, y, hours, last activity, inactive days, state,
/// agent-hour size bucket, closed plan flag, optional registry slug.
public struct CuratedGraphNode: Codable, Equatable, Sendable, Identifiable {
    public let key: String
    public let label: String
    public let kind: CuratedGraphNodeKind
    public let x: Int32
    public let y: Int32
    public let hours30Days: CuratedGraphHours
    public let lastActivity: Int64?
    public let inactiveDays: UInt32?
    public let state: CuratedGraphNodeState
    public let sizeBucket: UInt8
    public let isClosed: Bool
    public let projectSlug: String?

    public var id: String { key }

    public init(from decoder: Decoder) throws {
        var values = try decoder.unkeyedContainer()
        key = try values.decode(String.self)
        label = try values.decode(String.self)
        kind = try values.decode(CuratedGraphNodeKind.self)
        x = try values.decode(Int32.self)
        y = try values.decode(Int32.self)
        hours30Days = try values.decode(CuratedGraphHours.self)
        lastActivity = try values.decode(Int64?.self)
        inactiveDays = try values.decode(UInt32?.self)
        state = try values.decode(CuratedGraphNodeState.self)
        sizeBucket = try values.decode(UInt8.self)
        isClosed = try values.decode(Bool.self)
        projectSlug = try values.decode(String?.self)
        try requireGraphTupleEnd(values)
        guard sizeBucket <= 4 else {
            throw DecodingError.dataCorruptedError(
                in: values, debugDescription: "Invalid graph size bucket")
        }
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.unkeyedContainer()
        try values.encode(key)
        try values.encode(label)
        try values.encode(kind)
        try values.encode(x)
        try values.encode(y)
        try values.encode(hours30Days)
        try values.encode(lastActivity)
        try values.encode(inactiveDays)
        try values.encode(state)
        try values.encode(sizeBucket)
        try values.encode(isClosed)
        try values.encode(projectSlug)
    }
}

/// The indices address `CuratedGraphSnapshot.nodes`, never another response.
public struct CuratedGraphLink: Codable, Equatable, Sendable {
    public let sourceIndex: Int
    public let targetIndex: Int
    public let kind: CuratedGraphLinkKind
    public let memoryCount: UInt32

    public init(from decoder: Decoder) throws {
        var values = try decoder.unkeyedContainer()
        sourceIndex = Int(try values.decode(UInt16.self))
        targetIndex = Int(try values.decode(UInt16.self))
        kind = try values.decode(CuratedGraphLinkKind.self)
        memoryCount = try values.decode(UInt32.self)
        try requireGraphTupleEnd(values)
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.unkeyedContainer()
        try values.encode(sourceIndex)
        try values.encode(targetIndex)
        try values.encode(kind)
        try values.encode(memoryCount)
    }
}

/// Archived entries are absent; an archive history route supplies undo rows.
public struct CuratedGraphShelfItem: Codable, Equatable, Sendable, Identifiable {
    public let key: String
    public let label: String
    public let reason: CuratedGraphShelfReason
    public let memoryCount: UInt32
    public let lastActivity: Int64?
    public let inactiveDays: UInt32?
    public let projectSlug: String?

    public var id: String { key }

    public init(from decoder: Decoder) throws {
        var values = try decoder.unkeyedContainer()
        key = try values.decode(String.self)
        label = try values.decode(String.self)
        reason = try values.decode(CuratedGraphShelfReason.self)
        memoryCount = try values.decode(UInt32.self)
        lastActivity = try values.decode(Int64?.self)
        inactiveDays = try values.decode(UInt32?.self)
        projectSlug = try values.decode(String?.self)
        try requireGraphTupleEnd(values)
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.unkeyedContainer()
        try values.encode(key)
        try values.encode(label)
        try values.encode(reason)
        try values.encode(memoryCount)
        try values.encode(lastActivity)
        try values.encode(inactiveDays)
        try values.encode(projectSlug)
    }
}

/// Eligible rows or links omitted by the requested scope or payload limits.
public struct CuratedGraphOmissions: Codable, Equatable, Sendable {
    public let nodeCount: UInt32
    public let linkCount: UInt32
    public let staleProjectCount: UInt32
    public let outsideRegistryCount: UInt32

    public init(from decoder: Decoder) throws {
        var values = try decoder.unkeyedContainer()
        nodeCount = try values.decode(UInt32.self)
        linkCount = try values.decode(UInt32.self)
        staleProjectCount = try values.decode(UInt32.self)
        outsideRegistryCount = try values.decode(UInt32.self)
        try requireGraphTupleEnd(values)
    }

    public func encode(to encoder: Encoder) throws {
        var values = encoder.unkeyedContainer()
        try values.encode(nodeCount)
        try values.encode(linkCount)
        try values.encode(staleProjectCount)
        try values.encode(outsideRegistryCount)
    }
}

private func requireGraphTupleEnd(_ values: UnkeyedDecodingContainer) throws {
    guard values.isAtEnd else {
        throw DecodingError.dataCorruptedError(
            in: values, debugDescription: "Unexpected graph tuple field")
    }
}
