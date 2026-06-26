import Foundation
import AppKit

// MARK: - Models (v2 widget)

enum WorkState {
    case working, idle, stopped, broken, empty
}

/// A work block within a day, in local minutes-from-midnight (for the
/// 6a–10p timeline lane).
struct BlockMin: Identifiable {
    let id = UUID()
    let startMin: Double
    let endMin: Double
}

/// One day in the history chart, optionally with full detail filled in.
struct WorkDay: Identifiable {
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

struct WeekStat {
    let totalSeconds: Double
    let deltaSeconds: Double?   // vs avg; nil when no history
    let bestWeekday: String?
}

struct LatestMemory: Identifiable {
    let id: String             // real memory id → stable across refreshes
    let type: String           // decision / feedback / note / session_summary
    let title: String
    let content: String
    let agoMinutes: Int
}

/// One project on the Projects page. `tracking == false` (or nil seconds)
/// means the backend hasn't attributed time to it yet → "tracking soon".
struct Project: Identifiable {
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

struct UnattributedProjectTime {
    let todaySeconds: Double
    let weekSeconds: Double
}

/// Everything the popover's pages need.
struct WidgetData {
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
}

// MARK: - Service

class MnemonicService: ObservableObject {
    @Published var data = WidgetData()
    @Published var lastUpdate = Date()

    /// Lazily-fetched 30-day series for the Month chart (cached per refresh).
    @Published var monthDays: [WorkDay] = []

    private let mnemonicPath: String
    private var timer: Timer?

    init() {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        self.mnemonicPath = "\(home)/.cargo/bin/mnemonic"
    }

    func startPolling(interval: TimeInterval = 10) {
        stopPolling() // re-arming with a new cadence must not leak the old timer
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: interval, repeats: true) { [weak self] _ in
            self?.refresh()
        }
    }

    /// Popover-aware cadence: 10s only while the user is actually looking,
    /// 60s in the background (the menu-bar time only needs minute
    /// precision). The old fixed 10s cadence fired 3 HTTP calls — or CLI
    /// process spawns on a dead API — every 10 seconds, 24/7.
    func setPollingForeground(_ foreground: Bool) {
        startPolling(interval: foreground ? 10 : 60)
    }

    func stopPolling() {
        timer?.invalidate()
        timer = nil
    }

    func refresh() {
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self = self else { return }
            let d = self.fetchData()
            DispatchQueue.main.async {
                self.data = d
                self.lastUpdate = Date()
                // Refresh the month series in place. Blanking it (the old
                // behavior) emptied the Month chart mid-view on every poll.
                self.reloadMonthIfLoaded()
            }
        }
    }

    // MARK: Fetch

    private func fetchData() -> WidgetData {
        fetchDataHTTP() ?? fetchDataCLI()
    }

    /// Synchronous fetch for the preview renderer — same path the polling
    /// refresh uses, just without the background hop.
    func fetchDataNow() -> WidgetData {
        fetchData()
    }

    /// Preferred read path for the menu-bar widget. The daemon already keeps
    /// the HTTP server warm, so normal refreshes avoid spawning 3 CLI processes
    /// every 10 seconds. If the API/token is unavailable, fall back to CLI so
    /// stopped/broken states remain visible.
    private func fetchDataHTTP() -> WidgetData? {
        var d = WidgetData()

        guard let summary = httpObject("/api/activity/summary") else {
            return nil
        }

        d.isRunning = true
        d.hasData = true
        applySummary(summary, to: &d)

        if let stats = httpObject("/api/status") {
            d.memoriesTotal = Int(num(stats["total_memories"]))
        }

        if let memories = httpObject("/api/memories?limit=12"),
           let items = memories["results"] as? [[String: Any]] {
            applyLatestMemory(items: items, totalFallback: nil, to: &d)
        }

        applyProjects(http: true, to: &d)
        deriveState(hung: false, for: &d)
        return d
    }

    private func fetchDataCLI() -> WidgetData {
        var d = WidgetData()

        // Daemon status
        let status = runCommand(args: ["status"])
        d.isRunning = status.contains("is running")
        let hung = status.localizedCaseInsensitiveContains("hung")

        // Activity summary (worked today, session, week, today detail, days)
        if let json = parseObject(runCommand(args: ["activity", "summary"])) {
            d.hasData = true
            applySummary(json, to: &d)
        }

        // Latest *meaningful* memory. Pull a handful and pick the first
        // decision/feedback/note — skipping session_summary (dream output)
        // and obvious file/build-event noise — so the card shows something
        // worth reading, not "Session summary: …" or a raw file event.
        if let json = parseObject(runCommand(args: ["recent", "-l", "12", "--json"])) {
            let total = Int(num(json["total"]))
            d.memoriesTotal = total
            if let items = json["items"] as? [[String: Any]] {
                applyLatestMemory(items: items, totalFallback: total, to: &d)
            }
        }

        applyProjects(http: false, to: &d)
        deriveState(hung: hung, for: &d)
        return d
    }

    private func applySummary(_ json: [String: Any], to d: inout WidgetData) {
        if let w = json["worked_today"] as? [String: Any] {
            d.workedTodaySeconds = num(w["seconds"])
        }
        d.inSession = (json["in_session"] as? Bool) ?? false
        if let s = json["session_seconds"], !(s is NSNull) { d.sessionSeconds = num(s) }

        if let wk = json["week"] as? [String: Any] {
            let total = num(wk["total_seconds"])
            let delta: Double? = (wk["delta_vs_avg_seconds"] is NSNull || wk["delta_vs_avg_seconds"] == nil)
                ? nil : num(wk["delta_vs_avg_seconds"])
            let best = wk["best_weekday"] as? String
            d.week = WeekStat(totalSeconds: total, deltaSeconds: delta, bestWeekday: best)
        }

        if let daysArr = json["days"] as? [[String: Any]] {
            d.days = daysArr.map { dayFromBar($0) }
        }
        if let today = json["today"] as? [String: Any] {
            d.today = dayFromDetail(today, isToday: true)
            // Merge today's detail into the matching chart day.
            if let t = d.today, let idx = d.days.firstIndex(where: { $0.date == t.date }) {
                d.days[idx] = t
            }
        }
    }

    private func applyLatestMemory(items: [[String: Any]], totalFallback: Int?, to d: inout WidgetData) {
        if d.memoriesTotal == 0, let totalFallback {
            d.memoriesTotal = totalFallback
        }
        // Full list for the Memory page (browse + filter + search).
        d.recent = items.map { memItem($0) }
        // Card on the Work page: first meaningful (skip session_summary/noise).
        let pick = items.first(where: { isMeaningful($0) }) ?? items.first
        if let m = pick { d.latest = memItem(m) }
    }

    private func memItem(_ m: [String: Any]) -> LatestMemory {
        let title = m["title"] as? String ?? ""
        let ts = m["timestamp"] as? String ?? ""
        // Prefer the real memory id (stable). Fall back to timestamp+title.
        let id = (m["id"] as? String) ?? "\(ts)|\(title)"
        return LatestMemory(
            id: id,
            type: memoryType(m),
            title: title,
            content: m["content"] as? String ?? "",
            agoMinutes: minutesAgo(ts)
        )
    }

    /// Projects with optional attributed time. Older backends still return
    /// graph projects with null time, so the UI keeps the "tracking soon" state.
    private func applyProjects(http: Bool, to d: inout WidgetData) {
        // Both HTTP and CLI now return an object {projects:[...]} — parsing
        // by stripping to the first `{` is robust against tracing/ANSI log
        // lines (a bare `[` array could collide with the `[2m` ANSI prefix).
        let obj: [String: Any]? = http
            ? httpObject("/api/activity/projects")
            : parseObject(runCommand(args: ["activity", "projects", "--json"]))
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
            let week = (p["week"] as? [Any])?.map { num($0) } ?? []
            let mems = (p["mems"] as? [[String: Any]] ?? []).map { memItem($0) }
            let confidence = p["confidence"] is NSNull ? nil : p["confidence"] as? String
            return Project(
                key: p["key"] as? String ?? (p["name"] as? String ?? UUID().uuidString),
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

    private func numOpt(_ v: Any?) -> Double? {
        if v == nil || v is NSNull { return nil }
        return num(v)
    }

    private func deriveState(hung: Bool, for d: inout WidgetData) {
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

    /// Fetch the 30-day series for the Month chart (called lazily when the
    /// user switches to Month). Result is published on `monthDays`.
    func loadMonthIfNeeded() {
        if !monthDays.isEmpty { return }
        fetchMonth { [weak self] out in self?.monthDays = out }
    }

    /// Background re-fetch for an already-loaded month series. Only swaps
    /// the published value on success so a transient API hiccup doesn't
    /// blank a chart the user is looking at.
    private func reloadMonthIfLoaded() {
        guard !monthDays.isEmpty else { return }
        fetchMonth { [weak self] out in
            if !out.isEmpty { self?.monthDays = out }
        }
    }

    private func fetchMonth(_ deliver: @escaping ([WorkDay]) -> Void) {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }
            var out: [WorkDay] = []
            if let json = self.httpObject("/api/activity/week?days=30"),
               let days = json["days"] as? [[String: Any]] {
                out = days.map { self.dayFromBar($0) }
            } else if let arr = parseArray(self.runCommand(args: ["activity", "week", "--days", "30", "--json"])) {
                out = arr.map { self.dayFromBar($0) }
            }
            DispatchQueue.main.async { deliver(out) }
        }
    }

    /// Fetch full detail for a specific day (on tap of a non-today bar).
    /// Calls back on the main queue with the enriched day.
    func loadDayDetail(date: String, completion: @escaping (WorkDay?) -> Void) {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self = self else { return }
            let out = self.dayDetailNow(date: date)
            DispatchQueue.main.async { completion(out) }
        }
    }

    /// Synchronous day fetch. `httpObject` blocks on a background-queue
    /// completion so it's safe to call from any thread; the preview
    /// renderer uses this instead of `loadDayDetail` (whose main-queue
    /// completion would deadlock a synchronous wait on the main thread).
    func dayDetailNow(date: String) -> WorkDay? {
        let json = httpObject("/api/activity/day?date=\(date)")
            ?? parseObject(runCommand(args: ["activity", "day", "--date", date]))
        return json.map { dayFromDetail($0, isToday: false) }
    }

    // MARK: Parsing helpers

    private func dayFromBar(_ j: [String: Any]) -> WorkDay {
        let date = j["date"] as? String ?? ""
        let secs = num(j["seconds"])
        return makeDay(date: date, seconds: secs, isToday: isTodayStr(date))
    }

    private func dayFromDetail(_ j: [String: Any], isToday: Bool) -> WorkDay {
        let date = j["date"] as? String ?? ""
        let secs = num(j["total_seconds"])
        var day = makeDay(date: date, seconds: secs, isToday: isToday || isTodayStr(date))
        day.detailLoaded = true
        day.sessionCount = Int(num(j["sessions"]))
        day.longestSeconds = num(j["longest_seconds"])
        day.spanHuman = j["span_human"] as? String
        if let blocks = j["blocks"] as? [[String: Any]] {
            day.sessions = blocks.compactMap { b in
                guard let s = b["start"] as? String, let e = b["end"] as? String,
                      let sm = localMinutes(s), let em = localMinutes(e) else { return nil }
                return BlockMin(startMin: sm, endMin: em)
            }
        }
        return day
    }

    private func makeDay(date: String, seconds: Double, isToday: Bool) -> WorkDay {
        let comps = parseYMD(date)
        let dow = comps?.weekdayIndex ?? 0
        let letters = ["S", "M", "T", "W", "T", "F", "S"]
        return WorkDay(
            date: date,
            seconds: seconds,
            isToday: isToday,
            dowLetter: letters[dow],
            dayNum: comps?.day ?? 1,
            dow: dow,
            label: comps?.label ?? date
        )
    }

    // MARK: Date utils

    private func isTodayStr(_ s: String) -> Bool {
        let f = DateFormatter(); f.dateFormat = "yyyy-MM-dd"
        return s == f.string(from: Date())
    }

    private struct YMD { let day: Int; let weekdayIndex: Int; let label: String }
    private func parseYMD(_ s: String) -> YMD? {
        let f = DateFormatter(); f.dateFormat = "yyyy-MM-dd"
        guard let d = f.date(from: s) else { return nil }
        let cal = Calendar.current
        let day = cal.component(.day, from: d)
        let wd = cal.component(.weekday, from: d) - 1 // 0=Sun
        let lf = DateFormatter(); lf.dateFormat = "EEE, MMM d"
        return YMD(day: day, weekdayIndex: wd, label: lf.string(from: d))
    }

    /// Is this memory worth surfacing on the card? Decisions and feedback
    /// always qualify; notes qualify unless they read like a file/build
    /// event. Session summaries (dream output) never qualify.
    private func isMeaningful(_ m: [String: Any]) -> Bool {
        let type = memoryType(m).lowercased()
        if type == "decision" || type == "feedback" { return true }
        if type != "note" { return false } // session_summary, etc.
        let hay = ((m["title"] as? String ?? "") + " " + (m["content"] as? String ?? "")).lowercased()
        let noise = [".build/", "node_modules", "new file:", "deleted:", "untracked",
                     "modified file", "file changed", "build complete", "target/"]
        return !noise.contains { hay.contains($0) }
    }

    private func memoryType(_ m: [String: Any]) -> String {
        (m["type"] as? String) ?? (m["memory_type"] as? String) ?? "note"
    }

    /// RFC3339 → local minutes-from-midnight.
    private func localMinutes(_ iso: String) -> Double? {
        guard let d = parseISO(iso) else { return nil }
        let cal = Calendar.current
        let h = cal.component(.hour, from: d)
        let m = cal.component(.minute, from: d)
        return Double(h * 60 + m)
    }

    private func minutesAgo(_ iso: String) -> Int {
        guard let d = parseISO(iso) else { return 0 }
        return max(0, Int(Date().timeIntervalSince(d) / 60.0))
    }

    private func parseISO(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f.date(from: s) { return d }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: s)
    }

    private func num(_ v: Any?) -> Double {
        if let n = v as? NSNumber { return n.doubleValue }
        if let s = v as? String { return Double(s) ?? 0 }
        return 0
    }

    /// Strip any non-JSON prefix (tracing log lines) then parse an object.
    private func parseObject(_ out: String) -> [String: Any]? {
        guard let i = out.firstIndex(of: "{"),
              let data = String(out[i...]).data(using: .utf8),
              let j = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return j
    }

    private func parseArray(_ out: String) -> [[String: Any]]? {
        guard let i = out.firstIndex(of: "["),
              let data = String(out[i...]).data(using: .utf8),
              let j = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]]
        else { return nil }
        return j
    }

    // MARK: - Actions

    func addMemory(title: String, type: String) {
        runActionAsync(["save", "-t", title, title, "-T", type])
    }

    func startDaemon() { runActionAsync(["start", "-d"]) }
    func stopDaemon() { runActionAsync(["stop"]) }

    func openLog() {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        NSWorkspace.shared.open(URL(fileURLWithPath: "\(home)/.mnemonic/daemon.log"))
    }

    func generateContext() { runActionAsync(["context"]) }

    /// Fire-and-forget CLI action off the main thread. These ran
    /// synchronously on main before — `stop` alone blocks up to 5s in the
    /// daemon's SIGTERM poll, freezing the whole popover. Refreshes widget
    /// data once the command finishes so state flips without waiting for
    /// the next poll tick.
    private func runActionAsync(_ args: [String]) {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            _ = self.runCommand(args: args)
            self.refresh()
        }
    }

    /// Launch the full dashboard app (MnemonicApp target), reusing a running
    /// instance if present, else a packaged bundle, else the built binary.
    func openDashboard() {
        let running = NSWorkspace.shared.runningApplications.first { app in
            app.bundleIdentifier == "com.kossvat.mnemonic.app"
                || app.executableURL?.lastPathComponent == "MnemonicApp"
        }
        if let app = running {
            app.activate(options: [.activateIgnoringOtherApps])
            return
        }
        let home = FileManager.default.homeDirectoryForCurrentUser
        let bundles = [
            home.appendingPathComponent("Applications/Mnemonic.app"),
            URL(fileURLWithPath: "/Applications/Mnemonic.app"),
        ]
        if let appURL = bundles.first(where: { FileManager.default.fileExists(atPath: $0.path) }) {
            let cfg = NSWorkspace.OpenConfiguration(); cfg.activates = true
            NSWorkspace.shared.openApplication(at: appURL, configuration: cfg) { _, _ in }
            return
        }
        let sourceDir: URL = {
            if let env = ProcessInfo.processInfo.environment["MNEMONIC_SOURCE_DIR"] {
                return URL(fileURLWithPath: env).appendingPathComponent("clients/macos")
            }
            return home.appendingPathComponent("mnemonic/clients/macos")
        }()
        let built = sourceDir.appendingPathComponent(".build/release/MnemonicApp")
        if FileManager.default.fileExists(atPath: built.path) {
            spawn(executable: built, arguments: [])
        } else if let swift = locateSwift() {
            spawn(executable: URL(fileURLWithPath: swift),
                  arguments: ["run", "-c", "release", "MnemonicApp"], workingDir: sourceDir)
        } else {
            NSWorkspace.shared.activateFileViewerSelecting([built])
        }
    }

    private func spawn(executable: URL, arguments: [String], workingDir: URL? = nil) {
        let task = Process()
        task.executableURL = executable
        task.arguments = arguments
        if let workingDir { task.currentDirectoryURL = workingDir }
        task.standardOutput = FileHandle.nullDevice
        task.standardError = FileHandle.nullDevice
        try? task.run()
    }

    private func locateSwift() -> String? {
        ["/usr/bin/swift",
         "/Library/Developer/CommandLineTools/usr/bin/swift",
         "/Applications/Xcode.app/Contents/Developer/Toolchains/XcodeDefault.xctoolchain/usr/bin/swift",
         "/opt/homebrew/bin/swift", "/usr/local/bin/swift"]
            .first { FileManager.default.isExecutableFile(atPath: $0) }
    }

    private func runCommand(args: [String]) -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: mnemonicPath)
        process.arguments = args
        var env = ProcessInfo.processInfo.environment
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        env["PATH"] = "\(home)/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
        process.environment = env
        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice
        do {
            try process.run()
            // Drain stdout BEFORE waitUntilExit. A pipe buffers ~64KB;
            // with bigger output (`activity projects --json` qualifies)
            // the child blocks on write while we block in waitUntilExit —
            // deadlock, and orphaned mnemonic processes pile up.
            // readDataToEndOfFile returns at EOF, i.e. child exit, so the
            // wait after it never hangs.
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            process.waitUntilExit()
            return String(data: data, encoding: .utf8) ?? ""
        } catch {
            return ""
        }
    }
}
