import Foundation
import MnemonicShared

/// The live-project count beside the icon, the count of memories after
/// it, and whether the connection needs attention. Legacy fixtures retain
/// their previous initializer.
///
/// It is built from the data and compared as a value, so the item is drawn
/// again only when what it says differs.
struct StatusTitle: Equatable {
    var label: String?
    var alert = false

    init() {}

    init(live: LiveSnapshot?, failure: LocalAPIFailure?) {
        label = live.map { snapshot in
            let projects = String(snapshot.projects.count + max(0, snapshot.moreProjects))
            guard let memories = snapshot.memories else { return projects }
            return "\(projects) · \(memories)"
        }
        alert = failure != nil && failure != .off
    }

    init(_ data: WidgetData) {
        var parts: [String] = []
        if data.memoriesTotal > 0 {
            parts.append("\(data.memoriesTotal)")
        }
        if data.workedTodaySeconds >= 60 {
            parts.append(fmtDur(data.workedTodaySeconds))
        }
        label = parts.isEmpty ? nil : parts.joined(separator: " · ")
        alert = data.state == .stopped || data.state == .broken
    }
}
