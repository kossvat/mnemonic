import SwiftUI
import MnemonicShared

/// The pages of the deck that are mounted now, by index. With one page
/// mounted it holds one index; the tests of the widget at rest read it.
enum PageMounts {
    private(set) static var shown: [Int] = []

    static func appeared(_ index: Int) {
        if !shown.contains(index) { shown.append(index) }
    }

    static func disappeared(_ index: Int) {
        shown.removeAll { mounted in mounted == index }
    }
}

/// The live page and five secondary pages, mounted only while visible.
/// macOS has no TabView .page, so this is a custom deck: the page shown,
/// moved by the drag and replaced by its neighbour past a threshold. A
/// persistent header (brand + status + nav icons) sits above; custom amber
/// page dots sit below. Ported from mn-paged.jsx.
@MainActor
struct PagedContainerView: View {
    @ObservedObject var service: MnemonicService
    @ObservedObject var live: WidgetLiveController
    /// A memory begun on the Work page survives a swipe to another page.
    @StateObject private var draft = MemoryDraft()

    @State private var page: Int = 0

    init(service: MnemonicService, live: WidgetLiveController, previewPage: Int? = nil) {
        self.service = service
        self.live = live
        _page = State(initialValue: min(5, max(0, previewPage ?? 0)))
    }
    @State private var drag: CGFloat = 0
    // Height with a one-time migration: existing users had the old 560
    // default persisted, which clips the always-expanded Work page. Bump
    // everyone to at least 680 once (respecting any taller manual resize),
    // then honor future resizes normally.
    @State private var deckHeight: CGFloat = {
        let ud = ProfilePreferences.current
        let saved = (ud.object(forKey: "mn-deck-h") as? Double).map { height in CGFloat(height) }
        if !ud.bool(forKey: "mn-deck-migrated-680") {
            ud.set(true, forKey: "mn-deck-migrated-680")
            let h = Swift.max(saved ?? 680, 680)
            ud.set(Double(h), forKey: "mn-deck-h")
            return h
        }
        return saved ?? 680
    }()
    @State private var resizeBase: CGFloat = 0

    private let pageCount = 6
    private let deckWidth: CGFloat = 336
    private let minHeight: CGFloat = 420
    private let maxHeight: CGFloat = 840

    private var d: WidgetData { service.data }
    private let navIcons = [("bolt.circle", "Live"), ("clock", "Memory"), ("folder", "Projects"),
                            ("book.pages", "Journal"), ("trophy", "Records"),
                            ("square.and.arrow.up", "Share")]

    var body: some View {
        VStack(spacing: 0) {
            header
            deck.clipped()
            bottomBar
        }
        .frame(width: deckWidth, height: deckHeight)
        .background(WT.bg)
        .onChange(of: live.visible) { _, visible in
            if visible { live.selectPage(page) }
        }
    }

    // MARK: Header (brand + status + nav icons)

    private var header: some View {
        HStack(spacing: 9) {
            Text("Mnemonic")
                .font(.system(size: 13.5, weight: .bold)).tracking(-0.2)
                .foregroundStyle(WT.text)
            WidgetConnectionBadge(store: live.store)
            Spacer(minLength: 0)
            HStack(spacing: 1) {
                ForEach(Array(navIcons.enumerated()), id: \.offset) { i, item in
                    Button { goto(i) } label: {
                        Image(systemName: item.0)
                            .font(.system(size: 12, weight: .medium))
                            // sub, not ter: inactive tabs were nearly
                            // invisible, hiding that there ARE other pages.
                            .foregroundStyle(i == page ? WT.accent : WT.sub)
                            .frame(width: 23, height: 27)
                            .background(RoundedRectangle(cornerRadius: 7)
                                .fill(i == page ? WT.fill : .clear))
                    }
                    .buttonStyle(.plain)
                    .help(item.1)
                }
            }
        }
        .padding(.horizontal, 14)
        .padding(.top, 13).padding(.bottom, 10)
    }

    // MARK: Deck

    private var deck: some View {
        GeometryReader { geo in
            let w = geo.size.width
            // One page is mounted: the one shown. A page that is not looked
            // at is not laid out, drawn or kept up to date.
            Group {
                if live.visible { shownPage }
                else { Color.clear }
            }
                .frame(width: w)
                .id(page)
                .offset(x: drag)
                .contentShape(Rectangle())
                .gesture(
                    DragGesture(minimumDistance: 10)
                        .onChanged { v in
                            // Horizontal-only; let vertical scrolling pass through.
                            guard abs(v.translation.width) > abs(v.translation.height) else { return }
                            var dx = v.translation.width
                            if (page == 0 && dx > 0) || (page == pageCount - 1 && dx < 0) { dx *= 0.32 }
                            drag = dx
                        }
                        .onEnded { v in
                            let th = w * 0.16
                            if abs(v.translation.width) > abs(v.translation.height) {
                                if v.translation.width < -th { goto(page + 1) }
                                else if v.translation.width > th { goto(page - 1) }
                            }
                            drag = 0
                        }
                )
        }
    }

    // Each page is its own vertical ScrollView so tall content scrolls
    // (Codex: the Work page was clipping under the dots). Padding lives
    // inside the ScrollView so it scrolls too.
    @ViewBuilder
    private var shownPage: some View {
        let pad = EdgeInsets(top: 6, leading: 16, bottom: 16, trailing: 16)
        switch page {
        case 1:
            pageScroll(1, pad: EdgeInsets()) {
                MenuBarView(service: service, draft: draft, inDeck: true, onShare: { goto(5) })
            }
        case 2:
            pageScroll(2, pad: pad) {
                ProjectsPageView(data: d, onOpenApp: { service.openDashboard() })
            }
        case 3:
            pageScroll(3, pad: pad) { JournalPageView(service: service) }
        case 4:
            pageScroll(4, pad: pad) { RecordsPageView(service: service) }
        case 5:
            pageScroll(5, pad: EdgeInsets()) {
                ShareComposerView(data: d, service: service, inPage: true)
            }
        default:
            pageScroll(0, pad: EdgeInsets()) {
                LiveProjectsPage(live: live, openProject: { key in service.openDashboard(link: .project(key)) },
                                 search: { service.openDashboard(link: .search) })
            }
        }
    }

    private func pageScroll<C: View>(
        _ index: Int, pad: EdgeInsets, @ViewBuilder _ content: () -> C
    ) -> some View {
        ScrollView(.vertical, showsIndicators: false) {
            content()
                .padding(pad)
                .frame(maxWidth: .infinity, alignment: .topLeading)
        }
        .onAppear {
            PageMounts.appeared(index)
            live.selectPage(index)
            service.selectPage(index)
        }
        .onDisappear { PageMounts.disappeared(index) }
    }

    // MARK: Bottom bar — page dots + drag-to-resize grabber

    private var bottomBar: some View {
        VStack(spacing: 5) {
            // Grabber: drag vertically to resize the whole popover.
            Capsule().fill(WT.barIdle)
                .frame(width: 34, height: 4)
                .padding(.top, 5)
                .contentShape(Rectangle().inset(by: -10))
                .gesture(
                    DragGesture(minimumDistance: 2)
                        .onChanged { v in
                            if resizeBase == 0 { resizeBase = deckHeight }
                            deckHeight = min(maxHeight, max(minHeight, resizeBase + v.translation.height))
                        }
                        .onEnded { _ in
                            resizeBase = 0
                            ProfilePreferences.current.set(Double(deckHeight), forKey: "mn-deck-h")
                        }
                )
                .help("Drag to resize")

            HStack(spacing: 7) {
                ForEach(0..<pageCount, id: \.self) { i in
                    Button { goto(i) } label: {
                        RoundedRectangle(cornerRadius: 3)
                            .fill(i == page ? WT.accent : WT.barIdle)
                            .frame(width: i == page ? 17 : 6, height: 6)
                    }
                    .buttonStyle(.plain)
                }
            }
            .animation(.spring(response: 0.3, dampingFraction: 0.8), value: page)
        }
        .frame(maxWidth: .infinity)
        .padding(.bottom, 8)
    }

    private func goto(_ p: Int) {
        let np = max(0, min(pageCount - 1, p))
        page = np
        live.selectPage(np)
    }
}

@MainActor
private struct WidgetConnectionBadge: View {
    @ObservedObject var store: LiveStore
    var body: some View {
        Circle().fill(store.shown != nil && store.failure == nil ? WT.working : WT.ter)
            .frame(width: 5, height: 5)
            .help(store.failure?.widgetTitle ?? (store.shown == nil ? "Connecting" : "Local API connected"))
    }
}
