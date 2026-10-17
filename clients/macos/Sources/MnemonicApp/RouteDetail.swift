import SwiftUI
import MnemonicShared

/// The screen of the selected route. It observes the controllers whose values
/// change while the window rests: the label clock, live answers and refreshes.
/// Their changes draw this column again, not the window's frame, sidebar and
/// toolbar, which AppKit would otherwise lay out again on every tick.
@MainActor
struct RouteDetail: View {
    @ObservedObject var model: MnemonicAppModel
    @ObservedObject private var projects: AppProjectController
    @ObservedObject private var live: LiveStore
    @ObservedObject private var timeline: AppTimelineController
    @ObservedObject private var graph: AppGraphController
    let windowVisible: Bool

    init(model: MnemonicAppModel, windowVisible: Bool) {
        self.model = model
        projects = model.projects
        live = model.projects.live
        timeline = model.timeline
        graph = model.curatedGraph
        self.windowVisible = windowVisible
    }

    @ViewBuilder
    var body: some View {
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
