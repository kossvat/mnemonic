import MnemonicShared
import SwiftUI

/// Pure presentation shared by the real screen and explicit synthetic renders.
@MainActor
struct GraphPage: View {
    let state: GraphPageState
    var actions = GraphPageActions()
    var onDraw: ((CuratedGraphSnapshot) -> Void)? = nil
    @State private var viewportState = StaticGraphViewportState()

    var body: some View {
        VStack(spacing: 0) {
            header
            if state.message != nil || state.canUndo || state.canRetryMutation || state.mutationInFlight { recovery }
            Divider().overlay(Theme.Palette.border)
            HStack(alignment: .top, spacing: 0) {
                VStack(spacing: 0) {
                    if let failure = state.failure { failureNotice(failure) }
                    canvas.frame(maxWidth: .infinity, maxHeight: .infinity)
                    legend
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                Divider().overlay(Theme.Palette.border)
                GraphSidePanel(state: state, actions: actions).frame(width: 320)
            }
        }
        .background(Theme.Palette.bgPrimary)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: Theme.Space.lg) {
            VStack(alignment: .leading, spacing: 5) {
                Text("Project Graph").font(Theme.Font.heading).foregroundStyle(Theme.Palette.textPrimary)
                HStack(spacing: 6) {
                    Text(verbatim: focusLabel)
                        .font(Theme.Font.captionBold)
                    if let snapshot = state.snapshot {
                        Text(verbatim: "· \(snapshot.nodes.count) nodes · \(snapshot.links.count) links")
                            .font(Theme.Font.caption)
                    }
                }
                .foregroundStyle(Theme.Palette.textMuted)
                if let snapshot = state.snapshot, let omitted = GraphPresentation.omissions(snapshot.omitted) {
                    Text(verbatim: omitted).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                }
            }
            Spacer(minLength: 8)
            if state.loading { ProgressView().controlSize(.small).padding(.top, 5) }
            if state.expandedKey != nil {
                Button { actions.expand(nil) } label: {
                    Label("Overview", systemImage: "arrow.uturn.backward")
                }.buttonStyle(SubtleButtonStyle())
            }
            Button(action: actions.refresh) { Label("Refresh", systemImage: "arrow.clockwise") }
                .buttonStyle(SubtleButtonStyle()).disabled(state.loading)
        }
        .padding(Theme.Space.lg)
    }

    private var recovery: some View {
        HStack(alignment: .center, spacing: Theme.Space.md) {
            if state.mutationInFlight { ProgressView().controlSize(.small) }
            Text(verbatim: state.message ?? (state.mutationInFlight ? "Saving archive change" : "An archive action is available to undo."))
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textPrimary)
                .frame(maxWidth: .infinity, alignment: .leading)
            if state.canRetryMutation {
                Button("Retry", action: actions.retry).buttonStyle(SubtleButtonStyle())
            }
            if state.canUndo {
                Button("Undo", action: actions.undo).buttonStyle(SubtleButtonStyle())
            }
        }
        .disabled(state.mutationInFlight)
        .padding(.horizontal, Theme.Space.lg).padding(.vertical, Theme.Space.sm)
        .background(Theme.Palette.bgTint)
    }

    private func failureNotice(_ failure: CuratedGraphFailure) -> some View {
        ProjectNotice(icon: "exclamationmark.circle", title: "Graph unavailable", detail: failure.message)
            .padding(Theme.Space.lg)
    }

    @ViewBuilder private var canvas: some View {
        if let snapshot = state.snapshot, !snapshot.nodes.isEmpty, state.failure == nil {
            StaticGraphCanvas(scene: .curated(snapshot, focusKey: state.expandedKey),
                              selectedNode: state.selectedKey, onNode: actions.selectNode,
                              onEdge: actions.selectEdge, onDraw: { onDraw?(snapshot) },
                              viewportState: viewportState, paintRevision: state.paintRevision)
                .accessibilityLabel("Project graph. Select a node for its plan or a link for supporting memories.")
        } else {
            VStack(spacing: Theme.Space.md) {
                Image(systemName: state.loading ? "hourglass" : "circle.hexagongrid")
                    .font(.system(size: 34, weight: .light)).foregroundStyle(Theme.Palette.textMuted)
                Text(state.loading ? "Loading graph" : state.failure == nil ? "No nodes in this view" : "Graph not available")
                    .font(Theme.Font.title).foregroundStyle(Theme.Palette.textPrimary)
                Text(state.failure == nil
                     ? (state.staleProjects.isEmpty ? "Project activity will appear here when it is available."
                        : "Older projects are listed alongside. Select one to inspect or expand it.")
                     : "Refresh to request the graph again.")
                    .font(Theme.Font.body).foregroundStyle(Theme.Palette.textMuted)
                    .multilineTextAlignment(.center).frame(maxWidth: 320)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity).padding(Theme.Space.xl)
        }
    }

    private var legend: some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            HStack(spacing: Theme.Space.md) {
                ForEach([CuratedGraphNodeKind.project, .part, .unit, .technology, .person], id: \.rawValue) { kind in
                    HStack(spacing: 4) {
                        Image(systemName: GraphPresentation.symbol(kind)).foregroundStyle(GraphPresentation.color(kind))
                            .font(.system(size: 9)).frame(width: 10)
                        Text(verbatim: GraphPresentation.kind(kind)).font(Theme.Font.caption)
                    }
                }
            }
            Text("Tone: active within 7 days · quiet under 30 days · stale or activity unknown")
                .font(Theme.Font.caption)
            Text("Size: agent hours over 30 days. Select a node to see owner and agent time separately.")
                .font(Theme.Font.caption)
        }
        .foregroundStyle(Theme.Palette.textMuted)
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(Theme.Space.lg).background(Theme.Palette.bgSurface)
    }

    private var focusLabel: String {
        guard let key = state.expandedKey else { return "Overview" }
        let name = state.snapshot?.nodes.first { node in node.key == key }?.label
        return name.map { label in "Expanded: " + label } ?? "Expanded view"
    }
}
