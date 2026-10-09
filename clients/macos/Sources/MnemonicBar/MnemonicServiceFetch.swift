import Foundation
import MnemonicShared

extension MnemonicService {
    /// Only the selected secondary page asks for data. Each component tries
    /// HTTP first and shares the opening's one automatic command fallback.
    func fetchData(page: Int, commands: WidgetCommandAllowance, last: WidgetData) -> WidgetData? {
        guard [1, 2, 5].contains(page) else { return nil }
        var data = last
        var learned = false
        var usedAPI = false
        var usedCommand = false
        var checkedStatus = false
        var commandStatus: (running: Bool, hung: Bool)?

        // Check before other commands can spend the shared deadline, but
        // only after a failed HTTP read actually needs command fallback.
        func commandBudget() -> CommandBudget? {
            guard let budget = commands.budget() else { return nil }
            if !usedAPI, !checkedStatus {
                checkedStatus = true
                let status = runCommand(args: ["status"], budget: budget)
                if status.localizedCaseInsensitiveContains("hung") {
                    commandStatus = (false, true)
                } else if status.contains("is running") {
                    commandStatus = (true, false)
                } else if status.contains("is not running") || status.contains("has a stale PID file") {
                    commandStatus = (false, false)
                }
                if commandStatus != nil {
                    learned = true
                    usedCommand = true
                }
            }
            return budget
        }

        func object(_ path: String, _ args: [String]) -> [String: Any]? {
            if let object = httpObject(path) {
                usedAPI = true
                learned = true
                return object
            }
            guard let budget = commandBudget(),
                  let object = parseObject(runCommand(args: args, budget: budget))
            else { return nil }
            learned = true
            usedCommand = true
            return object
        }

        // Work, project charts, and Share use the same dated activity axis.
        if let summary = object("/api/activity/summary", ["activity", "summary"]) {
            applySummary(summary, to: &data)
            data.hasData = true
        }
        if page == 1 || page == 5 {
            if page == 1, let stats = httpObject("/api/status") {
                usedAPI = true
                learned = true
                data.memoriesTotal = Int(num(stats["total_memories"]))
            }
            if let memories = object("/api/memories?limit=12", ["recent", "-l", "12", "--json"]) {
                let items = (memories["results"] ?? memories["items"]) as? [[String: Any]]
                let total = memories["total"].map { value in Int(num(value)) }
                if memories["items"] != nil, let total { data.memoriesTotal = total }
                if let items { applyLatestMemory(items: items, totalFallback: total, to: &data) }
            }
        }
        if page == 2,
           let projects = object("/api/activity/projects", ["activity", "projects", "--json"]) {
            applyProjects(projects, to: &data)
        }
        if page == 1 {
            let recall = recallToday(commandBudget)
            data.recallToday = recall.state
            learned = learned || recall.source != nil || recall.state != last.recallToday
            if recall.source == 0 { usedAPI = true }
            if recall.source == 1 { usedCommand = true }
        }
        guard learned else { return nil }
        var hung = last.state == .broken
        if usedAPI {
            data.source = .api
            data.isRunning = true
            hung = false
        } else {
            if usedCommand { data.source = .command }
            if let status = commandStatus {
                data.isRunning = status.running
                hung = status.hung
            }
        }
        deriveState(hung: hung, for: &data)
        return data
    }

    /// Recall has a separate route on older daemons, but uses the same
    /// fallback deadline as the rest of the visible Work page.
    private func recallToday(_ budget: @escaping () -> CommandBudget?) -> RecallDayLoad {
        RecallDay.load([
            .route { self.httpReply("/api/recall") },
            .command {
                guard let budget = budget() else { return nil }
                return self.commandJSON(args: ["recall", "--json"], budget: budget)
            },
        ])
    }

    /// Legacy renderer compatibility. New fixture previews inject their data.
    func fetchDataNow() -> WidgetData {
        let commands = WidgetCommandAllowance(.explicit)
        let work = fetchData(page: 1, commands: commands, last: WidgetData()) ?? WidgetData()
        return fetchData(page: 2, commands: commands, last: work) ?? work
    }

    func applySummary(_ json: [String: Any], to d: inout WidgetData) {
        if let w = json["worked_today"] as? [String: Any] {
            d.workedTodaySeconds = num(w["seconds"])
        }
        d.inSession = (json["in_session"] as? Bool) ?? false
        d.sessionSeconds = numOpt(json["session_seconds"])

        if let wk = json["week"] as? [String: Any] {
            let total = num(wk["total_seconds"])
            let delta: Double? = (wk["delta_vs_avg_seconds"] is NSNull || wk["delta_vs_avg_seconds"] == nil)
                ? nil : num(wk["delta_vs_avg_seconds"])
            let best = wk["best_weekday"] as? String
            d.week = WeekStat(totalSeconds: total, deltaSeconds: delta, bestWeekday: best)
        }

        if let daysArr = json["days"] as? [[String: Any]] {
            d.days = daysArr.map { day in dayFromBar(day) }
        }
        if let today = json["today"] as? [String: Any] {
            d.today = dayFromDetail(today, isToday: true)
            // Merge today's detail into the matching chart day.
            if let t = d.today, let idx = d.days.firstIndex(where: { day in day.date == t.date }) {
                d.days[idx] = t
            }
        }
    }

    func applyLatestMemory(items: [[String: Any]], totalFallback: Int?, to d: inout WidgetData) {
        if d.memoriesTotal == 0, let totalFallback {
            d.memoriesTotal = totalFallback
        }
        // Full list for the Memory page (browse + filter + search).
        d.recent = items.map { item in memItem(item) }
        // Card on the Work page: first meaningful (skip session_summary and
        // what the daemon marked as noise).
        d.latest = MemoryPick.latest(of: items).map { item in memItem(item) }
    }

    func memItem(_ m: [String: Any]) -> LatestMemory {
        let title = m["title"] as? String ?? ""
        let ts = m["timestamp"] as? String ?? ""
        // Prefer the real memory id (stable). Fall back to timestamp+title.
        let id = (m["id"] as? String) ?? "\(ts)|\(title)"
        return LatestMemory(
            id: id,
            type: MemoryPick.memoryType(m),
            title: title,
            content: m["content"] as? String ?? "",
            agoMinutes: minutesAgo(ts)
        )
    }

    /// Apply one page of projects and their attributed time.
    func applyProjects(_ obj: [String: Any]?, to d: inout WidgetData) {
        guard let arr = obj?["projects"] as? [[String: Any]] else { return }
        if let u = obj?["unattributed"] as? [String: Any] {
            d.unattributed = UnattributedProjectTime(
                todaySeconds: num(u["today_seconds"]),
                weekSeconds: num(u["week_seconds"])
            )
        } else {
            d.unattributed = nil
        }
        d.projects = arr.map { p in
            let today = (p["today_seconds"] is NSNull) ? nil : numOpt(p["today_seconds"])
            let weekS = (p["week_seconds"] is NSNull) ? nil : numOpt(p["week_seconds"])
            let week = (p["week"] as? [Any])?.map { value in num(value) } ?? []
            let mems = (p["mems"] as? [[String: Any]] ?? []).map { item in memItem(item) }
            let confidence = p["confidence"] is NSNull ? nil : p["confidence"] as? String
            return Project(
                key: p["key"] as? String ?? (p["name"] as? String ?? ""),
                name: p["name"] as? String ?? "",
                todaySeconds: today,
                weekSeconds: weekS,
                week: week,
                memCount: Int(num(p["mem_count"])),
                tracking: (p["tracking"] as? Bool) ?? (weekS != nil),
                confidence: confidence,
                mems: mems
            )
        }
    }

    func numOpt(_ v: Any?) -> Double? {
        if v == nil || v is NSNull { return nil }
        return num(v)
    }

    func deriveState(hung: Bool, for d: inout WidgetData) {
        // Derive state. Check HUNG first: a hung daemon's `status` text
        // doesn't contain "is running", so the !isRunning branch would
        // otherwise mislabel it as Stopped (Codex caught this).
        if hung {
            d.state = .broken
        } else if !d.isRunning {
            d.state = .stopped
        } else if d.inSession {
            d.state = .working
        } else if d.workedTodaySeconds == 0 && (d.week?.totalSeconds ?? 0) == 0 {
            d.state = .empty
        } else {
            d.state = .idle
        }
    }

}
