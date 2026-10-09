import Foundation
import MnemonicShared

/// Only the isolated native fixture writes this marker. Equal states do not
/// touch it, so the sampler can detect a hide and reopen during its window.
@MainActor
final class FixtureWindowState {
    private let profile: MnemonicProfile
    private var last: Data?
    init(profile: MnemonicProfile) { self.profile = profile }

    func update(model: MnemonicAppModel) {
        guard profile.isolated else { return }
        // Resolve the existing parent before appending the absent marker: the
        // system can spell an existing temporary root differently from a new leaf.
        let url = profile.home.resolvingSymlinksInPath().appendingPathComponent("fixture-window.state")
        guard profile.contains(url) else { return }
        let expected: Set<String> = ["fixture-00", "fixture-01"]
        let board = Set((model.projects.board?.projects ?? []).filter { row in
            row.plan?.done == 1 && row.plan?.total == 2
        }.map(\.key))
        let live = Set((model.projects.live.shown?.projects ?? []).filter { row in
            !row.sessions.isEmpty && row.plan?.done == 1 && row.plan?.total == 2
        }.map(\.key))
        let visible = model.visible
        let ready = visible && model.route == .projects && model.projects.failure == nil
            && model.projects.live.failure == nil
            && expected.isSubset(of: board) && expected.isSubset(of: live)
        var value: [String: Any] = ["visible": visible, "route": model.route.rawValue, "ready": ready]
        if model.route == .graph {
            let snapshot = model.curatedGraph.snapshot
            let painted = snapshot != nil && snapshot == model.graphDrawnSnapshot
            let graphReady = visible && painted && !model.curatedGraph.loading
                && model.curatedGraph.failure == nil && Self.graphContainsFixture(snapshot)
            value = ["visible": visible, "route": "graph", "ready": graphReady, "painted": painted]
        }
        guard let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]), data != last else { return }
        do {
            try data.write(to: url, options: .atomic)
            last = data
        } catch { /* A missing marker blocks the sampler. */ }
    }

    static func graphContainsFixture(_ snapshot: CuratedGraphSnapshot?) -> Bool {
        guard let snapshot, !snapshot.nodes.isEmpty else { return false }
        return ["fixture-00", "fixture-01"].allSatisfy { slug in
            snapshot.nodes.enumerated().contains { index, node in
                node.kind == .project && node.projectSlug == slug && snapshot.links.contains { link in
                    link.kind == .contains && link.sourceIndex == index
                        && snapshot.nodes[link.targetIndex].kind == .unit
                }
            }
        }
    }
}
