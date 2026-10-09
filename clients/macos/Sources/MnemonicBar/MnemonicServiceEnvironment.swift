import Foundation
import MnemonicShared

/// Replace data sources and dispatch queues in lifecycle tests.
struct WidgetEnvironment {
    var fetch: ((_ command: Bool, _ last: WidgetData) -> WidgetData?)?
    /// Source compatibility for existing fixtures. The service never calls it.
    var every: (_ interval: TimeInterval, _ fire: @escaping () -> Void) -> () -> Void
    var background: (@escaping () -> Void) -> Void
    var main: (@escaping () -> Void) -> Void
    var fetchPage: ((_ page: Int, _ command: Bool, _ last: WidgetData) -> WidgetData?)?
    var fetchMonth: ((_ command: Bool) -> [WorkDay])?
    var http: ((_ path: String) -> (status: Int, body: Data)?)?
    var command: ((_ args: [String], _ budget: CommandBudget) -> String)?

    init(
        fetch: ((_ command: Bool, _ last: WidgetData) -> WidgetData?)? = nil,
        every: @escaping (_ interval: TimeInterval, _ fire: @escaping () -> Void) -> () -> Void = { _, _ in {} },
        background: @escaping (@escaping () -> Void) -> Void,
        main: @escaping (@escaping () -> Void) -> Void,
        fetchPage: ((_ page: Int, _ command: Bool, _ last: WidgetData) -> WidgetData?)? = nil,
        fetchMonth: ((_ command: Bool) -> [WorkDay])? = nil,
        http: ((_ path: String) -> (status: Int, body: Data)?)? = nil,
        command: ((_ args: [String], _ budget: CommandBudget) -> String)? = nil
    ) {
        self.fetch = fetch
        self.every = every
        self.background = background
        self.main = main
        self.fetchPage = fetchPage
        self.fetchMonth = fetchMonth
        self.http = http
        self.command = command
    }

    static let live = WidgetEnvironment(
        background: { work in DispatchQueue.global(qos: .utility).async(execute: work) },
        main: { work in DispatchQueue.main.async(execute: work) })
}

/// One automatic fallback batch per opening, shared with page-owned loads.
/// A late request from a previous opening cannot take the new allowance.
final class WidgetAutomaticFallback {
    private let lock = NSLock()
    private var opening: UInt64 = 0
    private var available = false

    func begin() -> UInt64 {
        lock.lock()
        defer { lock.unlock() }
        opening &+= 1
        available = true
        return opening
    }

    func end() {
        lock.lock()
        available = false
        lock.unlock()
    }

    func claim(opening expected: UInt64) -> CommandBudget? {
        lock.lock()
        defer { lock.unlock() }
        guard available, expected == opening else { return nil }
        available = false
        return CommandBudget(limit: MnemonicService.commandLimit)
    }
}

enum WidgetCommands {
    case automatic(WidgetAutomaticFallback, UInt64)
    case explicit
    case none

    var isExplicit: Bool {
        if case .explicit = self { return true }
        return false
    }
}

/// HTTP-first page reads share one command deadline, created only if needed.
final class WidgetCommandAllowance {
    private let commands: WidgetCommands
    private var tried = false
    private var held: CommandBudget?

    init(_ commands: WidgetCommands) { self.commands = commands }

    func budget() -> CommandBudget? {
        if !tried {
            tried = true
            switch commands {
            case .automatic(let fallback, let opening):
                held = fallback.claim(opening: opening)
            case .explicit:
                held = CommandBudget(limit: MnemonicService.commandLimit)
            case .none:
                break
            }
        }
        return held
    }
}
