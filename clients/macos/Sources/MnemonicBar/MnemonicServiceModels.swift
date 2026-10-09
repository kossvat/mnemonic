import Foundation
import MnemonicShared

// MARK: - Models (v2 widget)

// What the views observe is compared as a value: it is assigned only when
// it differs, so every model here is equatable and has a stable identity.

enum WorkState {
    case working, idle, stopped, broken, empty
}

/// A work block within a day, in local minutes-from-midnight (for the
/// 6a–10p timeline lane).
struct BlockMin: Identifiable, Equatable {
    var id: String { "\(startMin)-\(endMin)" }
    let startMin: Double
    let endMin: Double
}

/// One day in the history chart, optionally with full detail filled in.
struct WorkDay: Identifiable, Equatable {
    var id: String { date }   // stable across refreshes so SwiftUI diffs (no rebuild)
    let date: String          // YYYY-MM-DD
    let seconds: Double
    let isToday: Bool
    let dowLetter: String     // S M T W T F S
    let dayNum: Int           // 1..31 (for month weekly ticks)
    let dow: Int              // 0=Sun
    let label: String         // "Tue, May 26"

    // Detail (filled for today from summary, or lazily on tap)
    var detailLoaded: Bool = false
    var sessions: [BlockMin] = []
    var sessionCount: Int = 0
    var longestSeconds: Double = 0
    var spanHuman: String? = nil

    var minutes: Double { seconds / 60.0 }
}

struct WeekStat: Equatable {
    let totalSeconds: Double
    let deltaSeconds: Double?   // vs avg; nil when no history
    let bestWeekday: String?
}

struct LatestMemory: Identifiable, Equatable {
    let id: String             // real memory id → stable across refreshes
    let type: String           // decision / feedback / note / session_summary
    let title: String
    let content: String
    let agoMinutes: Int
}

/// One project on the Projects page. `tracking == false` (or nil seconds)
/// means the backend hasn't attributed time to it yet → "tracking soon".
struct Project: Identifiable, Equatable {
    var id: String { key }     // stable
    let key: String
    let name: String
    let todaySeconds: Double?
    let weekSeconds: Double?
    let week: [Double]          // 7 daily seconds (may be empty)
    let memCount: Int
    let tracking: Bool
    let confidence: String?
    var mems: [LatestMemory] = []
}

struct UnattributedProjectTime: Equatable {
    let todaySeconds: Double
    let weekSeconds: Double
}

/// Everything the popover's pages need.
struct WidgetData: Equatable {
    /// Where the data came from: the local API, or the command line when
    /// the API gave nothing.
    enum Source { case none, api, command }

    var source: Source = .none
    var isRunning: Bool = false
    var state: WorkState = .stopped
    var workedTodaySeconds: Double = 0
    var inSession: Bool = false
    var sessionSeconds: Double? = nil
    var week: WeekStat? = nil
    var days: [WorkDay] = []        // last 7
    var today: WorkDay? = nil       // today's detail (with blocks)
    var latest: LatestMemory? = nil
    var recent: [LatestMemory] = [] // full list for the Memory page
    var projects: [Project] = []    // graph projects, with time when attributed
    var unattributed: UnattributedProjectTime? = nil
    var memoriesTotal: Int = 0
    var hasData: Bool = false
    /// The card of memory today: what memory returned on this local day.
    var recallToday: RecallDayState = .unavailable
}

