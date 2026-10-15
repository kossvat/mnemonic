import SwiftUI
import MnemonicShared

struct Sidebar: View {
    @Binding var selection: AppRoute
    var daemonFailure: LocalAPIFailure? = nil

    var body: some View {
        VStack(alignment: .leading, spacing: 20) {
            header
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    ForEach(SidebarModel.groups(), id: \.label) { group in
                        VStack(alignment: .leading, spacing: 3) {
                            Text(verbatim: group.label)
                                .font(.system(size: 10, weight: .bold)).tracking(1.5)
                                .foregroundStyle(AppPalette.textSecondary)
                                .frame(height: 14, alignment: .leading)
                                .padding(.horizontal, 12).padding(.top, 10).padding(.bottom, 5)
                            ForEach(group.entries) { route in sidebarRow(route) }
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
            }
            .scrollIndicators(.hidden)
            .accessibilityLabel("Main navigation")
            daemonLine
        }
        .padding(.horizontal, 12).padding(.top, 24).padding(.bottom, 18)
        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
        .background(AppPalette.sidebar)
        .overlay(alignment: .trailing) {
            Rectangle().fill(AppPalette.border).frame(width: 1)
        }
        .navigationTitle("Mnemonic")
    }

    private var header: some View {
        HStack(spacing: 10) {
            Path { path in
                path.move(to: CGPoint(x: 4, y: 19))
                path.addLine(to: CGPoint(x: 4, y: 7))
                path.addLine(to: CGPoint(x: 9, y: 13))
                path.addLine(to: CGPoint(x: 13, y: 7))
                path.addLine(to: CGPoint(x: 13, y: 19))
                path.move(to: CGPoint(x: 18, y: 7))
                path.addLine(to: CGPoint(x: 18, y: 19))
                path.move(to: CGPoint(x: 22, y: 7))
                path.addLine(to: CGPoint(x: 22, y: 19))
            }
            .stroke(AppPalette.owner, style: StrokeStyle(lineWidth: 2, lineCap: .round))
            .frame(width: 25, height: 25).accessibilityHidden(true)
            Text("mnemonic")
                .font(.system(size: 18, weight: .semibold)).tracking(-0.5)
                .foregroundStyle(AppPalette.textPrimary)
            Spacer(minLength: 0)
        }
        .frame(height: 32).padding(.horizontal, 12)
    }

    private var daemonLine: some View {
        HStack(spacing: 7) {
            Circle().fill(daemonFailure == nil ? AppPalette.agents : AppPalette.textMuted)
                .frame(width: 5, height: 5).accessibilityHidden(true)
            Text(verbatim: SidebarModel.daemonText(daemonFailure))
                .font(.system(size: 12)).foregroundStyle(AppPalette.textSecondary)
                .fixedSize(horizontal: false, vertical: true)
            Spacer(minLength: 0)
        }
        .padding(.horizontal, 12).padding(.top, 14)
        .overlay(alignment: .top) {
            Rectangle().fill(AppPalette.border).frame(height: 1)
        }
    }

    private func sidebarRow(_ route: AppRoute) -> some View {
        let isActive = selection.sidebarRoute == route.sidebarRoute
        return Button {
            selection = route.sidebarRoute
        } label: {
            HStack(spacing: 11) {
                Image(systemName: route.symbol)
                    .font(.system(size: 16, weight: .regular)).frame(width: 18, height: 18)
                    .accessibilityHidden(true)
                Text(verbatim: route.title)
                    .font(.system(size: 14, weight: isActive ? .semibold : .regular)).lineLimit(1)
                Spacer(minLength: 0)
            }
            .foregroundStyle(isActive ? AppPalette.textPrimary : AppPalette.textSecondary)
            .padding(.horizontal, 12).frame(height: 44)
            .background(isActive ? AppPalette.activeRow : Color.clear,
                        in: RoundedRectangle(cornerRadius: 9, style: .continuous))
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(isActive ? [.isSelected] : [])
    }
}
