import Foundation

/// Which memory the card of the latest memory shows: the first of the
/// recent ones that is worth reading. A memory the daemon marked as noise
/// (`noise` set in the item) is never shown; a summary of a session and a
/// note that reads like a file or build event are passed over, and shown
/// only when nothing else is there.
public enum MemoryPick {
    /// Whether the daemon marked the item as noise.
    public static func isNoise(_ m: [String: Any]) -> Bool {
        guard let mark = m["noise"] else { return false }
        return !(mark is NSNull)
    }

    /// Is this memory worth surfacing on the card? Decisions and feedback
    /// qualify; notes qualify unless they read like a file/build event.
    /// Session summaries (dream output) never qualify, and neither does a
    /// memory marked as noise.
    public static func isMeaningful(_ m: [String: Any]) -> Bool {
        if isNoise(m) { return false }
        let type = memoryType(m).lowercased()
        if type == "decision" || type == "feedback" { return true }
        if type != "note" { return false }
        let hay = ((m["title"] as? String ?? "") + " " + (m["content"] as? String ?? "")).lowercased()
        let noise = [".build/", "node_modules", "new file:", "deleted:", "untracked",
                     "modified file", "file changed", "build complete", "target/"]
        return !noise.contains { word in hay.contains(word) }
    }

    /// The type of the item, under either name the daemon uses.
    public static func memoryType(_ m: [String: Any]) -> String {
        (m["type"] as? String) ?? (m["memory_type"] as? String) ?? "note"
    }

    /// The item the card shows: the first meaningful one, else the first
    /// that is not marked as noise, else nothing.
    public static func latest(of items: [[String: Any]]) -> [String: Any]? {
        items.first(where: isMeaningful) ?? items.first(where: { item in !isNoise(item) })
    }
}
