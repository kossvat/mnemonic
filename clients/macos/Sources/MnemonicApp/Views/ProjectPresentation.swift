import Foundation
import SwiftUI
import MnemonicShared

/// Display decisions shared by the board and detail, without data work.
enum ProjectPresentation {
    enum Served<Value> {
        case notServed, absent, value(Value)
    }

    static func served<Element>(_ presence: FieldPresence<[Element]>) -> Served<[Element]> {
        switch presence {
        case .missing: return .notServed
        case .null: return .absent
        case .value(let values): return values.isEmpty ? .absent : .value(values)
        }
    }

    static func focusUnit(_ units: [ProjectPlanUnit]) -> ProjectPlanUnit? {
        units.first { unit in unit.currentTask != nil } ?? units.first
    }

    struct LiveCard: Identifiable {
        let project: LiveProject
        let row: ProjectBoardRow?
        var id: String { project.key }
    }

    struct Groups {
        let live: [LiveCard]
        let recent: [ProjectBoardRow]
        let stale: [ProjectBoardRow]
        let archived: Int
        let ignored: Int
        let merged: Int
        var excluded: Int { archived + ignored + merged }
        var excludedLabel: String {
            [(archived, "archived"), (ignored, "ignored"), (merged, "merged")]
                .filter { count, _ in count > 0 }
                .map { count, kind in "\(count) \(kind)" }.joined(separator: " · ")
        }
    }

    static func groups(board: ProjectBoard?, live: LiveSnapshot?, now: Date) -> Groups {
        let rows = board?.projects ?? []
        let byKey = Dictionary(rows.map { row in (row.key, row) }, uniquingKeysWith: { first, _ in first })
        let cards = (live?.projects ?? []).filter { project in
            guard let row = byKey[project.key] else { return true }
            return row.state == .active && row.mergedInto == nil
        }.map { project in LiveCard(project: project, row: byKey[project.key]) }
        let liveKeys = Set(cards.map { card in card.id })
        let other = rows.filter { row in
            row.state == .active && row.mergedInto == nil && !liveKeys.contains(row.key)
        }.sorted { left, right in
            let leftTime = left.lastActiveAt ?? Int.min
            let rightTime = right.lastActiveAt ?? Int.min
            return leftTime == rightTime ? left.key < right.key : leftTime > rightTime
        }
        let cutoff = now.timeIntervalSince1970 - 30 * 24 * 60 * 60
        func stale(_ row: ProjectBoardRow) -> Bool {
            guard let at = row.lastActiveAt else { return false }
            return Double(at) < cutoff
        }
        return Groups(
            live: cards, recent: other.filter { row in !stale(row) },
            stale: other.filter { row in stale(row) },
            archived: rows.filter { row in row.mergedInto == nil && row.state == .archived }.count,
            ignored: rows.filter { row in row.mergedInto == nil && row.state == .ignored }.count,
            merged: rows.filter { row in row.mergedInto != nil }.count)
    }

    static func duration(_ seconds: Double?) -> String {
        guard let seconds, seconds.isFinite, seconds >= 0, seconds < Double(Int.max / 2) else {
            return "Unknown"
        }
        let minutes = Int(seconds / 60)
        if seconds > 0 && minutes == 0 { return "<1m" }
        if minutes < 60 { return "\(minutes)m" }
        let rest = minutes % 60
        return rest == 0 ? "\(minutes / 60)h" : "\(minutes / 60)h \(rest)m"
    }

    static func ownerTime(_ seconds: Double?, estimated: Bool) -> String {
        let value = duration(seconds)
        return value == "Unknown" || !estimated ? value : "~\(value)"
    }

    static func activity(_ at: Int?, now: Date) -> String {
        guard let at else { return "No activity recorded" }
        let elapsed = max(0, now.timeIntervalSince1970 - Double(at))
        if elapsed < 60 { return "Just now" }
        if elapsed < 3_600 { return "\(Int(elapsed / 60))m ago" }
        if elapsed < 86_400 { return "\(Int(elapsed / 3_600))h ago" }
        return "\(Int(elapsed / 86_400))d ago"
    }

    static func date(_ at: Int) -> String {
        Date(timeIntervalSince1970: Double(at)).formatted(date: .abbreviated, time: .shortened)
    }

    static func title(_ value: String) -> String {
        value.replacingOccurrences(of: "_", with: " ").capitalized
    }

    static func agent(_ value: String) -> String {
        switch value {
        case "codex": return "Codex"
        case "claude": return "Claude"
        default: return title(value)
        }
    }

    static func agentIcon(_ value: String) -> String {
        value == "codex" ? "terminal" : value == "claude" ? "sparkle" : "app.dashed"
    }

    static func planLine(_ plan: LivePlan?, message: String? = nil) -> String {
        guard let plan else { return message ?? "No plan recorded" }
        let count = plan.total > 0 ? " · \(plan.done)/\(plan.total) tasks" : ""
        return "\(plan.unit)\(count) · \(title(plan.stage))"
    }

    static func completion(_ unit: ProjectPlanUnit) -> Double? {
        unit.completionFraction.map { fraction in min(1, max(0, fraction)) }
    }

    static func progressLabel(_ unit: ProjectPlanUnit) -> String {
        if unit.provider == .funnel { return "\(unit.weeklyCount) this week" }
        if unit.total == 0 { return "No task count recorded" }
        return "\(unit.done) of \(unit.total) tasks done"
    }

    static func currentSource(_ source: String) -> String {
        switch source {
        case "registry": return "Task registry"
        case "branch": return "Session branch"
        case "first_open": return "First open task"
        default: return title(source)
        }
    }

    static func gate(_ status: ProjectGateStatus) -> String {
        switch status {
        case .passed: return "Passed"
        case .failed: return "Failed"
        case .stale: return "Out of date"
        case .none: return "No gate receipt"
        }
    }

    static func checkout(_ detail: ProjectGateDetail?) -> String {
        guard let dirty = detail?.dirty else { return "Working tree unknown" }
        return dirty ? "Uncommitted changes" : "No uncommitted changes"
    }

    static func commits(_ detail: ProjectGateDetail?) -> String {
        guard let count = detail?.commitsSince else { return "Commits since gate unknown" }
        return "\(count) \(count == 1 ? "commit" : "commits") since gate"
    }

    static func failure(_ value: LocalAPIFailure) -> (title: String, detail: String) {
        switch value {
        case .off:
            return ("Local API is off", "Project data is unavailable while this profile's API is disabled.")
        case .unreachable:
            return ("Daemon unavailable", "The local daemon did not answer. Available data may be out of date.")
        case .unauthorised:
            return ("Access unavailable", "The daemon did not accept this profile's access token.")
        case .earlierDaemon:
            return ("Daemon update needed", "This daemon does not provide the requested project data.")
        case .undecodable:
            return ("Project data unavailable", "The daemon's answer could not be read. Available data may be out of date.")
        }
    }
}

struct ProjectPill: View {
    let text: String
    var color: Color = Theme.Palette.textMuted
    var body: some View {
        Text(verbatim: text).font(Theme.Font.captionBold)
            .foregroundStyle(color).padding(.horizontal, 8).padding(.vertical, 4)
            .background(Capsule().fill(color.opacity(0.09)))
    }
}

struct ProjectNotice: View {
    let icon: String
    let title: String
    let detail: String
    var body: some View {
        HStack(alignment: .top, spacing: Theme.Space.md) {
            Image(systemName: icon).font(.system(size: 18)).foregroundStyle(Theme.Palette.textMuted)
                .frame(width: 24).padding(.top, 2)
            VStack(alignment: .leading, spacing: 5) {
                Text(verbatim: title).font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary)
                Text(verbatim: detail).font(Theme.Font.body).foregroundStyle(Theme.Palette.textMuted)
                    .fixedSize(horizontal: false, vertical: true)
            }
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }
}
