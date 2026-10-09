import MnemonicShared
import SwiftUI

struct GraphSelectionPanel: View {
    let state: GraphPageState
    let actions: GraphPageActions

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.lg) {
            if let edge = state.selectedEdge {
                GraphEvidencePanel(state: state, edge: edge, actions: actions)
            } else if state.selectedKey != nil {
                selection
            } else {
                ProjectNotice(icon: "cursorarrow", title: "Inspect the graph",
                              detail: "Select a project for its plan, or a relationship for the memories supporting it.")
            }
        }
    }

    @ViewBuilder private var selection: some View {
        HStack {
            Text("Selected").font(Theme.Font.captionBold).foregroundStyle(Theme.Palette.textMuted)
            Spacer()
            Button { actions.selectNode(nil) } label: { Image(systemName: "xmark") }
                .buttonStyle(.plain).accessibilityLabel("Close selection")
        }
        if let node = state.selectedNode {
            Text(verbatim: node.label).font(Theme.Font.heading).foregroundStyle(Theme.Palette.textPrimary)
            HStack(spacing: 6) {
                ProjectPill(text: GraphPresentation.kind(node.kind), color: GraphPresentation.color(node.kind))
                ProjectPill(text: node.isClosed ? "Closed" : GraphPresentation.state(node.state))
            }
            Text(verbatim: GraphPresentation.activity(node.inactiveDays))
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            HStack(alignment: .top, spacing: Theme.Space.xl) {
                hours("You · 30 days", value: node.hours30Days.ownerTenths)
                hours("Agents · 30 days", value: node.hours30Days.agentTenths)
            }
            if node.hours30Days.ownerTenths == nil || node.hours30Days.agentTenths == nil {
                Text("Unavailable means this measure is unknown, not zero.")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
            Button("Expand around this node") { actions.expand(node.key) }.buttonStyle(SubtleButtonStyle())
        } else if let shelf = state.selectedShelf {
            Text(verbatim: shelf.label).font(Theme.Font.heading).foregroundStyle(Theme.Palette.textPrimary)
            Text(verbatim: "\(shelf.memoryCount) memories · \(GraphPresentation.activity(shelf.inactiveDays))")
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            if shelf.reason == .staleProject {
                Button("Expand project") { actions.expand(shelf.key) }.buttonStyle(SubtleButtonStyle())
            }
        } else if let item = (state.outside.entities + state.history.entities).first(where: { item in
            "entity:" + item.id == state.selectedKey
        }) {
            Text(verbatim: item.name).font(Theme.Font.heading).foregroundStyle(Theme.Palette.textPrimary)
            Text(verbatim: "\(item.memoryCount) memories · \(GraphPresentation.date(item.lastMemory))")
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            Button(item.archivedAt == nil ? "Archive entity" : "Restore entity") {
                actions.archive(.entity(id: item.id, label: item.name), item.archivedAt == nil)
            }
            .buttonStyle(SubtleButtonStyle()).disabled(state.mutationInFlight)
        }
        if let slug = state.selectedProjectSlug {
            Button { actions.openProject(slug) } label: { Label("Open full project", systemImage: "arrow.up.right") }
                .buttonStyle(SubtleButtonStyle())
            GraphProjectPanel(project: state.project, loading: state.projectLoading, failure: state.projectFailure)
        }
        relationships
    }

    private func hours(_ label: String, value: UInt32?) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Text(verbatim: label).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            Text(verbatim: GraphPresentation.hours(value)).font(Theme.Font.title).monospacedDigit()
                .foregroundStyle(Theme.Palette.textPrimary)
        }
    }

    @ViewBuilder private var relationships: some View {
        if let snapshot = state.snapshot, let key = state.selectedKey {
            let links = snapshot.links.enumerated().filter { _, link in
                snapshot.nodes[link.sourceIndex].key == key || snapshot.nodes[link.targetIndex].key == key
            }
            if !links.isEmpty {
                Text("Relationships").font(Theme.Font.title).foregroundStyle(Theme.Palette.textPrimary)
                ForEach(Array(links), id: \.offset) { index, link in
                    let source = snapshot.nodes[link.sourceIndex]
                    let target = snapshot.nodes[link.targetIndex]
                    Button { actions.selectEdge(index) } label: {
                        VStack(alignment: .leading, spacing: 4) {
                            Text(verbatim: "\(source.label) · \(GraphPresentation.relationship(link.kind).lowercased()) · \(target.label)")
                                .font(Theme.Font.body).foregroundStyle(Theme.Palette.textPrimary)
                            Text(verbatim: "\(link.memoryCount) supporting memories")
                                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                        }
                        .frame(maxWidth: .infinity, alignment: .leading).padding(.vertical, 4)
                    }.buttonStyle(.plain)
                }
            }
        }
    }
}

struct GraphEvidencePanel: View {
    let state: GraphPageState
    let edge: GraphEdgeSelection
    let actions: GraphPageActions

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            HStack {
                Text("Supporting memories").font(Theme.Font.title).foregroundStyle(Theme.Palette.textPrimary)
                Spacer()
                Button { actions.selectNode(nil) } label: { Image(systemName: "xmark") }
                    .buttonStyle(.plain).accessibilityLabel("Close supporting memories")
            }
            Text(verbatim: "\(edge.sourceLabel) · \(GraphPresentation.relationship(edge.kind).lowercased()) · \(edge.targetLabel)")
                .font(Theme.Font.body).foregroundStyle(Theme.Palette.textPrimary)
            Text(verbatim: "\(edge.memoryCount) supporting memories recorded")
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            if let failure = state.evidenceFailure {
                Text(verbatim: failure.message).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                if let index = edgeIndex { Button("Retry evidence") { actions.selectEdge(index) }.buttonStyle(SubtleButtonStyle()) }
            }
            ForEach(state.evidence, id: \.id) { row in
                VStack(alignment: .leading, spacing: 6) {
                    if let memory = row.memory {
                        Text(verbatim: memory.title).font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary)
                        Text(verbatim: memory.content).font(Theme.Font.body).foregroundStyle(Theme.Palette.textMuted)
                            .lineLimit(5).textSelection(.enabled)
                        DisclosureGroup("Read full memory") {
                            Text(verbatim: memory.content).font(Theme.Font.body).foregroundStyle(Theme.Palette.textPrimary)
                                .textSelection(.enabled).padding(.top, 6)
                            Text(verbatim: memory.id).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                                .textSelection(.enabled)
                        }
                        .font(Theme.Font.caption)
                        Text(verbatim: ProjectPresentation.title(memory.memoryType))
                            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    } else {
                        Text("Memory unavailable").font(Theme.Font.bodyMedium).foregroundStyle(Theme.Palette.textPrimary)
                        Text(verbatim: row.failure?.message ?? "The memory could not be read.")
                            .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    }
                    Divider().overlay(Theme.Palette.border)
                }
            }
            if state.evidenceLoading { ProgressView().controlSize(.small) }
            if state.evidence.isEmpty && !state.evidenceLoading && state.evidenceFailure == nil {
                Text("No supporting memories were returned for this relationship.")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
            if state.evidenceDiscardedCount > 0 {
                Text(verbatim: "\(state.evidenceDiscardedCount) earlier memories are no longer retained. Select the relationship again to start over.")
                    .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            }
            if state.evidenceNextOffset != nil {
                Button("Load more memories", action: actions.moreEvidence).buttonStyle(SubtleButtonStyle())
                    .disabled(state.evidenceLoading)
            }
        }
    }

    private var edgeIndex: Int? {
        state.snapshot?.links.firstIndex { link in
            guard let snapshot = state.snapshot else { return false }
            return snapshot.nodes[link.sourceIndex].key == edge.sourceKey
                && snapshot.nodes[link.targetIndex].key == edge.targetKey && link.kind == edge.kind
        }
    }
}
