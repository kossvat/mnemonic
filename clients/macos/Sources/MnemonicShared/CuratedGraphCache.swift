import Foundation

/// Per-scope request tickets keep a late response from replacing newer data.
struct CuratedGraphCache {
    struct Entry {
        let snapshot: CuratedGraphSnapshot
        let tag: String?
    }
    struct Ticket {
        let scope: String
        let serial: UInt64
        let previous: Entry?
    }
    private var entries: [String: Entry] = [:]
    private var order: [String] = []
    private var serial: UInt64 = 0
    private var running: [String: UInt64] = [:]

    mutating func begin(_ expansion: String?) -> Ticket {
        let scope = expansion ?? ""
        serial &+= 1
        running[scope] = serial
        return Ticket(scope: scope, serial: serial, previous: entries[scope])
    }

    func isCurrent(_ ticket: Ticket) -> Bool { running[ticket.scope] == ticket.serial }

    mutating func finish(_ ticket: Ticket, entry: Entry?) {
        guard isCurrent(ticket) else { return }
        running.removeValue(forKey: ticket.scope)
        entries[ticket.scope] = entry
        order.removeAll { scope in scope == ticket.scope }
        if entry != nil && !ticket.scope.isEmpty { order.append(ticket.scope) }
        while order.count > 8 { entries.removeValue(forKey: order.removeFirst()) }
    }
}
