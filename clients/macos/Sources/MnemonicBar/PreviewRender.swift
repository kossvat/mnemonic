import AppKit
import MnemonicShared
import SwiftUI

/// Headless preview renderer — `MnemonicBar --render-previews [dir]`.
///
/// Renders every popover page to PNG (dark + light) using REAL data from
/// the live daemon, so design iterations can be reviewed as images without
/// clicking through the menu bar (which needs Accessibility permissions
/// automation doesn't have). Exits before the status item is created, so a
/// running widget instance is not disturbed.
enum PreviewRender {
    /// Returns true when preview mode handled the launch (caller exits).
    @MainActor
    static func runIfRequested() -> Bool {
        let args = CommandLine.arguments
        if args.contains("--render-live-fixture") {
            do { try LiveFixturePreview.run(arguments: args) }
            catch {
                FileHandle.standardError.write(Data("Fixture preview failed\n".utf8))
                exit(2)
            }
            return true
        }
        guard let i = args.firstIndex(of: "--render-previews") else { return false }
        let dir = i + 1 < args.count ? args[i + 1] : "/tmp/mnemonic-previews"
        render(to: dir)
        return true
    }

    @MainActor
    private static func render(to dirPath: String) {
        let dir = URL(fileURLWithPath: dirPath)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)

        let service = MnemonicService()
        var shown = service.fetchDataNow()
        // Dev-only: MN_PREVIEW_RECALL names a file with the object of
        // `mnemonic recall --json`, so the card of memory today renders a
        // chosen day (a filled one, an empty one, the log off) without a
        // daemon that has the route.
        if let path = ProcessInfo.processInfo.environment["MN_PREVIEW_RECALL"] {
            let bytes = try? Data(contentsOf: URL(fileURLWithPath: path))
            shown.recallToday = RecallDay.load([.command { bytes }]).state
        }
        service.present(shown)

        // Journal: fetch + parse synchronously so the page renders content,
        // not its loading state.
        let dayID = {
            let f = DateFormatter()
            f.locale = Locale(identifier: "en_US_POSIX")
            f.dateFormat = "yyyy-MM-dd"
            return f.string(from: Date())
        }()
        let journalDay = service.httpObject("/api/journal?day=\(dayID)")
            .flatMap { JournalDay.fromJSON($0, fallbackDate: Date()) }

        // Dev-only: render the share card for YESTERDAY too, to verify the
        // day-pager actually fetches and renders an earlier day (the live
        // pager needs menu-bar clicks automation can't do).
        let yIDf = DateFormatter()
        yIDf.locale = Locale(identifier: "en_US_POSIX")
        yIDf.dateFormat = "yyyy-MM-dd"
        let yID = yIDf.string(from: Calendar.current.date(byAdding: .day, value: -1, to: Date()) ?? Date())
        let yesterday: WorkDay? = service.dayDetailNow(date: yID)
        let yLabelF = DateFormatter(); yLabelF.dateFormat = "EEE · MMM d"
        let yLabel = yLabelF.string(from: Calendar.current.date(byAdding: .day, value: -1, to: Date()) ?? Date())

        // Records: fetch + parse synchronously so the page renders content,
        // not its loading state (same reasoning as the journal above).
        let leaderboard = service.httpObject("/api/leaderboard/sessions?limit=10")
        let records: [SessionRecord]? = leaderboard
            .flatMap { board in board["sessions"] as? [[String: Any]] }
            .map { rows in rows.compactMap(SessionRecord.fromJSON) }
        let going: [SessionRecord] = (leaderboard?["now"] as? [[String: Any]] ?? [])
            .compactMap(SessionRecord.fromJSON)

        let pagePad = EdgeInsets(top: 6, leading: 16, bottom: 16, trailing: 16)
        let live = WidgetLiveController(clock: SystemLiveClock()) { .failed(.off) }
        live.present(.failed(.off), at: Date())
        let pages: [(String, AnyView)] = [
            ("deck-work", AnyView(PagedContainerView(service: service, live: live, previewPage: 1))),
            ("page-projects", AnyView(ProjectsPageView(data: service.data, onOpenApp: {}).padding(pagePad))),
            ("page-journal", AnyView(JournalPageView(service: service, previewDay: journalDay).padding(pagePad))),
            ("page-records", AnyView(RecordsPageView(service: service, previewRecords: records,
                                                     previewGoing: going).padding(pagePad))),
            ("page-share", AnyView(ShareComposerView(data: service.data, service: service, inPage: true))),
            ("share-card-yesterday", AnyView(
                ShareCardView(data: service.data, mode: .today, tone: .dark, includeMemory: false,
                              day: yesterday, workedSeconds: yesterday?.seconds, dateLabel: yLabel)
                    .frame(width: ShareCardView.cardSize.width, height: ShareCardView.cardSize.height)
            )),
        ]

        for (name, view) in pages {
            for (suffix, appearance) in [("dark", NSAppearance.Name.darkAqua), ("light", .aqua)] {
                snapshot(
                    view: view.background(WT.bg),
                    appearance: appearance,
                    to: dir.appendingPathComponent("\(name)-\(suffix).png")
                )
            }
        }
        print("Previews written to \(dir.path)")
    }

    @MainActor
    private static func snapshot<V: View>(view: V, appearance: NSAppearance.Name, to url: URL) {
        let host = NSHostingView(rootView: view)
        host.appearance = NSAppearance(named: appearance)
        // Fixed deck width; height hugs the content (floor keeps empty
        // states from rendering as a sliver).
        host.frame = NSRect(x: 0, y: 0, width: 336, height: 10)
        let h = max(420, host.fittingSize.height)
        host.frame = NSRect(x: 0, y: 0, width: 336, height: h)
        host.layoutSubtreeIfNeeded()

        guard let rep = host.bitmapImageRepForCachingDisplay(in: host.bounds) else { return }
        host.cacheDisplay(in: host.bounds, to: rep)
        guard let png = rep.representation(using: .png, properties: [:]) else { return }
        try? png.write(to: url)
    }
}
