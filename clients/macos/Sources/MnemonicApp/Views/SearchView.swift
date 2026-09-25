import SwiftUI
import MnemonicShared

struct SearchView: View {
    @ObservedObject var model: MnemonicAppModel
    @State private var query = ""
    @State private var withGraphHop = true
    @State private var results: [SearchHit] = []
    @State private var isSearching = false
    @State private var hasSearched = false
    @FocusState private var searchFocused: Bool

    private let suggestions = ["auth service jwt", "billing tier pricing", "checkout flow", "feedback corrections"]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header
            searchInput
            Divider().background(Theme.Palette.border)
            content
        }
        .background(Theme.Palette.bgPrimary)
        .onReceive(NotificationCenter.default.publisher(for: .mnemonicFocusSearch)) { _ in
            model.route = .search
            searchFocused = true
        }
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 2) {
            Text("Search")
                .font(Theme.Font.display)
                .tracking(Theme.Font.trackingDisplay)
                .foregroundStyle(Theme.Palette.textPrimary)
            Text("Hybrid retrieval: BM25 + vector + graph hop fused via RRF")
                .font(Theme.Font.body)
                .foregroundStyle(Theme.Palette.textMuted)
        }
        .padding(.horizontal, Theme.Space.xl)
        .padding(.top, Theme.Space.xl)
        .padding(.bottom, Theme.Space.lg)
    }

    private var searchInput: some View {
        HStack(spacing: Theme.Space.sm) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 13))
                .foregroundStyle(Theme.Palette.textSubtle)
            TextField("Ask for a memory, project, price, or decision", text: $query)
                .textFieldStyle(.plain)
                .font(Theme.Font.body)
                .focused($searchFocused)
                .onSubmit { Task { await runSearch() } }

            Toggle(isOn: $withGraphHop) {
                HStack(spacing: 4) {
                    Image(systemName: "point.3.connected.trianglepath.dotted")
                        .font(.system(size: 10))
                    Text("Graph hop")
                        .font(Theme.Font.caption)
                }
                .foregroundStyle(Theme.Palette.textMuted)
            }
            .toggleStyle(.switch)
            .controlSize(.small)

            Button {
                Task { await runSearch() }
            } label: {
                if isSearching {
                    ProgressView().controlSize(.small)
                } else {
                    Text("Search")
                        .font(Theme.Font.bodyMedium)
                }
            }
            .buttonStyle(SubtleButtonStyle())
            .disabled(query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || isSearching)
        }
        .padding(.horizontal, Theme.Space.lg)
        .padding(.vertical, Theme.Space.md)
        .background(
            RoundedRectangle(cornerRadius: Theme.Radius.md, style: .continuous)
                .fill(Theme.Palette.bgSurface)
        )
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.md, style: .continuous)
                .stroke(searchFocused ? Theme.Palette.accent.opacity(0.4) : Theme.Palette.border,
                        lineWidth: Theme.Stroke.thin)
        )
        .padding(.horizontal, Theme.Space.xl)
        .padding(.bottom, Theme.Space.lg)
        .animation(Theme.Motion.quick, value: searchFocused)
    }

    @ViewBuilder
    private var content: some View {
        if isSearching {
            VStack {
                ProgressView().controlSize(.small)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
        } else if !hasSearched {
            EmptyStateView(
                icon: "sparkle.magnifyingglass",
                title: "Find something in your memory",
                subtitle: "Combines text matching, semantic similarity, and graph relationships. Try a project name, a concept, or a question.",
                suggestions: suggestions,
                onSuggestion: { suggestion in
                    query = suggestion
                    Task { await runSearch() }
                }
            )
        } else if results.isEmpty {
            EmptyStateView(
                icon: "questionmark.dashed",
                title: "Nothing found",
                subtitle: "Try a shorter query, or toggle off Graph hop to widen the net."
            )
        } else {
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(results) { hit in
                        SearchResultRow(hit: hit)
                        Divider().background(Theme.Palette.border.opacity(0.5))
                    }
                }
                .padding(.horizontal, Theme.Space.xl)
            }
        }
    }

    private func runSearch() async {
        let trimmed = query.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        isSearching = true
        hasSearched = true
        defer { isSearching = false }
        do {
            results = try await model.client.search(
                query: trimmed,
                limit: 40,
                withGraphHop: withGraphHop
            ).results
        } catch {
            model.showToast(error.localizedDescription)
        }
    }
}

struct SearchResultRow: View {
    let hit: SearchHit

    var body: some View {
        VStack(alignment: .leading, spacing: Theme.Space.sm) {
            HStack(alignment: .firstTextBaseline, spacing: Theme.Space.sm) {
                Text(hit.title)
                    .font(Theme.Font.title)
                    .foregroundStyle(Theme.Palette.textPrimary)
                    .lineLimit(2)
                Spacer()
                Text(RelativeTime.string(from: hit.timestamp))
                    .font(Theme.Font.caption)
                    .foregroundStyle(Theme.Palette.textSubtle)
            }
            Text(hit.contentPreview)
                .font(Theme.Font.body)
                .foregroundStyle(Theme.Palette.textMuted)
                .lineLimit(3)
            HStack(spacing: Theme.Space.sm) {
                TypeChip(type: hit.memoryType)
                SourceChip(label: hit.sources)
                Spacer()
                Text("RRF \(String(format: "%.3f", hit.rrfScore))")
                    .font(Theme.Font.mono)
                    .foregroundStyle(Theme.Palette.textSubtle)
            }
        }
        .padding(.vertical, Theme.Space.md)
    }
}

/// Subtle "via: bm25+vector+graph" provenance pill — accent for graph hits.
struct SourceChip: View {
    let label: String

    var body: some View {
        HStack(spacing: 4) {
            Image(systemName: "arrow.triangle.branch")
                .font(.system(size: 9, weight: .semibold))
            Text(label)
                .font(Theme.Font.captionBold)
        }
        .foregroundStyle(Theme.Palette.accent)
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .background(
            RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                .fill(Theme.Palette.accent.opacity(0.10))
        )
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                .stroke(Theme.Palette.accent.opacity(0.15), lineWidth: Theme.Stroke.hairline)
        )
    }
}
