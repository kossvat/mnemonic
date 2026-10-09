import Foundation
import SwiftUI
import MnemonicShared

/// All graph reads follow explicit app context or user actions. No clock lives here.
@MainActor
final class AppGraphController: ObservableObject {
    @Published var snapshot: CuratedGraphSnapshot?
    @Published private(set) var paintRevision: UInt64 = 0
    @Published var loading = false
    @Published var failure: CuratedGraphFailure?
    @Published var expandedKey: String?
    @Published var selectedKey: String?
    @Published var project: ProjectDetail?
    @Published var projectLoading = false
    @Published var projectFailure: CuratedGraphFailure?
    @Published var selectedEdge: GraphEdgeSelection?
    @Published var evidence: [GraphEvidenceRow] = []
    @Published var evidenceLoading = false
    @Published var evidenceFailure: CuratedGraphFailure?
    @Published var evidenceNextOffset: Int?
    @Published var evidenceDiscardedCount = 0
    @Published var outside = GraphArchiveListState()
    @Published var history = GraphArchiveListState()
    @Published var mutationInFlight = false
    @Published var message: String?
    @Published var recovery: GraphMutationRecovery?
    var canUndo: Bool { recovery != nil }
    var canRetryMutation: Bool { recovery?.confirmed == false }
    var onPresentationChange: (@MainActor () -> Void)?

    let requests: GraphRequests
    private(set) var visible = false
    var visit: UInt64 = 0
    var endpoint: UInt64 = 0
    var selection: UInt64 = 0
    var outsideGeneration: UInt64 = 0
    var historyGeneration: UInt64 = 0
    var mutation: UInt64 = 0
    var running: Set<UInt64> = []
    var invalidated: Set<UInt64> = []

    init(requests: GraphRequests) { self.requests = requests }
    convenience init(api: LocalAPI) { self.init(requests: GraphRequests(api: api)) }

    func setContext(visible shown: Bool) {
        guard shown != visible else { return }
        visit &+= 1
        visible = shown
        loading = false
        failure = nil
        clearSnapshot()
        clearSelection()
        invalidateLists(clear: false)
        changed()
    }

    func resetEndpoint() {
        endpoint &+= 1
        visit &+= 1
        mutation &+= 1
        expandedKey = nil
        clearSnapshot()
        failure = nil
        loading = false
        mutationInFlight = false
        recovery = nil
        message = nil
        clearSelection()
        invalidateLists(clear: true)
        changed()
    }

    func invalidateSelected() {
        if running.contains(visit) { invalidated.insert(visit) }
        clearSnapshot()
        clearSelection()
        invalidateLists(clear: true)
        changed()
    }

    /// Called synchronously before the app awaits a live health response.
    @discardableResult
    func invalidateRunningRead() -> Bool {
        guard visible, running.contains(visit) else { return false }
        invalidated.insert(visit)
        clearSnapshot()
        clearSelection()
        changed()
        return true
    }

    /// A removed canvas must repaint even when its replacement data is equal.
    func clearSnapshot() {
        paintRevision &+= 1
        snapshot = nil
    }

    func changed() { onPresentationChange?() }
    func current(_ started: UInt64) -> Bool { visible && visit == started }
    func selectionCurrent(_ started: UInt64, _ selected: UInt64) -> Bool {
        current(started) && selection == selected && !mutationInFlight
    }

    func clearSelection() {
        selection &+= 1
        selectedKey = nil
        project = nil
        projectLoading = false
        projectFailure = nil
        selectedEdge = nil
        evidence = []
        evidenceLoading = false
        evidenceFailure = nil
        evidenceNextOffset = nil
        evidenceDiscardedCount = 0
    }

    func invalidateLists(clear: Bool) {
        outsideGeneration &+= 1
        historyGeneration &+= 1
        if clear {
            outside = GraphArchiveListState()
            history = GraphArchiveListState()
        } else {
            outside.loading = false
            history.loading = false
        }
    }
}
