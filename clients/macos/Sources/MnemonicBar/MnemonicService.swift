import Foundation
import AppKit
import Combine
import MnemonicShared

/// Data for secondary widget pages. Page appearances request their own
/// data; the live coordinator owns all recurring work and the status title.
class MnemonicService: ObservableObject {
    @Published private(set) var data = WidgetData() { didSet { assignments += 1 } }
    @Published private(set) var monthDays: [WorkDay] = [] { didSet { assignments += 1 } }

    /// A visible page may react to an explicit refresh. No timer sends this.
    let refreshed = PassthroughSubject<Bool, Never>()
    private(set) var backing = WidgetData()
    private(set) var isOpen = false
    private(set) var selectedPage = 0
    private(set) var assignments = 0
    private(set) var dropped = 0
    /// Retained for source compatibility. Live projects own title updates.
    var onTitle: ((StatusTitle) -> Void)?
    /// Daemon actions invalidate the live snapshot even while closed.
    var onDaemonAction: (() -> Void)?

    let environment: WidgetEnvironment
    let automaticFallback = WidgetAutomaticFallback()
    private var opening: UInt64 = 0
    private(set) var pageVisit: UInt64 = 0
    private var active: PageRequest?
    private var pending: PageRequest?
    private var backingMonth: [WorkDay] = []
    private var monthVisit: UInt64?
    private var monthLoading = false
    private var pendingMonth: (visit: UInt64, commands: WidgetCommands)?
    private var dashboard: Process?
    private var dashboardReadiness: AppLinkReadinessSubscriber?
    private var dashboardRoutes = DashboardLaunchState()

    static let commandLimit: TimeInterval = 6
    static let actionLimit: TimeInterval = 30

    private struct PageRequest {
        let page: Int
        let visit: UInt64
        let commands: WidgetCommands
    }

    init(environment: WidgetEnvironment = .live) {
        self.environment = environment
    }

    private var mnemonicPath: String { MnemonicProfile.current.command.path }
    private var ownsPageData: Bool { [1, 2, 5].contains(selectedPage) }

    /// Compatibility with the previous lifecycle. Startup does no legacy work.
    func start() {}

    func opened() {
        guard !isOpen else { return }
        isOpen = true
        opening = automaticFallback.begin()
        pageVisit &+= 1
        publish()
        requestAppearance()
    }

    func closed() {
        isOpen = false
        pending = nil
        pendingMonth = nil
        automaticFallback.end()
    }

    /// The deck calls this on selection. Journal and Records fetch their
    /// own data when mounted; live page zero never starts a legacy request.
    func selectPage(_ page: Int) {
        guard (0...5).contains(page), page != selectedPage else { return }
        selectedPage = page
        pageVisit &+= 1
        pending = nil
        pendingMonth = nil
        guard isOpen else { return }
        publish()
        requestAppearance()
    }

    private func requestAppearance() {
        guard isOpen, ownsPageData else { return }
        enqueue(PageRequest(
            page: selectedPage, visit: pageVisit,
            commands: .automatic(automaticFallback, opening)))
    }

    /// Explicit actions may use their bounded command fallback. Requests
    /// without that permission stay HTTP-only. Hidden pages ask nothing.
    func refresh(command: Bool = true) {
        guard isOpen else { return }
        guard ownsPageData else {
            if selectedPage == 4 { refreshed.send(command) }
            return
        }
        enqueue(PageRequest(
            page: selectedPage, visit: pageVisit,
            commands: command ? .explicit : .none))
    }

    private func enqueue(_ request: PageRequest) {
        guard active == nil else {
            dropped += 1
            // Only the latest visible page is owed. Repeated appearances
            // of the active visit do not create a duplicate request.
            if active?.visit != request.visit || request.commands.isExplicit {
                pending = request
            }
            return
        }
        active = request
        let last = backing
        environment.background { [weak self] in
            guard let self else { return }
            let allowance = WidgetCommandAllowance(request.commands)
            let fetched: WidgetData?
            if let fetch = self.environment.fetchPage {
                fetched = fetch(request.page, allowance.budget() != nil, last)
            } else if let fetch = self.environment.fetch {
                fetched = fetch(allowance.budget() != nil, last)
            } else {
                fetched = self.fetchData(page: request.page, commands: allowance, last: last)
            }
            self.environment.main {
                self.active = nil
                if let fetched { self.backing = fetched }
                if self.isOpen, self.pageVisit == request.visit {
                    self.publish()
                    self.refreshed.send(request.commands.isExplicit)
                    if request.page == 1, self.monthVisit == request.visit {
                        self.requestMonth(commands: request.commands)
                    }
                }
                let owed = self.pending
                self.pending = nil
                if let owed, self.isOpen, self.pageVisit == owed.visit {
                    self.enqueue(owed)
                }
            }
        }
    }

    private func publish() {
        guard isOpen else { return }
        if data != backing { data = backing }
        if selectedPage == 1, monthDays != backingMonth { monthDays = backingMonth }
    }

    /// Fixture preview injection is explicit and starts no data work.
    func present(_ shown: WidgetData) {
        backing = shown
        if data != shown { data = shown }
    }

    func loadMonthIfNeeded() {
        guard isOpen, selectedPage == 1, monthVisit != pageVisit else { return }
        requestMonth(commands: .explicit)
    }

    private func requestMonth(commands: WidgetCommands) {
        guard isOpen, selectedPage == 1 else { return }
        let visit = pageVisit
        guard !monthLoading else {
            if monthVisit != visit { pendingMonth = (visit, commands) }
            return
        }
        monthVisit = visit
        monthLoading = true
        fetchMonth(commands: WidgetCommandAllowance(commands)) { [weak self] days in
            guard let self else { return }
            self.monthLoading = false
            if !days.isEmpty {
                self.backingMonth = days
            } else if self.monthVisit == visit {
                // A later Week-to-Month selection may retry this visit.
                // Keep the marker until delivery so in-flight asks coalesce.
                self.monthVisit = nil
            }
            if self.isOpen, self.pageVisit == visit { self.publish() }
            let owed = self.pendingMonth
            self.pendingMonth = nil
            if let owed, self.showing(page: 1, visit: owed.visit) {
                self.requestMonth(commands: owed.commands)
            }
        }
    }

    /// A page callback may update its view only during the same appearance.
    func showing(page: Int, visit: UInt64) -> Bool {
        isOpen && selectedPage == page && pageVisit == visit
    }

    /// Capture this opening before a page starts asynchronous HTTP work.
    func automaticCommands() -> WidgetCommandAllowance {
        WidgetCommandAllowance(.automatic(automaticFallback, opening))
    }

    // MARK: - Actions

    func addMemory(title: String, type: String) {
        runActionAsync(["save", "-t", title, title, "-T", type])
    }

    func startDaemon() { runActionAsync(["start", "-d"], daemonAction: true) }
    func stopDaemon() { runActionAsync(["stop"], daemonAction: true) }

    func openLog() {
        NSWorkspace.shared.open(MnemonicProfile.current.logURL)
    }

    func generateContext() { runActionAsync(["context", "--no-record"]) }

    /// Fire-and-forget CLI action off the main thread. These ran
    /// synchronously on main before — `stop` alone blocks up to 5s in the
    /// daemon's SIGTERM poll, freezing the whole popover. Refreshes widget
    /// data once the command finishes, if its page is still visible.
    private func runActionAsync(_ args: [String], daemonAction: Bool = false) {
        environment.background { [weak self] in
            guard let self else { return }
            _ = self.runCommand(args: args, budget: CommandBudget(limit: Self.actionLimit))
            self.environment.main {
                if daemonAction { self.onDaemonAction?() }
                self.refresh()
            }
        }
    }

    /// Reuse only the dashboard this service started. Another running app
    /// may belong to another profile, so it is never selected by its name.
    func openDashboard(link: AppLink = .projects) {
        let profile = MnemonicProfile.current
        guard let identity = try? profile.identity(), link.url(identity: identity) != nil else { return }
        if let started = dashboard, started.isRunning {
            guard dashboardRoutes.readiness?.identity == identity else {
                dashboardFailure("Close the dashboard before opening a different profile, then try again.")
                return
            }
            if dashboardRoutes.rejectExpired() {
                dashboardReadiness?.cancel()
                dashboardReadiness = nil
                dashboardFailure("The dashboard did not become ready. Close it, then update or rebuild the dashboard and try again.")
                return
            }
            if let ready = dashboardRoutes.request(link, identity: identity) { ready.post(identity: identity) }
            return
        }
        if let previous = dashboardRoutes.readiness { dashboardEnded(previous) }
        let home = FileManager.default.homeDirectoryForCurrentUser
        let sourceDir: URL = {
            if let env = ProcessInfo.processInfo.environment["MNEMONIC_SOURCE_DIR"] {
                return URL(fileURLWithPath: env).appendingPathComponent("clients/macos")
            }
            return home.appendingPathComponent("mnemonic/clients/macos")
        }()
        let built = sourceDir.appendingPathComponent(".build/release/MnemonicApp")
        let sibling = Bundle.main.executableURL?.deletingLastPathComponent()
            .appendingPathComponent("MnemonicApp")
        let executables = sibling.map { executable in [executable, built] } ?? [built]
        let bundles = [
            home.appendingPathComponent("Applications/Mnemonic.app"),
            URL(fileURLWithPath: "/Applications/Mnemonic.app"),
        ]
        let localBundle = sourceDir.appendingPathComponent("build")
            .appendingPathComponent(bundles[0].lastPathComponent)
        guard let binary = DashboardCapability.firstCompatible(built: executables, bundles: [localBundle] + bundles) else {
            dashboardFailure("No compatible dashboard was found. Update the dashboard, or run clients/macos/scripts/build-app.sh from the current source checkout, then try again.")
            return
        }
        let nonce = UUID().uuidString.replacingOccurrences(of: "-", with: "").lowercased()
        guard let readiness = AppLinkReadiness(identity: identity, nonce: nonce),
              let arguments = Self.dashboardArguments(
                profile: profile, link: link, identity: identity, readinessNonce: nonce),
              dashboardRoutes.begin(readiness, link: link) else { return }
        dashboardReadiness = AppLinkReadinessSubscriber(readiness: readiness) { [weak self] notice in
            DispatchQueue.main.async { self?.dashboardBecameReady(notice) }
        }
        dashboard = spawn(executable: binary, arguments: arguments, readiness: readiness)
        if dashboard == nil {
            dashboardEnded(readiness)
            dashboardFailure("The dashboard could not start. Update or rebuild the dashboard, then try again.")
        }
    }

    /// The route names only an opaque identity. The profile's existing
    /// command arguments select its store outside the URL.
    static func dashboardArguments(
        profile: MnemonicProfile, link: AppLink, identity: String, readinessNonce: String? = nil
    ) -> [String]? {
        guard let url = link.url(identity: identity) else { return nil }
        var arguments = profile.commandArguments + ["--open-url", url.absoluteString]
        if let readinessNonce {
            guard AppLinkReadiness(identity: identity, nonce: readinessNonce) != nil else { return nil }
            arguments += ["--route-ready", readinessNonce]
        }
        return arguments
    }

    private func dashboardBecameReady(_ readiness: AppLinkReadiness) {
        guard dashboardRoutes.readiness == readiness else { return }
        guard dashboard?.isRunning == true else {
            dashboardEnded(readiness)
            return
        }
        guard let latest = dashboardRoutes.received(readiness) else { return }
        dashboardReadiness?.cancel()
        dashboardReadiness = nil
        latest.post(identity: readiness.identity)
    }

    private func dashboardEnded(_ readiness: AppLinkReadiness) {
        guard dashboardRoutes.ended(readiness) else { return }
        dashboardReadiness?.cancel()
        dashboardReadiness = nil
        dashboard = nil
    }

    @discardableResult
    private func spawn(
        executable: URL, arguments: [String], workingDir: URL? = nil, readiness: AppLinkReadiness
    ) -> Process? {
        let task = Process()
        task.executableURL = executable
        task.arguments = arguments
        if let workingDir { task.currentDirectoryURL = workingDir }
        task.standardOutput = FileHandle.nullDevice
        task.standardError = FileHandle.nullDevice
        task.terminationHandler = { [weak self] _ in
            DispatchQueue.main.async { self?.dashboardEnded(readiness) }
        }
        do {
            try task.run()
            return task
        } catch {
            return nil
        }
    }

    private func dashboardFailure(_ message: String) {
        let alert = NSAlert()
        alert.messageText = "Dashboard unavailable"
        alert.informativeText = message
        alert.addButton(withTitle: "OK")
        alert.runModal()
    }

    /// Run the `mnemonic` command within the budget, and give what it
    /// wrote. A run that was cut at the limit gives nothing.
    func runCommand(args: [String], budget: CommandBudget) -> String {
        if let run = environment.command { return run(args, budget) }
        var env = ProcessInfo.processInfo.environment
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        env["PATH"] = "\(home)/.cargo/bin:/usr/local/bin:/usr/bin:/bin"
        // An isolated profile is named to the command too: without it the
        // command would read the owner's store.
        let output = budget.output(
            of: URL(fileURLWithPath: mnemonicPath),
            arguments: MnemonicProfile.current.commandArguments + args,
            environment: env)
        return output.flatMap { bytes in String(data: bytes, encoding: .utf8) } ?? ""
    }
}
