import Foundation
import SwiftUI
import MnemonicShared

@MainActor
final class MnemonicAppModel: ObservableObject {
    @Published var route: AppRoute = .projects {
        didSet {
            guard route != oldValue else { return }
            epoch &+= 1
            isLoading = false
            syncTimelineContext()
            graphDrawnSnapshot = nil
            syncGraphContext()
            projects.select(route)
        }
    }
    @Published var status: StatusResponse?
    @Published var memories: [Memory] = []
    @Published var entities: [EntitySummary] = []
    @Published var daily: [DailyCount] = []
    @Published var isLoading = false
    @Published var setupMessage: String?
    @Published var toastMessage: String?
    @Published private(set) var endpoint: String
    @Published var theme: String {
        didSet { preferences.set(theme, forKey: "MnemonicTheme") }
    }

    let profile: MnemonicProfile
    let identity: String?
    let client: MnemonicClient
    let localAPI: LocalAPI
    let projects: AppProjectController
    let timeline: AppTimelineController
    let curatedGraph: AppGraphController
    private(set) var graphDrawnSnapshot: CuratedGraphSnapshot?
    private let preferences: ProfilePreferences
    private let readMemories: @MainActor () async throws -> MemoryListResponse
    private let readEntities: @MainActor () async throws -> EntitiesResponse
    private let readStatus: @MainActor () async throws -> StatusResponse
    private var visibleWindows: Set<ObjectIdentifier> = []
    private var epoch: UInt64 = 0
    private var runningEpochs: Set<UInt64> = []
    private var invalidatedEpochs: Set<UInt64> = []
    private var linkSubscriber: AppLinkSubscriber?
    private var fixtureState: FixtureWindowState?
    private var endpointChanging = false
    var onWindowStateChange: (@MainActor () -> Void)?
    let dashboardActivation = DashboardActivation()
    lazy var activateWindow: @MainActor () -> Void = { [weak self] in
        self?.dashboardActivation.activate()
    }

    init(
        profile: MnemonicProfile = .current, controller: AppProjectController? = nil,
        timelineController: AppTimelineController? = nil,
        graphController: AppGraphController? = nil,
        readMemories: (@MainActor () async throws -> MemoryListResponse)? = nil,
        readEntities: (@MainActor () async throws -> EntitiesResponse)? = nil,
        readStatus: (@MainActor () async throws -> StatusResponse)? = nil
    ) {
        self.profile = profile
        preferences = ProfilePreferences(profile: profile, client: "MnemonicApp")
        LegacyCache.removeForThisClient(profile: profile)
        let saved = profile.isolated || profile.endpointNamed ? nil
            : preferences.string(forKey: "MnemonicEndpoint")
        let initial = saved.flatMap { value in try? LocalAPI.validatedEndpoint(value, profile: profile) }
            ?? profile.endpoint
        endpoint = initial.absoluteString
        theme = preferences.string(forKey: "MnemonicTheme") ?? "system"
        localAPI = (try? LocalAPI(profile: profile, endpoint: initial)) ?? LocalAPI(profile: profile)
        let client = MnemonicClient(profile: profile, endpoint: initial)
        self.client = client
        self.readMemories = readMemories ?? { try await client.fetchMemories(limit: 150) }
        self.readEntities = readEntities ?? { try await client.fetchEntities(limit: 500) }
        self.readStatus = readStatus ?? { try await client.fetchStatus() }
        projects = controller ?? AppProjectController(profile: profile, api: localAPI)
        timeline = timelineController ?? AppTimelineController(api: localAPI)
        curatedGraph = graphController ?? AppGraphController(api: localAPI)
        identity = try? profile.identity()
        projects.onLegacyRefresh = { [weak self] selected in
            await self?.refreshLegacy(selected)
        }
        projects.onLegacyInvalidation = { [weak self] selected in
            guard let self, self.route == selected, !self.endpointChanging else { return false }
            if selected == .graph {
                let replacing = self.curatedGraph.invalidateRunningRead()
                self.curatedGraph.invalidateSelected()
                return replacing
            }
            if selected.timelineDay != nil { return self.timeline.invalidateRunningRead() }
            return false
        }
        projects.onPresentationChange = { [weak self] in
            self?.syncTimelineContext()
            self?.syncGraphContext()
            self?.onWindowStateChange?()
        }
        curatedGraph.onPresentationChange = { [weak self] in
            guard let self else { return }
            if self.curatedGraph.snapshot == nil || self.curatedGraph.failure != nil {
                self.graphDrawnSnapshot = nil
            }
            self.onWindowStateChange?()
        }
        if let identity {
            linkSubscriber = AppLinkSubscriber(identity: identity) { [weak self] link in
                Task { @MainActor [weak self] in self?.open(link) }
            }
        }
        if CommandLine.arguments.contains("--fixture-window"), profile.isolated {
            fixtureState = FixtureWindowState(profile: profile)
            onWindowStateChange = { [weak self] in
                guard let self else { return }
                self.fixtureState?.update(model: self)
            }
        }
    }

    var isOnline: Bool {
        let liveOnline = projects.live.shown != nil && projects.live.failure == nil
        if route.sidebarRoute == .projects || route.timelineDay != nil || route == .graph { return liveOnline }
        return setupMessage == nil && projects.live.failure == nil && (status != nil || liveOnline)
    }
    var tokenPath: String { get async { await client.tokenPath() } }
    var visible: Bool { projects.visible }

    private func syncTimelineContext(visible shown: Bool? = nil) {
        timeline.setContext(day: route.timelineDay, visible: (shown ?? visible) && !endpointChanging)
    }

    private func syncGraphContext(visible shown: Bool? = nil) {
        curatedGraph.setContext(visible: (shown ?? visible) && route == .graph && !endpointChanging)
    }

    /// The canvas reports its captured snapshot only after a completed paint.
    /// Snapshot removal or failure revokes proof even if an identical response returns.
    func graphDidDraw(_ snapshot: CuratedGraphSnapshot) {
        guard visible, route == .graph, !endpointChanging,
              !curatedGraph.loading, curatedGraph.failure == nil,
              curatedGraph.snapshot == snapshot, !snapshot.nodes.isEmpty else { return }
        graphDrawnSnapshot = snapshot
        onWindowStateChange?()
    }

    func window(_ id: ObjectIdentifier, visible shown: Bool) {
        if shown { visibleWindows.insert(id) } else { visibleWindows.remove(id) }
        if visibleWindows.isEmpty {
            graphDrawnSnapshot = nil
            epoch &+= 1
            isLoading = false
            syncTimelineContext(visible: false)
            syncGraphContext(visible: false)
            projects.hide()
        } else if !projects.visible && !endpointChanging {
            Task { @MainActor [weak self] in
                guard let self, !self.visibleWindows.isEmpty, !self.endpointChanging else { return }
                self.syncTimelineContext(visible: true)
                self.syncGraphContext(visible: true)
                await self.projects.setVisible(true)
            }
        }
        onWindowStateChange?()
    }

    @discardableResult
    func open(_ url: URL) -> Bool {
        guard let identity, let link = AppLink(url: url, expectedIdentity: identity) else { return false }
        open(link)
        return true
    }

    private func open(_ link: AppLink) {
        switch link {
        case .project(let key): route = .project(key)
        case .projects: route = .projects
        case .search: route = .search
        }
        activateWindow()
    }

    func applyEndpoint(_ value: String) async {
        guard !endpointChanging else { return }
        do {
            let chosen = try LocalAPI.validatedEndpoint(value, profile: profile)
            endpointChanging = true
            defer { endpointChanging = false }
            epoch &+= 1
            syncTimelineContext(visible: false)
            timeline.resetEndpoint()
            syncGraphContext(visible: false)
            curatedGraph.resetEndpoint()
            graphDrawnSnapshot = nil
            projects.hide()
            try await localAPI.setEndpoint(chosen.absoluteString)
            try await client.setEndpoint(chosen.absoluteString)
            endpoint = chosen.absoluteString
            preferences.set(endpoint, forKey: "MnemonicEndpoint")
            status = nil
            setupMessage = nil
            endpointChanging = false
            if !visibleWindows.isEmpty {
                syncTimelineContext(visible: true)
                syncGraphContext(visible: true)
                await projects.setVisible(true)
            }
        } catch {
            setupMessage = error.localizedDescription
        }
    }

    /// Explicit refresh updates live health and the selected route together.
    func refreshAll(invalidate: Bool = true) async {
        if invalidate, visible, route.timelineDay != nil, !endpointChanging {
            timeline.invalidateSelected()
        }
        if invalidate, visible, route == .graph, !endpointChanging {
            curatedGraph.invalidateSelected()
        }
        if invalidate && invalidateRunningRead(route) {
            // The active read owns its replacement, even if it completes
            // before live health returns. Do not request that route twice.
            await projects.coordinator.signalled()
        } else if invalidate || !runningEpochs.contains(epoch) {
            await projects.signalled()
        }
    }

    private func refreshLegacy(_ selected: AppRoute) async {
        switch selected {
        case .timeline:
            guard route == selected, !endpointChanging else { return }
            syncTimelineContext()
            await timeline.refresh()
        case .overview: await refreshOverview()
        case .memories, .memoryMap: await refreshMemories()
        case .entities: await refreshEntities()
        case .graph:
            guard route == selected, !endpointChanging else { return }
            syncGraphContext()
            await curatedGraph.refresh()
        case .manage: await refreshStatus()
        case .search:
            if setupMessage != nil { await refreshStatus(.search) }
        default: break
        }
    }

    private func begin(_ selected: AppRoute) -> UInt64? {
        guard visible, route == selected, !runningEpochs.contains(epoch), !endpointChanging else { return nil }
        runningEpochs.insert(epoch)
        isLoading = true
        return epoch
    }
    private func current(_ selected: AppRoute, _ started: UInt64) -> Bool {
        visible && route == selected && epoch == started
    }
    private func finish(_ started: UInt64) {
        runningEpochs.remove(started)
        invalidatedEpochs.remove(started)
        if epoch == started { isLoading = false }
    }
    private func failed(_ error: Error, route selected: AppRoute, epoch started: UInt64) {
        guard current(selected, started) else { return }
        setupMessage = error.localizedDescription
    }

    @discardableResult
    private func invalidateRunningRead(_ selected: AppRoute) -> Bool {
        guard visible, route == selected, !endpointChanging, runningEpochs.contains(epoch) else { return false }
        invalidatedEpochs.insert(epoch)
        return true
    }

    /// Appearance joins an existing read. A mutation revokes its answer and
    /// requests a serial replacement; repeated invalidations share that read.
    private func reload<Value>(
        _ selected: AppRoute, invalidate: Bool = false,
        read: @MainActor () async throws -> Value, publish: @MainActor (Value) -> Void
    ) async {
        if invalidate { invalidateRunningRead(selected) }
        guard let started = begin(selected) else { return }
        defer { finish(started) }
        repeat {
            invalidatedEpochs.remove(started)
            do {
                let value = try await read()
                guard current(selected, started) else { return }
                if invalidatedEpochs.contains(started) { continue }
                publish(value)
                setupMessage = nil
            } catch {
                guard current(selected, started) else { return }
                if invalidatedEpochs.contains(started) { continue }
                failed(error, route: selected, epoch: started)
            }
        } while current(selected, started) && invalidatedEpochs.contains(started)
    }

    func refreshOverview() async {
        await reload(.overview, read: {
            async let status = readStatus()
            async let daily = client.fetchDaily(days: 14)
            return try await (status, daily)
        }, publish: { values in
            self.status = values.0
            self.daily = values.1.days
        })
    }

    func refreshMemories(invalidate: Bool = false) async {
        let selected = route
        guard selected == .memories || selected == .memoryMap else { return }
        await reload(selected, invalidate: invalidate, read: readMemories, publish: { values in
            self.memories = values.results
        })
    }

    func refreshEntities(invalidate: Bool = false) async {
        await reload(.entities, invalidate: invalidate, read: readEntities, publish: { values in
            self.entities = values.results
        })
    }

    private func refreshStatus(_ selected: AppRoute = .manage) async {
        await reload(selected, read: readStatus, publish: { value in
            self.status = value
        })
    }

    func showToast(_ message: String) {
        guard visible else { return }
        toastMessage = message
        Task {
            try? await Task.sleep(for: .seconds(2))
            if toastMessage == message { toastMessage = nil }
        }
    }

}
