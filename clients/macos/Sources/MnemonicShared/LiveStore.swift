import Combine
import Foundation

/// The live snapshot as a client shows it. What is asked of the daemon is
/// kept in a backing value; the published value, which redraws a view, is
/// set only while the surface is visible and only when it differs.
@MainActor
public final class LiveStore: ObservableObject {
    @Published public private(set) var shown: LiveSnapshot?
    @Published public private(set) var failure: LocalAPIFailure?

    /// What the daemon last said, whether shown or not.
    public private(set) var snapshot: LiveSnapshot?
    private var failed: LocalAPIFailure?
    private var publicationAllowed = true
    /// How many times a published value was set: what the tests count.
    public private(set) var publications = 0

    public init() {}

    /// Whether a surface that shows the snapshot is on screen.
    public var visible = false {
        didSet { if visible && publicationAllowed { publish() } }
    }

    /// Whether the last snapshot held a live session.
    public var hasLiveSession: Bool { snapshot?.hasLiveSession ?? false }

    /// Take the answer of a request. By default a visible store publishes
    /// it immediately. With publish false only backing changes, even after
    /// reopening, until a later publish-enabled answer confirms that value.
    public func take(_ answer: LiveAnswer, publish: Bool = true) {
        publicationAllowed = publish
        switch answer {
        case .snapshot(let taken):
            snapshot = taken
            failed = nil
        case .unchanged:
            failed = nil
        case .failed(let why):
            failed = why
            // A daemon that is gone has no live session: a project must
            // not stay live on screen.
            snapshot = nil
        }
        if visible && publish { self.publish() }
    }

    private func publish() {
        if shown != snapshot {
            shown = snapshot
            publications += 1
        }
        if failure != failed {
            failure = failed
            publications += 1
        }
    }
}
