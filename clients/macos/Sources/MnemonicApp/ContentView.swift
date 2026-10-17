import SwiftUI
import MnemonicShared

@MainActor
struct ContentView: View {
    @ObservedObject var model: MnemonicAppModel
    @State private var windowVisible = false

    var body: some View {
        NavigationSplitView {
            LiveSidebar(model: model, live: model.projects.live)
                .navigationSplitViewColumnWidth(min: 200, ideal: 208, max: 260)
        } detail: {
            ZStack(alignment: .bottom) {
                RouteDetail(model: model, windowVisible: windowVisible)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)

                if let message = model.toastMessage {
                    HStack(spacing: Theme.Space.sm) {
                        Circle()
                            .fill(AppPalette.agents)
                            .frame(width: 6, height: 6)
                        Text(message)
                            .font(Theme.Font.bodyMedium)
                            .foregroundStyle(AppPalette.textPrimary)
                    }
                    .padding(.horizontal, Theme.Space.lg)
                    .padding(.vertical, Theme.Space.sm)
                    .background(
                        RoundedRectangle(cornerRadius: Theme.Radius.md, style: .continuous)
                            .fill(AppPalette.raised)
                    )
                    .overlay(
                        RoundedRectangle(cornerRadius: Theme.Radius.md, style: .continuous)
                            .stroke(AppPalette.border, lineWidth: Theme.Stroke.thin)
                    )
                    .padding(.bottom, Theme.Space.xl)
                    .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .background(AppPalette.window)
        }
        .background(AppPalette.window)
        .toolbar {
            ToolbarItem(placement: .primaryAction) {
                Button {
                    Task { await model.refreshAll() }
                } label: {
                    Image(systemName: "arrow.clockwise")
                        .font(.system(size: 12, weight: .medium))
                }
                .disabled(model.isLoading)
                .help("Refresh this screen")
            }
        }
        .background(AppWindowVisibility { id, visible in
            windowVisible = visible
            model.window(id, visible: visible)
        })
        .environment(\.mnemonicWindowVisible, windowVisible)
        .onOpenURL { url in model.open(url) }
        .onReceive(NotificationCenter.default.publisher(for: .mnemonicRefresh)) { _ in
            Task { await model.refreshAll() }
        }
        .onReceive(NotificationCenter.default.publisher(for: .mnemonicSelectRoute)) { notification in
            guard
                let raw = notification.object as? String,
                let route = AppRoute(rawValue: raw)
            else { return }
            model.route = route
        }
    }
}

/// The sidebar with the daemon's state, which follows live answers without
/// drawing the window's frame and toolbar again.
private struct LiveSidebar: View {
    @ObservedObject var model: MnemonicAppModel
    @ObservedObject var live: LiveStore

    var body: some View {
        Sidebar(selection: $model.route, daemonFailure: live.failure ?? (model.isOnline ? nil : .unreachable))
    }
}
