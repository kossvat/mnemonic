import Combine
import Foundation
import MnemonicShared

/// The menu bar uses the shared signal coordinator, never a second poll.
@MainActor
final class WidgetLiveController: ObservableObject {
    let store: LiveStore
    let coordinator: LiveCoordinator
    @Published private(set) var visible = false
    @Published private(set) var now = Date()
    private(set) var page = 0
    private(set) var title = StatusTitle()
    var onTitle: ((StatusTitle) -> Void)?
    private var subscriber: LiveSignalSubscriber?
    private var failure: LocalAPIFailure?

    init(clock: LiveClock, ask: @escaping @Sendable () async -> LiveAnswer) {
        let store = LiveStore()
        self.store = store
        coordinator = LiveCoordinator(store: store, clock: clock, ask: ask)
        coordinator.onAnswer = { [weak self] answer in self?.received(answer) }
        coordinator.onLabelsTick = { [weak self] in
            guard let self, self.visible else { return }
            self.now = Date()
        }
    }

    convenience init(profile: MnemonicProfile) {
        let api = LocalAPI(profile: profile)
        self.init(clock: SystemLiveClock()) { await api.live() }
        if let name = try? LiveSignal.name(of: profile) {
            subscriber = LiveSignalSubscriber(name: name) { [weak self] in
                Task { @MainActor in await self?.coordinator.signalled() }
            }
        }
    }

    func start() async { await coordinator.request() }

    func hide() {
        if visible { visible = false }
        coordinator.hide()
    }

    func setVisible(_ shown: Bool) async {
        if !shown { hide(); return }
        if visible != shown { visible = shown }
        if shown { now = Date() }
        await coordinator.setSurfaceVisible(shown)
        // Opening may finish after a later close; use the current state.
        coordinator.setPlanVisible(visible && page == 0)
    }

    func selectPage(_ selected: Int) {
        page = selected
        coordinator.setPlanVisible(visible && selected == 0)
    }

    /// Offline preview input. It subscribes to no signal and asks nothing.
    func present(_ answer: LiveAnswer, at date: Date) {
        visible = true
        now = date
        store.visible = true
        store.take(answer)
        received(answer)
    }

    private func received(_ answer: LiveAnswer) {
        switch answer {
        case .failed(let reason): failure = reason
        case .snapshot, .unchanged: failure = nil
        }
        let next = StatusTitle(live: store.snapshot, failure: failure)
        if next != title {
            title = next
            onTitle?(next)
        }
    }
}

extension LocalAPIFailure {
    var widgetTitle: String {
        switch self {
        case .off: return "Local API is off"
        case .unreachable: return "Daemon is unavailable"
        case .unauthorised: return "Access was not authorised"
        case .earlierDaemon: return "Daemon update needed"
        case .undecodable: return "Live response could not be read"
        }
    }

    var widgetDetail: String {
        switch self {
        case .off: return "Enable the local API in this profile to see live projects."
        case .unreachable: return "Live projects will return when the daemon is reachable."
        case .unauthorised: return "The profile token was refused. Check this profile's connection."
        case .earlierDaemon: return "This daemon does not yet provide live project data."
        case .undecodable: return "Live activity is hidden until a valid response arrives."
        }
    }
}
