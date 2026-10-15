import SwiftUI
import MnemonicShared

@MainActor
struct ContentView: View {
    @ObservedObject var model: MnemonicAppModel
    @ObservedObject private var projects: AppProjectController
    @ObservedObject private var live: LiveStore
    @ObservedObject private var timeline: AppTimelineController
    @ObservedObject private var graph: AppGraphController
    @State private var windowVisible = false

    init(model: MnemonicAppModel) {
        self.model = model
        projects = model.projects
        live = model.projects.live
        timeline = model.timeline
        graph = model.curatedGraph
    }

    var body: some View {
        NavigationSplitView {
            Sidebar(selection: $model.route, daemonFailure: live.failure ?? (model.isOnline ? nil : .unreachable))
                .navigationSplitViewColumnWidth(min: 200, ideal: 208, max: 260)
        } detail: {
            ZStack(alignment: .bottom) {
                detailView
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

    @ViewBuilder
    private var detailView: some View {
        switch model.route {
        case .projects:
            ProjectsView(board: projects.board, live: live.shown,
                         failure: projects.failure ?? live.failure, loading: projects.loading,
                         now: projects.now, onOpen: { key in model.route = .project(key) },
                         paintRevision: model.boardPaintRevision,
                         onDraw: { paint, card, revision in
                             if windowVisible { model.boardDidDraw(paint, card: card, revision: revision) }
                         })
        case .timeline(let day):
            TimelinePage(
                day: day, snapshot: timeline.snapshot, loading: timeline.loading,
                failure: timeline.failure, mutationInFlight: timeline.mutationInFlight,
                message: timeline.message, canUndo: timeline.canUndo,
                now: projects.now, live: live.failure == nil ? live.shown : nil,
                registry: projects.board,
                onDay: { chosen in model.route = .timeline(chosen) },
                onToday: selectToday,
                onRefresh: { Task { await model.refreshAll() } },
                onApply: { correction, chosen in await timeline.apply(correction, for: chosen) },
                onUndo: { await timeline.undo() })
        case .graph:
            GraphView(controller: graph, onOpenProject: { key in model.route = .project(key) },
                      onDraw: { snapshot in
                          if windowVisible { model.graphDidDraw(snapshot) }
                      })
        case .project(let key):
            ProjectPage(key: key, detail: projects.detail,
                        failure: projects.failure, loading: projects.loading,
                        liveFailure: live.failure, now: projects.now, onBack: { model.route = .projects })
        default:
            if let message = model.setupMessage, model.status == nil {
                SetupView(message: message, model: model)
            } else {
                switch model.route {
                case .overview: OverviewView(model: model)
                case .memories: MemoriesView(model: model)
                case .search: SearchView(model: model)
                case .entities: EntitiesView(model: model)
                case .memoryMap: MemoryMapView(model: model)
                case .manage: ManageView(model: model)
                default: EmptyView()
                }
            }
        }
    }

    func selectToday() {
        model.route = .timeline(TimelineDay.today(now: projects.currentDate))
    }
}
