import Foundation
import MnemonicShared

extension AppGraphController {
    @discardableResult
    func setArchived(_ target: GraphArchiveTarget, archived: Bool) async -> Bool {
        guard visible, !mutationInFlight else { return false }
        let origin = endpoint
        let reloadOutside = outside.isLoaded, reloadHistory = history.isLoaded
        // Revoke reads before sending: even a failed POST may have committed.
        visit &+= 1
        mutation &+= 1
        let operation = mutation
        mutationInFlight = true
        loading = false
        clearSnapshot()
        failure = nil
        message = nil
        clearSelection()
        invalidateLists(clear: true)
        changed()
        let answer = await requests.mutate(target, archived)
        guard endpoint == origin, mutation == operation else { return false }
        mutationInFlight = false
        let confirmed: Bool
        switch answer {
        case .success:
            confirmed = true
            message = archived ? "Archived. You can undo this change." : "Restored. You can undo this change."
        case .failure(let reason):
            confirmed = false
            message = "The change could not be confirmed and may have been saved. "
                + reason.message + " Retry or undo this change."
        }
        recovery = GraphMutationRecovery(target: target, archived: archived, confirmed: confirmed)
        changed()
        // A reopened graph needs the completed operation's current state.
        // Hidden responses keep recovery controls without follow-up reads.
        guard visible else { return confirmed }
        let started = visit
        await refresh()
        guard endpoint == origin, mutation == operation else { return false }
        guard current(started) else { return confirmed }
        let refreshed = failure == nil && snapshot != nil
        if confirmed && !refreshed {
            recovery = GraphMutationRecovery(target: target, archived: archived, confirmed: false)
            message = "The change was saved, but the refreshed graph could not be confirmed. "
                + "Retry or undo this change."
            changed()
        }
        if reloadOutside { await loadOutside(reset: true) }
        guard endpoint == origin, mutation == operation else { return false }
        guard current(started) else { return confirmed && refreshed }
        if reloadHistory { await loadHistory(reset: true) }
        return endpoint == origin && mutation == operation && confirmed && refreshed
    }

    @discardableResult
    func retryMutation() async -> Bool {
        guard let recovery, !recovery.confirmed else { return false }
        return await setArchived(recovery.target, archived: recovery.archived)
    }

    @discardableResult
    func undo() async -> Bool {
        guard let recovery else { return false }
        return await setArchived(recovery.target, archived: !recovery.archived)
    }
}
