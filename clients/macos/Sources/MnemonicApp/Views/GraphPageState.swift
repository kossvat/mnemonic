import MnemonicShared
import SwiftUI

/// Values and explicit actions only; rendering never creates a transport or clock.
struct GraphPageState {
    var snapshot: CuratedGraphSnapshot?
    var paintRevision: UInt64 = 0
    var loading = false
    var failure: CuratedGraphFailure?
    var expandedKey: String?
    var selectedKey: String?
    var project: ProjectDetail?
    var projectLoading = false
    var projectFailure: CuratedGraphFailure?
    var selectedEdge: GraphEdgeSelection?
    var evidence: [GraphEvidenceRow] = []
    var evidenceLoading = false
    var evidenceFailure: CuratedGraphFailure?
    var evidenceNextOffset: Int?
    var evidenceDiscardedCount = 0
    var outside = GraphArchiveListState()
    var history = GraphArchiveListState()
    var mutationInFlight = false
    var message: String?
    var canUndo = false
    var canRetryMutation = false
    var recovery: GraphMutationRecovery?

    var selectedNode: CuratedGraphNode? {
        snapshot?.nodes.first { node in node.key == selectedKey }
    }
    var selectedShelf: CuratedGraphShelfItem? {
        snapshot?.shelf.first { item in item.key == selectedKey }
    }
    var selectedProjectSlug: String? { selectedNode?.projectSlug ?? selectedShelf?.projectSlug }
    var staleProjects: [CuratedGraphShelfItem] {
        snapshot?.shelf.filter { item in item.reason == .staleProject } ?? []
    }
    var outsideShelf: [CuratedGraphShelfItem] {
        snapshot?.shelf.filter { item in item.reason == .outsideRegistry } ?? []
    }
    var shelfNotice: String? {
        if loading { return "Loading this list with the graph." }
        if failure != nil || snapshot == nil { return "This list is unavailable. Refresh the graph to load it." }
        return nil
    }
    var staleProjectCount: Int? { shelfNotice == nil ? staleProjects.count : nil }
    var outsideEntityCount: Int? {
        if outside.isLoaded { return outside.total ?? outside.entities.count }
        return shelfNotice == nil ? outsideShelf.count : nil
    }
}

struct GraphPageActions {
    var refresh: () -> Void = {}
    var selectNode: (String?) -> Void = { _ in }
    var expand: (String?) -> Void = { _ in }
    var selectEdge: (Int) -> Void = { _ in }
    var moreEvidence: () -> Void = {}
    var outside: (Bool) -> Void = { _ in }
    var history: (Bool) -> Void = { _ in }
    var archive: (GraphArchiveTarget, Bool) -> Void = { _, _ in }
    var retry: () -> Void = {}
    var undo: () -> Void = {}
    var openProject: (String) -> Void = { _ in }
}

/// Formatting depends on the received values, never on a repeating date source.
enum GraphPresentation {
    static func hours(_ tenths: UInt32?) -> String {
        guard let tenths else { return "Unavailable" }
        return String(format: "%.1f h", locale: Locale(identifier: "en_US_POSIX"), Double(tenths) / 10)
    }

    static func activity(_ days: UInt32?) -> String {
        guard let days else { return "Activity unavailable" }
        if days == 0 { return "Activity today" }
        return "\(days) \(days == 1 ? "day" : "days") since activity"
    }

    static func date(_ stamp: Int64?) -> String {
        guard let stamp else { return "Activity unavailable" }
        return Date(timeIntervalSince1970: Double(stamp)).formatted(date: .abbreviated, time: .omitted)
    }

    static func kind(_ value: CuratedGraphNodeKind) -> String {
        switch value {
        case .project: return "Project"
        case .part: return "Part"
        case .unit: return "Plan unit"
        case .technology: return "Technology"
        case .person: return "Person"
        }
    }

    static func color(_ value: CuratedGraphNodeKind) -> Color {
        switch value {
        case .project: return Theme.Palette.entityProject
        case .part: return Theme.Palette.entityModule
        case .unit: return Theme.Palette.entityConcept
        case .technology: return Theme.Palette.entityTech
        case .person: return Theme.Palette.entityPerson
        }
    }

    static func symbol(_ value: CuratedGraphNodeKind) -> String {
        switch value {
        case .project: return "circle.fill"
        case .part: return "diamond.fill"
        case .unit: return "square.fill"
        case .technology: return "hexagon.fill"
        case .person: return "circle.circle"
        }
    }

    static func state(_ value: CuratedGraphNodeState) -> String {
        switch value {
        case .active: return "Active"
        case .quiet: return "Quiet"
        case .stale: return "Stale"
        case .archived: return "Archived"
        }
    }

    static func relationship(_ value: CuratedGraphLinkKind) -> String {
        switch value {
        case .contains: return "Contains"
        case .uses: return "Uses"
        case .involves: return "Involves"
        case .relates: return "Related to"
        }
    }

    static func omissions(_ value: CuratedGraphOmissions) -> String? {
        let counts = [(value.nodeCount, "node", "nodes"), (value.linkCount, "link", "links"),
                      (value.staleProjectCount, "stale project", "stale projects"),
                      (value.outsideRegistryCount, "outside entity", "outside entities")]
        let parts = counts.filter { item in item.0 > 0 }.map { count, singular, plural in
            "\(count) \(count == 1 ? singular : plural)"
        }
        return parts.isEmpty ? nil : "Outside this view: " + parts.joined(separator: " · ")
    }

    static func entityID(_ key: String) -> String? {
        guard key.hasPrefix("entity:"), key.count > "entity:".count else { return nil }
        return String(key.dropFirst("entity:".count))
    }
}
