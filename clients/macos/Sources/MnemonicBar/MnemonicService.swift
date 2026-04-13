import Foundation
import AppKit

struct DailyCount: Identifiable {
    let id = UUID()
    let date: String
    let count: Int
}

struct MemoryStats {
    var total: Int = 0
    var decisions: Int = 0
    var feedback: Int = 0
    var notes: Int = 0
    var sessions: Int = 0
    var security: Int = 0
    var dbSizeKB: Double = 0
    var isRunning: Bool = false
    var pid: Int? = nil
    var daily: [DailyCount] = []
    var silentHours: Double? = nil
    var lastActivity: String? = nil
}

struct MemoryEntry: Identifiable {
    let id = UUID()
    let type: String
    let title: String
    let importance: Double
    let timestamp: String
}

class MnemonicService: ObservableObject {
    @Published var stats = MemoryStats()
    @Published var recent: [MemoryEntry] = []
    @Published var lastUpdate = Date()

    private let mnemonicPath: String
    private var timer: Timer?

    init() {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        self.mnemonicPath = "\(home)/.cargo/bin/mnemonic"
    }

    func startPolling(interval: TimeInterval = 10) {
        refresh()
        timer = Timer.scheduledTimer(withTimeInterval: interval, repeats: true) { [weak self] _ in
            self?.refresh()
        }
    }

    func stopPolling() {
        timer?.invalidate()
        timer = nil
    }

    func refresh() {
        DispatchQueue.global(qos: .utility).async { [weak self] in
            guard let self = self else { return }
            let newStats = self.fetchStats()
            let newRecent = self.fetchRecent()
            DispatchQueue.main.async {
                self.stats = newStats
                self.recent = newRecent
                self.lastUpdate = Date()
            }
        }
    }

    private func fetchStats() -> MemoryStats {
        let output = runCommand(args: ["stats", "--json"])
        var stats = MemoryStats()

        // Strip any non-JSON prefix (e.g. tracing log lines)
        let jsonStr: String
        if let braceIdx = output.firstIndex(of: "{") {
            jsonStr = String(output[braceIdx...])
        } else {
            jsonStr = output
        }

        guard let data = jsonStr.data(using: .utf8),
              let json = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else {
            // Fallback: at least check if running
            let statusOutput = runCommand(args: ["status"])
            stats.isRunning = statusOutput.contains("is running")
            return stats
        }

        stats.total = json["total"] as? Int ?? 0
        stats.dbSizeKB = json["db_size_kb"] as? Double ?? 0
        stats.isRunning = json["daemon_running"] as? Bool ?? false
        stats.pid = json["daemon_pid"] as? Int
        stats.silentHours = json["silent_hours"] as? Double
        stats.lastActivity = json["last_activity"] as? String

        if let byType = json["by_type"] as? [String: Any] {
            stats.decisions = byType["decision"] as? Int ?? 0
            stats.feedback = byType["feedback"] as? Int ?? 0
            stats.notes = byType["note"] as? Int ?? 0
            stats.sessions = byType["session_summary"] as? Int ?? 0
            stats.security = byType["security"] as? Int ?? 0
        }

        if let daily = json["daily"] as? [[String: Any]] {
            stats.daily = daily.compactMap { item in
                guard let date = item["date"] as? String,
                      let count = item["count"] as? Int else { return nil }
                return DailyCount(date: date, count: count)
            }
        }

        return stats
    }

    private func fetchRecent() -> [MemoryEntry] {
        let output = runCommand(args: ["recent", "-l", "8"])
        var entries: [MemoryEntry] = []

        // Parse lines like: "  [  decision] Title (importance: 0.7)"
        for line in output.components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)

            if trimmed.hasPrefix("[") {
                // Extract type
                if let closeBracket = trimmed.firstIndex(of: "]") {
                    let typeStr = trimmed[trimmed.index(after: trimmed.startIndex)..<closeBracket]
                        .trimmingCharacters(in: .whitespaces)

                    let rest = trimmed[trimmed.index(after: closeBracket)...]
                        .trimmingCharacters(in: .whitespaces)

                    // Extract title and importance
                    var title = String(rest)
                    var importance = 0.5

                    if let impRange = rest.range(of: "(importance: ") {
                        title = String(rest[..<impRange.lowerBound]).trimmingCharacters(in: .whitespaces)
                        let impStr = rest[impRange.upperBound...]
                        if let endParen = impStr.firstIndex(of: ")") {
                            importance = Double(impStr[..<endParen]) ?? 0.5
                        }
                    }

                    // Truncate long titles
                    if title.count > 60 {
                        title = String(title.prefix(57)) + "..."
                    }

                    entries.append(MemoryEntry(
                        type: String(typeStr),
                        title: title,
                        importance: importance,
                        timestamp: ""
                    ))
                }
            }
        }

        return entries
    }

    // MARK: - Actions

    func filterByType(_ type: String) -> [MemoryEntry] {
        let output = runCommand(args: ["recent", "-l", "20"])
        return parseEntries(output).filter { $0.type == type }
    }

    func search(query: String) -> [MemoryEntry] {
        let output = runCommand(args: ["query", query, "-l", "8"])
        return parseEntries(output)
    }

    func quickSave(title: String, type: String) {
        _ = runCommand(args: ["save", "-t", title, title, "-T", type])
    }

    func startDaemon() {
        _ = runCommand(args: ["start", "-d"])
    }

    func stopDaemon() {
        _ = runCommand(args: ["stop"])
    }

    func openLog() {
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        let logPath = "\(home)/.mnemonic/daemon.log"
        NSWorkspace.shared.open(URL(fileURLWithPath: logPath))
    }

    func generateContext() {
        _ = runCommand(args: ["context"])
    }

    private func parseEntries(_ output: String) -> [MemoryEntry] {
        var entries: [MemoryEntry] = []
        for line in output.components(separatedBy: "\n") {
            let trimmed = line.trimmingCharacters(in: .whitespaces)
            if trimmed.hasPrefix("[") {
                if let closeBracket = trimmed.firstIndex(of: "]") {
                    let typeStr = trimmed[trimmed.index(after: trimmed.startIndex)..<closeBracket]
                        .trimmingCharacters(in: .whitespaces)
                    let rest = trimmed[trimmed.index(after: closeBracket)...]
                        .trimmingCharacters(in: .whitespaces)
                    var title = String(rest)
                    var importance = 0.5
                    if let impRange = rest.range(of: "(importance: ") {
                        title = String(rest[..<impRange.lowerBound]).trimmingCharacters(in: .whitespaces)
                        let impStr = rest[impRange.upperBound...]
                        if let endParen = impStr.firstIndex(of: ")") {
                            importance = Double(impStr[..<endParen]) ?? 0.5
                        }
                    }
                    if title.count > 60 { title = String(title.prefix(57)) + "..." }
                    entries.append(MemoryEntry(type: String(typeStr), title: title, importance: importance, timestamp: ""))
                }
            }
        }
        return entries
    }

    private func runCommand(args: [String]) -> String {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: mnemonicPath)
        process.arguments = args

        // Set PATH so mnemonic can find its deps
        var env = ProcessInfo.processInfo.environment
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        env["PATH"] = "\(home)/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
        process.environment = env

        let pipe = Pipe()
        process.standardOutput = pipe
        process.standardError = FileHandle.nullDevice

        do {
            try process.run()
            process.waitUntilExit()
            let data = pipe.fileHandleForReading.readDataToEndOfFile()
            return String(data: data, encoding: .utf8) ?? ""
        } catch {
            return ""
        }
    }
}
