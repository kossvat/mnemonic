import Foundation
import MnemonicShared

extension MnemonicService {
    /// A Journal answer belongs to the page appearance that requested it.
    func loadJournal(day: String, completion: @escaping ([String: Any]?) -> Void) {
        guard isOpen, selectedPage == 3 else { return }
        let visit = pageVisit
        environment.background { [weak self] in
            guard let self else { return }
            let object = self.httpObject("/api/journal?day=\(day)")
            self.environment.main {
                guard self.showing(page: 3, visit: visit) else { return }
                completion(object)
            }
        }
    }

    /// Appearance loads share the opening's allowance; an explicit refresh
    /// gets its own bounded retry. Delivery still belongs to this visit.
    func loadRecords(
        command: Bool, automatic: Bool = true,
        completion: @escaping ([String: Any]?) -> Void
    ) {
        guard isOpen, selectedPage == 4 else { return }
        let visit = pageVisit
        let commands: WidgetCommandAllowance
        if !command {
            commands = WidgetCommandAllowance(.none)
        } else if automatic {
            commands = automaticCommands()
        } else {
            commands = WidgetCommandAllowance(.explicit)
        }
        environment.background { [weak self] in
            guard let self else { return }
            let object = self.httpObject("/api/leaderboard/sessions?limit=10")
                ?? self.cliObject(
                    ["session", "leaderboard", "--limit", "10", "--json"], commands: commands)
            self.environment.main {
                guard self.showing(page: 4, visit: visit) else { return }
                completion(object)
            }
        }
    }

    func fetchMonth(commands: WidgetCommandAllowance, deliver: @escaping ([WorkDay]) -> Void) {
        environment.background { [weak self] in
            guard let self else { return }
            var days: [WorkDay] = []
            if let fetch = self.environment.fetchMonth {
                days = fetch(commands.budget() != nil)
            } else if let object = self.httpObject("/api/activity/week?days=30"),
                      let rows = object["days"] as? [[String: Any]] {
                days = rows.map { row in self.dayFromBar(row) }
            } else if let budget = commands.budget(),
                      let rows = self.parseArray(self.runCommand(
                        args: ["activity", "week", "--days", "30", "--json"], budget: budget)) {
                days = rows.map { row in self.dayFromBar(row) }
            }
            self.environment.main { deliver(days) }
        }
    }

    /// A user-selected day may use its own bounded command budget. A late
    /// reply does not update a page that was closed or replaced meanwhile.
    func loadDayDetail(date: String, completion: @escaping (WorkDay?) -> Void) {
        guard isOpen, [1, 5].contains(selectedPage) else { return }
        let page = selectedPage
        let visit = pageVisit
        environment.background { [weak self] in
            guard let self else { return }
            let day = self.dayDetailNow(date: date)
            self.environment.main {
                guard self.showing(page: page, visit: visit) else { return }
                completion(day)
            }
        }
    }

    /// Synchronous detail is also used by the explicit legacy renderer.
    func dayDetailNow(date: String) -> WorkDay? {
        let object = httpObject("/api/activity/day?date=\(date)")
            ?? parseObject(runCommand(
                args: ["activity", "day", "--date", date],
                budget: CommandBudget(limit: Self.commandLimit)))
        return object.map { value in dayFromDetail(value, isToday: false) }
    }

    // MARK: Parsing helpers

    func dayFromBar(_ j: [String: Any]) -> WorkDay {
        let date = j["date"] as? String ?? ""
        let secs = num(j["seconds"])
        return makeDay(date: date, seconds: secs, isToday: isTodayStr(date))
    }

    func dayFromDetail(_ j: [String: Any], isToday: Bool) -> WorkDay {
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

    func makeDay(date: String, seconds: Double, isToday: Bool) -> WorkDay {
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

    func isTodayStr(_ s: String) -> Bool {
        let f = DateFormatter(); f.dateFormat = "yyyy-MM-dd"
        return s == f.string(from: Date())
    }

    struct YMD { let day: Int; let weekdayIndex: Int; let label: String }
    func parseYMD(_ s: String) -> YMD? {
        let f = DateFormatter(); f.dateFormat = "yyyy-MM-dd"
        guard let d = f.date(from: s) else { return nil }
        let cal = Calendar.current
        let day = cal.component(.day, from: d)
        let wd = cal.component(.weekday, from: d) - 1 // 0=Sun
        let lf = DateFormatter(); lf.dateFormat = "EEE, MMM d"
        return YMD(day: day, weekdayIndex: wd, label: lf.string(from: d))
    }

    /// RFC3339 → local minutes-from-midnight.
    func localMinutes(_ iso: String) -> Double? {
        guard let d = parseISO(iso) else { return nil }
        let cal = Calendar.current
        let h = cal.component(.hour, from: d)
        let m = cal.component(.minute, from: d)
        return Double(h * 60 + m)
    }

    func minutesAgo(_ iso: String) -> Int {
        guard let d = parseISO(iso) else { return 0 }
        return max(0, Int(Date().timeIntervalSince(d) / 60.0))
    }

    func parseISO(_ s: String) -> Date? {
        let f = ISO8601DateFormatter()
        f.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = f.date(from: s) { return d }
        f.formatOptions = [.withInternetDateTime]
        return f.date(from: s)
    }

    func num(_ v: Any?) -> Double {
        if let n = v as? NSNumber { return n.doubleValue }
        if let s = v as? String { return Double(s) ?? 0 }
        return 0
    }

    /// Strip any non-JSON prefix (tracing log lines) then parse an object.
    func parseObject(_ out: String) -> [String: Any]? {
        guard let i = out.firstIndex(of: "{"),
              let data = String(out[i...]).data(using: .utf8),
              let j = try? JSONSerialization.jsonObject(with: data) as? [String: Any]
        else { return nil }
        return j
    }

    /// Run the mnemonic CLI and parse its stdout as a JSON object. The
    /// fallback path for pages whose HTTP endpoint is unavailable — the
    /// dashboard API is disabled in the default config.
    func cliObject(_ args: [String], commands: WidgetCommandAllowance) -> [String: Any]? {
        guard let budget = commands.budget() else { return nil }
        return parseObject(runCommand(args: args, budget: budget))
    }

    /// The command's output from its first brace, as bytes, for a model
    /// that decodes by hand: what precedes the brace is not JSON (a
    /// tracing line), as parseObject assumes. Nil when the command gave
    /// no object at all.
    func commandJSON(args: [String], budget: CommandBudget) -> Data? {
        let out = runCommand(args: args, budget: budget)
        guard let i = out.firstIndex(of: "{") else { return nil }
        return String(out[i...]).data(using: .utf8)
    }

    func parseArray(_ out: String) -> [[String: Any]]? {
        guard let i = out.firstIndex(of: "["),
              let data = String(out[i...]).data(using: .utf8),
              let j = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]]
        else { return nil }
        return j
    }

}
