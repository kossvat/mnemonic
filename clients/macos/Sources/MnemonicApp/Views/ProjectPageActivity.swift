import Foundation
import MnemonicShared

extension ProjectPresentation {
    struct PageSession: Identifiable {
        let id: Int
        let kind: String
        let agent: String
        let state: String
        let elapsed: String
        let lastRecord: String?
        let driver: String
    }

    struct PageHistory: Identifiable {
        let id: String
        let title: String
        let kind: String?
    }

    struct PageDay: Identifiable {
        let id: String
        let label: String
        let times: BoardTimes?
        let history: [PageHistory]
    }

    struct PageRecent {
        let days: [PageDay]
        let timeMessage: String?
        let historyMessage: String?
        var columnCount: Int { 3 }
        var historyLineLimit: Int { 2 }
    }

    static func pageSession(_ session: LiveSession, index: Int, now: Date, calendar: Calendar) -> PageSession {
        let state: String
        switch session.state {
        case .running: state = "Running"
        case .waiting: state = "Waiting for you"
        case .quiet: state = "Quiet"
        }
        let lastRecord: String?
        switch session.lastRecordAt {
        case .missing: lastRecord = nil
        case .null: lastRecord = "No last record time recorded"
        case .value(let at): lastRecord = "last record " + recordAge(at, now: now) + " ago"
        }
        let driver: String
        switch session.driver {
        case "owner": driver = "Owner-led session"
        case "program": driver = "Automated session"
        default: driver = session.driver
        }
        return PageSession(id: index, kind: session.agent, agent: AgentCount(kind: session.agent, count: 1).name,
                           state: state, elapsed: duration(max(0, now.timeIntervalSince1970 - Double(session.startedAt))),
                           lastRecord: lastRecord, driver: driver)
    }

    static func recordAge(_ at: Int, now: Date) -> String {
        let minutes = Int(max(0, now.timeIntervalSince1970 - Double(at)) / 60)
        if minutes == 0 { return "under 1 min" }
        if minutes < 60 { return "\(minutes) min" }
        if minutes < 1_440 { return "\(minutes / 60) hr" }
        let days = minutes / 1_440
        return "\(days) \(days == 1 ? "day" : "days")"
    }

    static func pageTimes(_ day: ProjectDay, estimated: Bool) -> BoardTimes {
        BoardTimes(owner: day.ownerSeconds == nil ? "Unavailable" : ownerTime(day.ownerSeconds, estimated: estimated),
                   agents: duration(Double(day.agentSeconds)))
    }

    static func pageHistory(_ entry: ProjectHistoryEntry) -> PageHistory {
        let kind: String?
        switch entry.kind {
        case .missing: kind = nil
        case .null: kind = "Kind not recorded"
        case .value("decision"): kind = "Decision"
        case .value("correction"): kind = "Correction"
        case .value(let value): kind = value
        }
        return PageHistory(id: entry.id, title: entry.title, kind: kind)
    }

    static func pageRecent(_ detail: ProjectDetail, calendar: Calendar) -> PageRecent {
        let history = Dictionary(grouping: detail.history) { entry in
            TimelineDay.string(Date(timeIntervalSince1970: Double(entry.at)), calendar: calendar)
        }
        let time = Dictionary(detail.time.map { day in (day.day, day) }, uniquingKeysWith: { first, _ in first })
        let days = Set(time.keys).union(history.keys).sorted(by: >).map { day in
            let entries = (history[day] ?? []).sorted { left, right in
                left.at == right.at ? left.id < right.id : left.at > right.at
            }
            let label = TimelineDay.date(day, calendar: calendar).map { date in
                boardDate(date, calendar: calendar, format: "EEE, d MMM yyyy")
            } ?? day
            return PageDay(id: day, label: label,
                           times: time[day].map { value in pageTimes(value, estimated: detail.ownerEstimate) },
                           history: entries.map(pageHistory))
        }
        // An empty array carries no kind field and cannot prove support for corrections.
        return PageRecent(days: days, timeMessage: detail.time.isEmpty ? "No daily time recorded" : nil,
                          historyMessage: detail.history.isEmpty ? "The returned history holds no decision or correction." : nil)
    }
}
