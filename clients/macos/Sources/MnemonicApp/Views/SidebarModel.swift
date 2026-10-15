import MnemonicShared

enum SidebarModel {
    struct Group {
        let label: String
        let entries: [AppRoute]
    }

    /// The owner's pick of 2026-10-14: the screens the mockup does not list
    /// stay under the group that will replace them.
    static func groups() -> [Group] {
        [Group(label: "NOW", entries: [.projects, .timeline(TimelineDay.today())]),
         Group(label: "MEMORY", entries: [.memories, .search, .entities, .memoryMap]),
         Group(label: "EXPLORE", entries: [.graph, .overview, .manage])]
    }

    static func daemonText(_ failure: LocalAPIFailure?) -> String {
        guard let failure else { return "daemon running" }
        switch failure {
        case .off: return "local API is off"
        case .unreachable: return "daemon unavailable"
        case .unauthorised: return "access unavailable"
        case .earlierDaemon: return "daemon update needed"
        case .undecodable: return "answer unreadable"
        }
    }
}
