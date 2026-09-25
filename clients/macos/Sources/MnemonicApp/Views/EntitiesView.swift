import SwiftUI
import MnemonicShared

struct EntitiesView: View {
    @ObservedObject var model: MnemonicAppModel
    @State private var filter = ""
    @State private var selected: EntitySummary?
    @State private var detail: EntityDetail?
    @State private var isLoadingDetail = false

    private var filtered: [EntitySummary] {
        let base = model.entities.sorted { $0.mentions > $1.mentions }
        guard !filter.isEmpty else { return base }
        return base.filter {
            $0.name.localizedCaseInsensitiveContains(filter)
                || $0.type.localizedCaseInsensitiveContains(filter)
        }
    }

    private var grouped: [(type: String, items: [EntitySummary])] {
        Dictionary(grouping: filtered, by: \.type)
            .map { (type: $0.key, items: $0.value.sorted { $0.mentions > $1.mentions }) }
            .sorted { $0.type < $1.type }
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().background(Theme.Palette.border)
            HSplitView {
                listColumn.frame(minWidth: 360)
                EntityDetailView(
                    detail: detail,
                    isLoading: isLoadingDetail,
                    model: model,
                    onMerged: {
                        Task {
                            await model.refreshEntities()
                            if let selected {
                                await loadDetail(selected)
                            }
                        }
                    }
                )
                .frame(minWidth: 520)
                .background(Theme.Palette.bgSurface)
            }
        }
        .background(Theme.Palette.bgPrimary)
        .task {
            if model.entities.isEmpty { await model.refreshEntities() }
        }
        .onChange(of: selected) { _, newValue in
            guard let newValue else {
                detail = nil
                return
            }
            Task { await loadDetail(newValue) }
        }
    }

    private var header: some View {
        HStack(alignment: .firstTextBaseline) {
            VStack(alignment: .leading, spacing: 2) {
                Text("Entities")
                    .font(Theme.Font.display)
                    .tracking(Theme.Font.trackingDisplay)
                    .foregroundStyle(Theme.Palette.textPrimary)
                Text("\(model.entities.count) tracked across \(grouped.count) types")
                    .font(Theme.Font.caption)
                    .foregroundStyle(Theme.Palette.textSubtle)
            }
            Spacer()
            Button {
                Task { await model.refreshEntities() }
            } label: {
                Label("Refresh", systemImage: "arrow.clockwise")
                    .font(Theme.Font.caption)
            }
            .buttonStyle(SubtleButtonStyle())
        }
        .padding(Theme.Space.xl)
    }

    private var listColumn: some View {
        VStack(spacing: 0) {
            searchBar
            if filtered.isEmpty {
                EmptyStateView(
                    icon: "circle.dotted",
                    title: "No entities yet",
                    subtitle: "As the daemon ingests memories, named entities will appear here."
                )
            } else {
                ScrollView {
                    LazyVStack(alignment: .leading, spacing: Theme.Space.lg) {
                        ForEach(grouped, id: \.type) { group in
                            VStack(alignment: .leading, spacing: Theme.Space.xs) {
                                HStack(spacing: 6) {
                                    Image(systemName: EntityIcon.symbol(for: group.type))
                                        .font(.system(size: 10))
                                        .foregroundStyle(EntityIcon.color(for: group.type))
                                    SectionLabel(group.type)
                                    Spacer()
                                    Text("\(group.items.count)")
                                        .font(Theme.Font.caption)
                                        .foregroundStyle(Theme.Palette.textSubtle)
                                }
                                .padding(.horizontal, Theme.Space.lg)
                                VStack(spacing: 0) {
                                    ForEach(group.items) { entity in
                                        EntityRow(
                                            entity: entity,
                                            isSelected: selected?.id == entity.id
                                        )
                                        .contentShape(Rectangle())
                                        .onTapGesture { selected = entity }
                                        Divider().background(Theme.Palette.border.opacity(0.4))
                                    }
                                }
                            }
                        }
                    }
                    .padding(.vertical, Theme.Space.md)
                }
            }
        }
    }

    private var searchBar: some View {
        HStack(spacing: Theme.Space.sm) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 11))
                .foregroundStyle(Theme.Palette.textSubtle)
            TextField("Filter entities", text: $filter)
                .textFieldStyle(.plain)
                .font(Theme.Font.body)
        }
        .padding(.horizontal, Theme.Space.md)
        .padding(.vertical, Theme.Space.sm)
        .background(
            RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                .fill(Theme.Palette.bgTint)
        )
        .padding(.horizontal, Theme.Space.lg)
        .padding(.vertical, Theme.Space.md)
    }

    private func loadDetail(_ entity: EntitySummary) async {
        isLoadingDetail = true
        defer { isLoadingDetail = false }
        do {
            detail = try await model.client.fetchEntity(name: entity.name)
        } catch {
            model.showToast(error.localizedDescription)
        }
    }
}

struct EntityRow: View {
    let entity: EntitySummary
    var isSelected: Bool = false

    var body: some View {
        HStack(spacing: Theme.Space.md) {
            // Active indicator
            Rectangle()
                .fill(isSelected ? Theme.Palette.accent : Color.clear)
                .frame(width: 2)
                .padding(.vertical, 2)

            Image(systemName: EntityIcon.symbol(for: entity.type))
                .font(.system(size: 12))
                .frame(width: 16)
                .foregroundStyle(EntityIcon.color(for: entity.type))

            Text(entity.name)
                .font(Theme.Font.body)
                .foregroundStyle(Theme.Palette.textPrimary)
                .lineLimit(1)

            Spacer()

            Text("\(entity.mentions)")
                .font(Theme.Font.mono)
                .foregroundStyle(Theme.Palette.textSubtle)
        }
        .padding(.leading, Theme.Space.md - 2)
        .padding(.trailing, Theme.Space.lg)
        .padding(.vertical, Theme.Space.sm)
        .background(isSelected ? Theme.Palette.bgTint : Color.clear)
    }
}
