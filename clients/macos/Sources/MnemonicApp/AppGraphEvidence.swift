import Foundation
import MnemonicShared

extension AppGraphController {
    func selectEdge(_ index: Int) async {
        guard visible, !mutationInFlight, let snapshot, snapshot.links.indices.contains(index) else { return }
        let link = snapshot.links[index]
        let source = snapshot.nodes[link.sourceIndex], target = snapshot.nodes[link.targetIndex]
        clearSelection()
        selectedEdge = GraphEdgeSelection(sourceKey: source.key, targetKey: target.key,
                                          sourceLabel: source.label, targetLabel: target.label,
                                          kind: link.kind, memoryCount: link.memoryCount)
        evidenceNextOffset = 0
        changed()
        await loadMoreEvidence()
    }

    func loadMoreEvidence() async {
        guard visible, !mutationInFlight, !evidenceLoading, let edge = selectedEdge,
              let offset = evidenceNextOffset else { return }
        let started = visit, selected = selection
        evidenceLoading = true
        evidenceFailure = nil
        changed()
        defer {
            if selectionCurrent(started, selected) { evidenceLoading = false; changed() }
        }
        let answer = await requests.evidence(edge, offset, 10)
        guard selectionCurrent(started, selected), selectedEdge == edge else { return }
        switch answer {
        case .failure(let reason): evidenceFailure = reason; changed()
        case .success(let page):
            // Publish each bounded read, but never issue the next read after a
            // selection, visibility, focus or endpoint change during an await.
            for id in page.memoryIDs {
                guard selectionCurrent(started, selected), selectedEdge == edge else { return }
                let memory = await requests.memory(id)
                guard selectionCurrent(started, selected), selectedEdge == edge else { return }
                let row: GraphEvidenceRow
                switch memory {
                case .success(let value): row = .init(id: id, memory: value, failure: nil)
                case .failure(let reason): row = .init(id: id, memory: nil, failure: reason)
                }
                if let index = evidence.firstIndex(where: { item in item.id == id }) { evidence[index] = row }
                else { evidence.append(row) }
                if evidence.count > 30 {
                    evidence.removeFirst(evidence.count - 30)
                    evidenceDiscardedCount += 1
                }
                changed()
            }
            evidenceNextOffset = page.nextOffset
            changed()
        }
    }

    func loadOutside(reset: Bool = false) async { await loadArchives(archived: false, reset: reset) }
    func loadHistory(reset: Bool = false) async { await loadArchives(archived: true, reset: reset) }

    private func loadArchives(archived: Bool, reset: Bool) async {
        guard visible, !mutationInFlight else { return }
        if reset {
            if archived { historyGeneration &+= 1; history = GraphArchiveListState() }
            else { outsideGeneration &+= 1; outside = GraphArchiveListState() }
        }
        var state = archived ? history : outside
        guard !state.loading, !state.isLoaded || state.nextOffset != nil else { return }
        let offset = state.isLoaded ? state.nextOffset! : 0
        let started = visit, generation = archived ? historyGeneration : outsideGeneration
        state.loading = true
        state.failure = nil
        if archived { history = state } else { outside = state }
        changed()
        let answer = await requests.archives(archived, offset, 20)
        guard current(started), !mutationInFlight,
              generation == (archived ? historyGeneration : outsideGeneration) else { return }
        state = archived ? history : outside
        state.loading = false
        switch answer {
        case .failure(let reason): state.failure = reason
        case .success(let page):
            for row in page.entities {
                if let index = state.entities.firstIndex(where: { item in item.id == row.id }) {
                    state.entities[index] = row
                } else { state.entities.append(row) }
            }
            if state.entities.count > 100 {
                let discarded = state.entities.count - 100
                state.entities.removeFirst(discarded)
                state.discardedCount += discarded
            }
            state.isLoaded = true
            state.total = page.total
            state.nextOffset = page.nextOffset
            state.failure = nil
        }
        if archived { history = state } else { outside = state }
        changed()
    }
}
