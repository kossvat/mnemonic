import Combine
import Foundation
import MnemonicShared

/// Visible route reads share the live clock and never run for hidden windows.
@MainActor
final class AppProjectController: ObservableObject {
    @Published private(set) var board: ProjectBoard?
    @Published private(set) var detail: ProjectDetail?
    @Published private(set) var failure: LocalAPIFailure?
    @Published private(set) var loading = false
    @Published private(set) var visible = false
    @Published private(set) var now: Date
    let live = LiveStore()
    let coordinator: LiveCoordinator
    private(set) var route: AppRoute = .projects
    var onLegacyRefresh: (@MainActor (AppRoute) async -> Void)?
    /// A separately owned route can revoke its in-flight reply before live health returns.
    var onLegacyInvalidation: (@MainActor (AppRoute) -> Bool)?
    var onPresentationChange: (@MainActor () -> Void)?
    private let readBoard: @Sendable () async -> Result<ProjectBoard, LocalAPIFailure>
    private let readDetail: @Sendable (String) async -> Result<ProjectDetail, LocalAPIFailure>
    private let date: @MainActor () -> Date
    private var visit: UInt64 = 0
    private var requestedVisit: UInt64?
    private var runningVisits: Set<UInt64> = []
    private var invalidatedVisits: Set<UInt64> = []
    private var subscriber: LiveSignalSubscriber?

    init(
        clock: LiveClock, date: @escaping @MainActor () -> Date = Date.init,
        ask: @escaping @Sendable () async -> LiveAnswer,
        board: @escaping @Sendable () async -> Result<ProjectBoard, LocalAPIFailure>,
        detail: @escaping @Sendable (String) async -> Result<ProjectDetail, LocalAPIFailure>
    ) {
        self.date = date
        now = date()
        readBoard = board
        readDetail = detail
        coordinator = LiveCoordinator(store: live, clock: clock, allowHiddenRequests: false, ask: ask)
        coordinator.onLabelsTick = { [weak self] in
            guard let self, self.visible else { return }
            self.now = self.date()
        }
        coordinator.onRefreshTick = { [weak self] in
            guard let self, self.visible else { return }
            let started = self.visit
            if self.invalidateSelectedRead() { return }
            Task { @MainActor [weak self] in
                guard let self, self.visible, self.visit == started else { return }
                await self.refresh()
            }
        }
        coordinator.onAnswer = { [weak self] _ in
            guard let self, self.visible else { return }
            self.onPresentationChange?()
        }
    }

    convenience init(profile: MnemonicProfile, api: LocalAPI) {
        self.init(clock: SystemLiveClock(), ask: { await api.live() },
                  board: { await api.projects() }, detail: { key in await api.project(key: key) })
        if let name = try? LiveSignal.name(of: profile) {
            subscriber = LiveSignalSubscriber(name: name) { [weak self] in
                Task { @MainActor [weak self] in await self?.signalled() }
            }
        }
    }

    /// The route is selected synchronously; stale tasks cannot select an old page.
    @discardableResult
    func select(_ selected: AppRoute) -> Task<Void, Never>? {
        guard route != selected else { return nil }
        route = selected
        visit &+= 1
        requestedVisit = nil
        coordinator.setPlanVisible(selected.refreshes(at: date()))
        failure = nil
        loading = false
        // A deep link may change the route while every window is hidden.
        // Discard another project's detail before that route can reappear.
        if case .project(let key) = selected, detail?.key != key { detail = nil }
        onPresentationChange?()
        guard visible else { return nil }
        return Task { @MainActor [weak self] in await self?.refresh(force: false) }
    }

    func setVisible(_ shown: Bool) async {
        guard shown != visible else { return }
        if !shown { hide(); return }
        visible = true
        now = date()
        visit &+= 1
        requestedVisit = nil
        // Opening asks for live state; only this method loads the selected
        // route, so a delayed live answer cannot duplicate a project read.
        let started = visit
        await coordinator.setSurfaceVisible(true)
        guard visible, visit == started else { return }
        coordinator.setPlanVisible(route.refreshes(at: date()))
        await refresh(force: false)
    }

    func hide() {
        coordinator.hide()
        guard visible else { return }
        visible = false
        visit &+= 1
        requestedVisit = nil
        loading = false
        onPresentationChange?()
    }

    func signalled() async {
        guard visible else { return }
        let started = visit
        let replacing = invalidateSelectedRead()
        await coordinator.signalled()
        guard visible, visit == started else { return }
        // A read invalidated before the live await owns its replacement,
        // including when that replacement finishes before live health does.
        if !replacing { await refresh() }
    }

    private func invalidateSelectedRead() -> Bool {
        guard visible else { return false }
        switch route {
        case .projects, .project:
            guard runningVisits.contains(visit) else { return false }
            invalidatedVisits.insert(visit)
            now = date()
            return true
        default: return onLegacyInvalidation?(route) ?? false
        }
    }

    func refresh(force: Bool = true) async {
        guard visible else { return }
        // The label clock is absent with no live sessions. The existing
        // visible refresh still advances activity ages and the stale fold.
        now = date()
        // Today's timeline becomes historical at local midnight without a new timer.
        coordinator.setPlanVisible(route.refreshes(at: now))
        let currentVisit = visit
        let selected = route
        if force && invalidateSelectedRead() { return }
        guard !runningVisits.contains(currentVisit), force || requestedVisit != currentVisit else { return }
        requestedVisit = currentVisit
        runningVisits.insert(currentVisit)
        defer {
            runningVisits.remove(currentVisit)
            invalidatedVisits.remove(currentVisit)
        }
        repeat {
            invalidatedVisits.remove(currentVisit)
            switch selected {
            case .projects:
                loading = true
                let answer = await readBoard()
                guard visible, visit == currentVisit, route == selected else { return }
                if invalidatedVisits.contains(currentVisit) { continue }
                loading = false
                switch answer {
                case .success(let value):
                    if board != value { board = value }
                    failure = nil
                case .failure(let reason):
                    board = nil
                    failure = reason
                }
            case .project(let key):
                loading = true
                let answer = await readDetail(key)
                guard visible, visit == currentVisit, route == selected else { return }
                if invalidatedVisits.contains(currentVisit) { continue }
                loading = false
                switch answer {
                case .success(let value):
                    if detail != value { detail = value }
                    failure = nil
                case .failure(let reason):
                    detail = nil
                    failure = reason
                }
            default:
                await onLegacyRefresh?(selected)
            }
        } while visible && visit == currentVisit && route == selected && invalidatedVisits.contains(currentVisit)
        guard visible, visit == currentVisit else { return }
        onPresentationChange?()
    }

    /// Rendering fixtures does not construct a transport or arm a clock.
    func present(board: ProjectBoard?, detail: ProjectDetail?, live answer: LiveAnswer, route: AppRoute) {
        self.route = route
        self.board = board
        self.detail = detail
        visible = true
        live.visible = true
        live.take(answer)
        failure = nil
        loading = false
    }
}
