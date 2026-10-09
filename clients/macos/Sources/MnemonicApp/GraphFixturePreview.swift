import AppKit
import MnemonicShared
import SwiftUI

/// Explicit synthetic inputs only; no app model, API or owner store is created.
enum GraphFixturePreview {
    struct Example {
        let name: String
        let state: GraphPageState
        let height: CGFloat
    }

    @MainActor
    static func run(arguments: [String]) throws {
        guard let index = arguments.firstIndex(of: "--render-graph-fixtures"),
              arguments.count > index + 2 else { throw AppFixturePreview.Failure.arguments }
        guard MnemonicProfile.current.isolated else { throw AppFixturePreview.Failure.profile }
        _ = NSApplication.shared
        let directory = URL(fileURLWithPath: arguments[index + 1])
        let output = URL(fileURLWithPath: arguments[index + 2])
        let examples = try load(from: directory)
        let memories: MemoryGraphResponse = try read("memory-map", at: directory)
        try FileManager.default.createDirectory(at: output, withIntermediateDirectories: true)
        for (suffix, scheme) in [("light", ColorScheme.light), ("dark", .dark)] {
            for example in examples {
                try AppFixturePreview.write(
                    GraphPage(state: example.state), route: .graph, scheme: scheme, height: example.height,
                    to: output.appendingPathComponent("graph-\(example.name)-\(suffix).png"))
            }
            try AppFixturePreview.write(
                GraphMemoryFixturePage(graph: memories), route: .memoryMap, scheme: scheme, height: 900,
                to: output.appendingPathComponent("memory-map-\(suffix).png"))
        }
    }

    static func load(from directory: URL) throws -> [Example] {
        let full: CuratedGraphSnapshot = try read("graph-full", at: directory)
        let expanded: CuratedGraphSnapshot = try read("graph-expanded", at: directory)
        let stale: CuratedGraphSnapshot = try read("graph-stale-only", at: directory)
        let empty: CuratedGraphSnapshot = try read("graph-empty", at: directory)
        let project: ProjectDetail = try read("graph-project", at: directory)
        let memories: [CuratedGraphMemory] = try read("graph-evidence", at: directory)
        let outside: CuratedGraphArchivePage = try read("graph-outside", at: directory)
        let history: CuratedGraphArchivePage = try read("graph-history", at: directory)
        var fullState = GraphPageState()
        fullState.snapshot = full
        fullState.selectedKey = full.nodes.first?.key
        fullState.project = project
        var emptyState = GraphPageState()
        emptyState.snapshot = empty
        var errorState = GraphPageState()
        errorState.failure = .unavailable
        var staleState = GraphPageState()
        staleState.snapshot = stale
        var expandedState = GraphPageState()
        expandedState.snapshot = expanded
        expandedState.expandedKey = expanded.nodes.first?.key
        if let link = expanded.links.first(where: { link in link.kind == .uses }) {
            let source = expanded.nodes[link.sourceIndex], target = expanded.nodes[link.targetIndex]
            expandedState.selectedEdge = GraphEdgeSelection(
                sourceKey: source.key, targetKey: target.key, sourceLabel: source.label,
                targetLabel: target.label, kind: link.kind, memoryCount: link.memoryCount)
        }
        expandedState.evidence = memories.map { memory in GraphEvidenceRow(id: memory.id, memory: memory, failure: nil) }
        expandedState.evidenceNextOffset = memories.count
        expandedState.outside = list(outside)
        expandedState.history = list(history)
        expandedState.message = "The archive change could not be confirmed. Retry or undo it."
        expandedState.recovery = GraphMutationRecovery(
            target: .entity(id: "retired", label: "Retired board"), archived: true, confirmed: false)
        expandedState.canUndo = true
        expandedState.canRetryMutation = true
        return [Example(name: "full", state: fullState, height: 1_450),
                Example(name: "empty", state: emptyState, height: 900),
                Example(name: "error", state: errorState, height: 900),
                Example(name: "stale-only", state: staleState, height: 1_050),
                Example(name: "expanded", state: expandedState, height: 1_500)]
    }

    private static func list(_ page: CuratedGraphArchivePage) -> GraphArchiveListState {
        var state = GraphArchiveListState()
        state.entities = page.entities
        state.total = page.total
        state.nextOffset = page.nextOffset
        state.isLoaded = true
        return state
    }

    private static func read<Value: Decodable>(_ name: String, at directory: URL) throws -> Value {
        try JSONDecoder().decode(Value.self,
            from: Data(contentsOf: directory.appendingPathComponent(name + ".json")))
    }
}

/// The Memory Map uses the same scene adapter and painter as its real callsite.
struct GraphMemoryFixturePage: View {
    let graph: MemoryGraphResponse

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            VStack(alignment: .leading, spacing: 5) {
                Text("Memory Map").font(Theme.Font.heading).foregroundStyle(Theme.Palette.textPrimary)
                Text(verbatim: "\(graph.nodes.count) memories · \(graph.edges.count) shared links")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
            .padding(Theme.Space.lg)
            Divider().overlay(Theme.Palette.border)
            StaticGraphCanvas(scene: .memoryMap(nodes: graph.nodes, edges: graph.edges),
                              selectedNode: nil, onNode: { _ in }, onEdge: { _ in })
                .frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background(Theme.Palette.bgPrimary)
    }
}
