import SwiftUI
import MnemonicShared

struct Sidebar: View {
    @Binding var selection: AppRoute

    private struct Group {
        let label: String
        let routes: [AppRoute]
    }

    private let groups: [Group] = [
        Group(label: "Workspace", routes: [.overview, .memories, .search, .entities, .graph, .memoryMap]),
        Group(label: "Admin",     routes: [.manage])
    ]

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header

            ScrollView {
                VStack(alignment: .leading, spacing: Theme.Space.xl) {
                    ForEach(groups, id: \.label) { group in
                        VStack(alignment: .leading, spacing: Theme.Space.xs) {
                            SectionLabel(group.label)
                                .padding(.horizontal, Theme.Space.md)
                            VStack(spacing: 1) {
                                ForEach(group.routes) { route in
                                    sidebarRow(route)
                                }
                            }
                        }
                    }
                }
                .padding(.vertical, Theme.Space.lg)
            }
        }
        .background(Theme.Palette.bgPrimary)
        .navigationTitle("Mnemonic")
    }

    private var header: some View {
        HStack(spacing: Theme.Space.sm) {
            ZStack {
                RoundedRectangle(cornerRadius: 6, style: .continuous)
                    .fill(Theme.Palette.accent.opacity(0.12))
                    .frame(width: 24, height: 24)
                Image(systemName: "brain")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(Theme.Palette.accent)
            }
            Text("Mnemonic")
                .font(Theme.Font.title)
                .foregroundStyle(Theme.Palette.textPrimary)
            Spacer()
        }
        .padding(.horizontal, Theme.Space.lg)
        .padding(.vertical, Theme.Space.lg)
        .overlay(alignment: .bottom) {
            Rectangle()
                .fill(Theme.Palette.border)
                .frame(height: Theme.Stroke.hairline)
        }
    }

    private func sidebarRow(_ route: AppRoute) -> some View {
        let isActive = selection == route
        return Button {
            selection = route
        } label: {
            HStack(spacing: Theme.Space.sm) {
                // Active indicator
                Rectangle()
                    .fill(isActive ? Theme.Palette.accent : Color.clear)
                    .frame(width: 2)
                    .padding(.vertical, 2)
                HStack(spacing: Theme.Space.sm) {
                    Image(systemName: route.symbol)
                        .font(.system(size: 12, weight: .regular))
                        .frame(width: 16)
                        .foregroundStyle(isActive ? Theme.Palette.textPrimary : Theme.Palette.textMuted)
                    Text(route.title)
                        .font(Theme.Font.body)
                        .foregroundStyle(isActive ? Theme.Palette.textPrimary : Theme.Palette.textMuted)
                    Spacer()
                }
                .padding(.vertical, Theme.Space.sm)
                .padding(.trailing, Theme.Space.md)
            }
            .padding(.leading, Theme.Space.md - 2)
            .background(
                isActive ? Theme.Palette.bgTint : Color.clear,
                in: RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
            )
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .padding(.horizontal, Theme.Space.sm)
    }
}
