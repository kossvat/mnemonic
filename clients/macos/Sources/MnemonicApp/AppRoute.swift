import Foundation
import MnemonicShared

/// A detail route carries its key; sidebar entries have no hidden selection.
enum AppRoute: Hashable, Identifiable, CaseIterable {
    case projects, project(String), timeline(String), overview, memories, search, entities, graph, memoryMap, manage

    static var allCases: [AppRoute] {
        [.projects, .overview, .memories, .search, .entities, .graph, .memoryMap, .manage,
         .timeline(TimelineDay.today())]
    }

    var id: String { rawValue }
    var rawValue: String {
        switch self {
        case .projects: "projects"
        case .project(let key): "project/" + key
        case .timeline(let day): "timeline/" + day
        case .overview: "overview"
        case .memories: "memories"
        case .search: "search"
        case .entities: "entities"
        case .graph: "graph"
        case .memoryMap: "memoryMap"
        case .manage: "manage"
        }
    }

    init?(rawValue: String) {
        if rawValue == "timeline" {
            self = .timeline(TimelineDay.today())
            return
        }
        if rawValue.hasPrefix("timeline/") {
            let day = String(rawValue.dropFirst("timeline/".count))
            guard TimelineDay.isValid(day) else { return nil }
            self = .timeline(day)
            return
        }
        guard let route = Self.allCases.first(where: { route in route.rawValue == rawValue }) else { return nil }
        self = route
    }

    var sidebarRoute: AppRoute {
        if case .project = self { return .projects }
        if case .timeline = self { return .timeline(TimelineDay.today()) }
        return self
    }

    var timelineDay: String? {
        if case .timeline(let day) = self { return day }
        return nil
    }

    var title: String {
        switch self {
        case .projects: "Projects"
        case .project: "Project"
        case .timeline: "Timeline"
        case .overview: "Overview"
        case .memories: "Memories"
        case .search: "Search"
        case .entities: "Entities"
        case .graph: "Project Graph"
        case .memoryMap: "Memory Map"
        case .manage: "Manage"
        }
    }

    var symbol: String {
        switch self {
        case .projects, .project: "square.grid.2x2"
        case .timeline: "chart.bar.xaxis"
        case .overview: "gauge.with.dots.needle.bottom.50percent"
        case .memories: "tray.full"
        case .search: "magnifyingglass"
        case .entities: "person.3.sequence"
        case .graph: "point.3.connected.trianglepath.dotted"
        case .memoryMap: "rectangle.connected.to.line.below"
        case .manage: "slider.horizontal.3"
        }
    }

    var refreshes: Bool { refreshes(at: Date()) }

    func refreshes(at now: Date) -> Bool {
        switch self {
        case .projects, .project, .overview: true
        case .timeline(let day): day == TimelineDay.today(now: now)
        default: false
        }
    }
}
