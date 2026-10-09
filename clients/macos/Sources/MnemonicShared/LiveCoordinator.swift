import Foundation

/// A one-shot timer. Cancellation also revokes a callback queued for delivery.
public protocol LiveTimer: AnyObject {
    func cancel()
}

/// A clock may have queued its callback already when the owner cancels.
/// Only one delivery can claim this lease, and cancellation revokes it.
private final class LiveTimerLease: @unchecked Sendable {
    private let lock = NSLock()
    private var available = true

    func claim() -> Bool {
        lock.withLock {
            guard available else { return false }
            available = false
            return true
        }
    }

    func cancel() { lock.withLock { available = false } }
}

/// The coordinator also guards injected clocks, including queued callbacks
/// they cannot remove. An old delivery cannot clear a replacement's slot.
private final class LeasedLiveTimer: LiveTimer {
    private let timer: LiveTimer
    private let lease: LiveTimerLease

    init(timer: LiveTimer, lease: LiveTimerLease) {
        self.timer = timer
        self.lease = lease
    }

    func cancel() {
        lease.cancel()
        timer.cancel()
    }

    deinit { cancel() }
}

/// What arms timers. The tests stand in for it and hold the clock.
@MainActor
public protocol LiveClock: AnyObject {
    func arm(
        after seconds: TimeInterval, tolerance: TimeInterval,
        _ fire: @escaping @MainActor () -> Void
    ) -> LiveTimer
}

/// Decides when a client asks the daemon for the live snapshot, and holds
/// the only repeating timers a client may hold (design 11.4):
///
/// - it asks on the signal, when the popover opens, when a window becomes
///   visible;
/// - a safety request every 300 s, only while the last snapshot held a
///   live session, also while everything is closed: a daemon that died
///   posts nothing, and a project must not stay live on screen;
/// - a refresh every 30 s, only while a plan surface is visible: an edit
///   of a plan makes no transcript grow, so nothing announces it;
/// - a tick every 60 s for the elapsed labels, only while a surface is
///   visible and a session is live.
///
/// With nothing visible and no live session it holds no timer at all.
@MainActor
public final class LiveCoordinator {
    public static let safety: (every: TimeInterval, tolerance: TimeInterval) = (300, 60)
    public static let refresh: (every: TimeInterval, tolerance: TimeInterval) = (30, 10)
    public static let labels: (every: TimeInterval, tolerance: TimeInterval) = (60, 15)

    private let store: LiveStore
    private let ask: @Sendable () async -> LiveAnswer
    private let clock: LiveClock
    /// Called on the tick of the elapsed labels.
    public var onLabelsTick: (@MainActor () -> Void)?
    /// Non-published notification for status items and visible route loaders.
    public var onAnswer: (@MainActor (LiveAnswer) -> Void)?
    /// The visible surface may refresh its own route on the shared tick.
    public var onRefreshTick: (@MainActor () -> Void)?
    private let allowHiddenRequests: Bool

    private var safetyTimer: LiveTimer?
    private var refreshTimer: LiveTimer?
    private var labelsTimer: LiveTimer?
    private var asking = false
    private var askAgain = false
    private var visibleGeneration: UInt64 = 0

    public init(
        store: LiveStore, clock: LiveClock, allowHiddenRequests: Bool = true,
        ask: @escaping @Sendable () async -> LiveAnswer
    ) {
        self.store = store
        self.clock = clock
        self.ask = ask
        self.allowHiddenRequests = allowHiddenRequests
    }

    /// A surface that shows live projects is on screen: the popover, or a
    /// window of the app.
    public private(set) var surfaceVisible = false
    /// A surface that shows a plan is on screen: the board, a project,
    /// today's timeline.
    public private(set) var planVisible = false

    /// The timers held now, by name: what the tests look at.
    public var armed: Set<String> {
        var names: Set<String> = []
        if safetyTimer != nil { names.insert("safety") }
        if refreshTimer != nil { names.insert("refresh") }
        if labelsTimer != nil { names.insert("labels") }
        return names
    }

    /// The daemon said the snapshot changed.
    public func signalled() async { await request() }

    /// Hide synchronously so an in-flight reply cannot publish after close.
    public func hide() {
        visibleGeneration &+= 1
        surfaceVisible = false
        planVisible = false
        store.visible = false
        rearm()
    }

    /// A surface became visible or was closed. Becoming visible asks.
    public func setSurfaceVisible(_ visible: Bool) async {
        if !visible { hide(); return }
        let opened = visible && !surfaceVisible
        if opened { visibleGeneration &+= 1 }
        surfaceVisible = visible
        if !visible { planVisible = false }
        store.visible = visible
        if opened { await request() } else { rearm() }
    }

    /// A plan surface became visible or was left.
    public func setPlanVisible(_ visible: Bool) {
        planVisible = visible && surfaceVisible
        rearm()
    }

    /// Ask the daemon, one request at a time: a wish to ask that comes
    /// while a request is out is one more request after it, not many.
    public func request() async {
        guard surfaceVisible || allowHiddenRequests else { return }
        if asking {
            askAgain = true
            return
        }
        asking = true
        repeat {
            askAgain = false
            let generation = visibleGeneration
            let answer = await ask()
            let deliver = allowHiddenRequests || (surfaceVisible && generation == visibleGeneration)
            // Keep backing aligned with the API's ETag cache. A current
            // unchanged answer can then publish a stale visit's held value.
            store.take(answer, publish: deliver)
            if deliver { onAnswer?(answer) }
        } while askAgain && (surfaceVisible || allowHiddenRequests)
        asking = false
        rearm()
    }

    /// Hold exactly the timers the state now calls for.
    private func rearm() {
        hold(&safetyTimer, when: store.hasLiveSession && (surfaceVisible || allowHiddenRequests), Self.safety) { [weak self] in
            guard let self else { return }
            self.safetyTimer = nil
            let generation = self.visibleGeneration
            Task { [weak self] in
                guard let self, self.store.hasLiveSession,
                      self.allowHiddenRequests || generation == self.visibleGeneration else { return }
                await self.request()
            }
        }
        hold(&refreshTimer, when: planVisible, Self.refresh) { [weak self] in
            guard let self else { return }
            self.refreshTimer = nil
            let generation = self.visibleGeneration
            Task { [weak self] in
                // Closing can happen after delivery but before this task.
                guard let self, self.planVisible, generation == self.visibleGeneration else { return }
                self.onRefreshTick?()
                await self.request()
            }
        }
        hold(&labelsTimer, when: surfaceVisible && store.hasLiveSession, Self.labels) {
            [weak self] in
            self?.labelsTimer = nil
            self?.onLabelsTick?()
            self?.rearm()
        }
    }

    private func hold(
        _ timer: inout LiveTimer?, when wanted: Bool,
        _ pace: (every: TimeInterval, tolerance: TimeInterval),
        _ fire: @escaping @MainActor () -> Void
    ) {
        if wanted {
            if timer == nil {
                let lease = LiveTimerLease()
                let scheduled = clock.arm(after: pace.every, tolerance: pace.tolerance) {
                    guard lease.claim() else { return }
                    fire()
                }
                timer = LeasedLiveTimer(timer: scheduled, lease: lease)
            }
        } else {
            timer?.cancel()
            timer = nil
        }
    }
}

/// Timers of the system, with their tolerance: the system may fire them
/// together with others, and wakes the process less.
@MainActor
public final class SystemLiveClock: LiveClock {
    public init() {}

    private final class Armed: LiveTimer {
        let timer: Timer
        let lease: LiveTimerLease

        init(_ timer: Timer, lease: LiveTimerLease) {
            self.timer = timer
            self.lease = lease
        }

        func cancel() {
            lease.cancel()
            timer.invalidate()
        }

        deinit { cancel() }
    }

    public func arm(
        after seconds: TimeInterval, tolerance: TimeInterval,
        _ fire: @escaping @MainActor () -> Void
    ) -> LiveTimer {
        let lease = LiveTimerLease()
        let timer = Timer(timeInterval: seconds, repeats: false) { _ in
            Task { @MainActor in
                guard lease.claim() else { return }
                fire()
            }
        }
        timer.tolerance = tolerance
        RunLoop.main.add(timer, forMode: .common)
        return Armed(timer, lease: lease)
    }
}
