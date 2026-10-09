import MnemonicShared
import SwiftUI

struct GraphSidePanel: View {
    let state: GraphPageState
    let actions: GraphPageActions

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Theme.Space.xl) {
                GraphSelectionPanel(state: state, actions: actions)
                Divider().overlay(Theme.Palette.border)
                staleProjects
                Divider().overlay(Theme.Palette.border)
                outsideEntities
                Divider().overlay(Theme.Palette.border)
                archiveHistory
            }
            .padding(Theme.Space.lg)
        }
        .background(Theme.Palette.bgSurface)
    }

    private var staleProjects: some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            sectionTitle("Stale projects", count: state.staleProjectCount)
            Text("No recent activity, or activity is unavailable. Archiving is optional.")
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            if let notice = state.shelfNotice {
                smallNotice(notice)
            } else {
                if state.staleProjects.isEmpty { smallNotice("No stale projects in this view.") }
                ForEach(state.staleProjects) { item in
                    VStack(alignment: .leading, spacing: 6) {
                        shelfLabel(key: item.key, label: item.label,
                                   detail: GraphPresentation.activity(item.inactiveDays))
                        HStack(spacing: 8) {
                            Button("Expand") { actions.expand(item.key) }.buttonStyle(SubtleButtonStyle())
                            if let slug = item.projectSlug {
                                Button("Archive") { actions.archive(.project(key: slug, label: item.label), true) }
                                    .buttonStyle(SubtleButtonStyle()).disabled(state.mutationInFlight)
                                    .accessibilityLabel("Archive \(item.label)")
                            }
                        }
                    }
                    .padding(.vertical, 3)
                }
                if let count = state.snapshot?.omitted.staleProjectCount, count > 0 {
                    smallNotice("\(count) more stale projects are outside this view. The Projects page lists the registry.")
                }
            }
        }
    }

    private var outsideEntities: some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            sectionTitle("Outside the registry", count: state.outsideEntityCount)
            Text("Project-like entities without a current registry project.")
                .font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            if state.outside.isLoaded {
                archiveRows(state.outside, restoring: false)
            } else if let notice = state.shelfNotice {
                smallNotice(notice)
            } else {
                ForEach(state.outsideShelf) { item in
                    HStack(alignment: .top, spacing: 8) {
                        shelfLabel(key: item.key, label: item.label,
                                   detail: "\(item.memoryCount) memories · \(GraphPresentation.activity(item.inactiveDays))")
                        if let id = GraphPresentation.entityID(item.key) {
                            Button("Archive") { actions.archive(.entity(id: id, label: item.label), true) }
                                .buttonStyle(SubtleButtonStyle()).disabled(state.mutationInFlight)
                                .accessibilityLabel("Archive \(item.label)")
                        }
                    }
                }
                if state.outsideShelf.isEmpty { smallNotice("No outside entities in this view.") }
                if let count = state.snapshot?.omitted.outsideRegistryCount, count > 0 {
                    smallNotice("\(count) more outside entities are not shown in the overview.")
                }
            }
            listStatus(state.outside)
            HStack {
                if !state.outside.isLoaded {
                    Button("Load outside entities") { actions.outside(true) }
                } else if state.outside.nextOffset != nil {
                    Button("Load more") { actions.outside(false) }
                }
                if state.outside.isLoaded {
                    Button("Start over") { actions.outside(true) }
                }
            }
            .buttonStyle(SubtleButtonStyle()).disabled(state.outside.loading)
        }
    }

    private var archiveHistory: some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            sectionTitle("Archive history", count: state.history.total)
            if state.history.isLoaded {
                archiveRows(state.history, restoring: true)
            } else {
                smallNotice("Archived entities can be restored individually.")
            }
            listStatus(state.history)
            HStack {
                if !state.history.isLoaded {
                    Button("Load history") { actions.history(true) }
                } else if state.history.nextOffset != nil {
                    Button("Load more") { actions.history(false) }
                }
                if state.history.isLoaded {
                    Button("Start over") { actions.history(true) }
                }
            }
            .buttonStyle(SubtleButtonStyle()).disabled(state.history.loading)
        }
    }

    private func archiveRows(_ list: GraphArchiveListState, restoring: Bool) -> some View {
        VStack(alignment: .leading, spacing: Theme.Space.md) {
            if list.entities.isEmpty { smallNotice(restoring ? "No archived entities." : "No outside entities.") }
            ForEach(list.entities) { item in
                HStack(alignment: .top, spacing: 8) {
                    shelfLabel(key: "entity:" + item.id, label: item.name,
                               detail: "\(item.memoryCount) memories · \(GraphPresentation.date(item.lastMemory))")
                    Button(restoring ? "Restore" : "Archive") {
                        actions.archive(.entity(id: item.id, label: item.name), !restoring)
                    }
                    .buttonStyle(SubtleButtonStyle()).disabled(state.mutationInFlight)
                    .accessibilityLabel("\(restoring ? "Restore" : "Archive") \(item.name)")
                }
            }
        }
    }

    private func shelfLabel(key: String, label: String, detail: String) -> some View {
        Button { actions.selectNode(key) } label: {
            VStack(alignment: .leading, spacing: 4) {
                Text(verbatim: label).font(Theme.Font.bodyMedium)
                    .foregroundStyle(state.selectedKey == key ? Theme.Palette.accent : Theme.Palette.textPrimary)
                Text(verbatim: detail).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
                    .fixedSize(horizontal: false, vertical: true)
            }
            .frame(maxWidth: .infinity, alignment: .leading).contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }

    @ViewBuilder private func listStatus(_ list: GraphArchiveListState) -> some View {
        if list.loading { ProgressView().controlSize(.small) }
        if let failure = list.failure { smallNotice(failure.message) }
        if list.discardedCount > 0 {
            smallNotice("\(list.discardedCount) earlier entries are no longer retained. Start over to return to the first page.")
        }
        if list.isLoaded, let total = list.total {
            smallNotice("\(list.entities.count) shown · \(total) total")
        }
    }

    private func sectionTitle(_ title: String, count: Int?) -> some View {
        HStack {
            Text(verbatim: title).font(Theme.Font.title).foregroundStyle(Theme.Palette.textPrimary)
            Spacer()
            if let count { Text(verbatim: "\(count)").font(Theme.Font.mono).foregroundStyle(Theme.Palette.textMuted) }
        }
    }

    private func smallNotice(_ text: String) -> some View {
        Text(verbatim: text).font(Theme.Font.caption).foregroundStyle(Theme.Palette.textMuted)
            .fixedSize(horizontal: false, vertical: true)
    }
}
