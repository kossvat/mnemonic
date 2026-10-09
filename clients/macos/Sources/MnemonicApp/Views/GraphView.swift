import MnemonicShared
import SwiftUI

/// The app owns visibility and invalidation; user actions are the only tasks here.
@MainActor
struct GraphView: View {
    @ObservedObject var controller: AppGraphController
    let onOpenProject: (String) -> Void
    var onDraw: ((CuratedGraphSnapshot) -> Void)? = nil

    var body: some View {
        GraphPage(state: presentation, actions: actions, onDraw: onDraw)
    }

    private var presentation: GraphPageState {
        var state = GraphPageState()
        state.snapshot = controller.snapshot
        state.paintRevision = controller.paintRevision
        state.loading = controller.loading
        state.failure = controller.failure
        state.expandedKey = controller.expandedKey
        state.selectedKey = controller.selectedKey
        state.project = controller.project
        state.projectLoading = controller.projectLoading
        state.projectFailure = controller.projectFailure
        state.selectedEdge = controller.selectedEdge
        state.evidence = controller.evidence
        state.evidenceLoading = controller.evidenceLoading
        state.evidenceFailure = controller.evidenceFailure
        state.evidenceNextOffset = controller.evidenceNextOffset
        state.evidenceDiscardedCount = controller.evidenceDiscardedCount
        state.outside = controller.outside
        state.history = controller.history
        state.mutationInFlight = controller.mutationInFlight
        state.message = controller.message
        state.canUndo = controller.canUndo
        state.canRetryMutation = controller.canRetryMutation
        state.recovery = controller.recovery
        return state
    }

    private var actions: GraphPageActions {
        GraphPageActions(
            refresh: { controller.invalidateSelected(); Task { await controller.refresh() } },
            selectNode: { key in Task { await controller.selectNode(key) } },
            expand: { key in Task { await controller.expand(key) } },
            selectEdge: { index in Task { await controller.selectEdge(index) } },
            moreEvidence: { Task { await controller.loadMoreEvidence() } },
            outside: { reset in Task { await controller.loadOutside(reset: reset) } },
            history: { reset in Task { await controller.loadHistory(reset: reset) } },
            archive: { target, archived in Task { await controller.setArchived(target, archived: archived) } },
            retry: { Task { await controller.retryMutation() } },
            undo: { Task { await controller.undo() } },
            openProject: onOpenProject)
    }
}
