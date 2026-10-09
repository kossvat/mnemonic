import SwiftUI

// MARK: - Records contract (parsed from /api/leaderboard/sessions)

struct SessionRecord: Identifiable {
    let id: String
    let startedAt: Date?
    let durationSeconds: Double
    let durationHuman: String
    let topProject: String?
    /// The stretch is still going on: its length grows while the work does.
    let live: Bool

    static func fromJSON(_ obj: [String: Any]) -> SessionRecord? {
        guard let id = obj["session_id"] as? String else { return nil }
        // duration_seconds arrives as NSNumber; accept Int/Double spellings.
        let secs: Double
        if let n = obj["duration_seconds"] as? NSNumber {
            secs = n.doubleValue
        } else {
            return nil
        }
        return SessionRecord(
            id: id,
            startedAt: parseTimestamp(obj["started_at"] as? String ?? ""),
            durationSeconds: secs,
            durationHuman: obj["duration_human"] as? String ?? fmtDur(secs),
            topProject: obj["top_project"] as? String,
            live: obj["live"] as? Bool ?? false
        )
    }

    /// The backend stores mixed RFC3339 and SQLite `YYYY-MM-DD HH:MM:SS`
    /// (UTC) rows — accept both, mirroring the daemon's own parser.
    static func parseTimestamp(_ s: String) -> Date? {
        if let d = ISO8601DateFormatter().date(from: s) { return d }
        let iso = ISO8601DateFormatter()
        iso.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let d = iso.date(from: s) { return d }
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.timeZone = TimeZone(identifier: "UTC")
        f.dateFormat = "yyyy-MM-dd HH:mm:ss"
        return f.date(from: s)
    }
}

// MARK: - Records page (longest-sessions leaderboard)

struct RecordsPageView: View {
    let service: MnemonicService

    @State private var records: [SessionRecord]? = nil
    /// The stretches going on now, longest first, whatever their rank.
    @State private var going: [SessionRecord] = []
    @State private var isLoading = false
    @State private var errorText: String? = nil

    /// `previewRecords` lets the headless preview renderer inject already-
    /// fetched rows so snapshots show content, not the loading state (same
    /// pattern as JournalPageView.previewDay).
    init(service: MnemonicService,
         previewRecords: [SessionRecord]? = nil, previewGoing: [SessionRecord] = []) {
        self.service = service
        _records = State(initialValue: previewRecords)
        _going = State(initialValue: previewGoing)
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Records")
                .font(.system(size: 19, weight: .bold)).tracking(-0.4)
                .foregroundStyle(WT.text)
                .padding(.bottom, 2)
            Text("Longest work sessions")
                .font(.system(size: 12.5, weight: .medium))
                .foregroundStyle(WT.sub)
                .padding(.bottom, 12)

            if let records, !records.isEmpty {
                if !going.isEmpty {
                    sectionLabel("NOW")
                    ForEach(going) { r in NowRowView(record: r) }
                    sectionLabel("ALL TIME").padding(.top, 10)
                }
                ForEach(Array(records.enumerated()), id: \.element.id) { i, r in
                    if i > 0 { Rectangle().fill(WT.sep).frame(height: 1) }
                    RecordRowView(rank: i + 1, record: r)
                }
            } else if isLoading {
                loadingState
            } else if let errorText {
                message(errorText, icon: "wifi.slash")
            } else {
                message("No sessions yet. Records appear as you work.",
                        icon: "trophy")
            }
        }
        .onAppear {
            // The deck mounts only the page shown, so this runs each time
            // the owner comes to the page: the owner's act, which may use the
            // command line. Preview-injected rows render as-is.
            service.selectPage(4)
            if records == nil { load(command: true, automatic: true) }
        }
        .onReceive(service.refreshed) { command in
            // An explicit refresh may retry a visible page. The service
            // binds its fallback and completion to this appearance.
            load(command: command, automatic: false)
        }
    }

    private func sectionLabel(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 10, weight: .bold)).tracking(0.6)
            .foregroundStyle(WT.ter)
            .padding(.bottom, 4)
    }

    private var loadingState: some View {
        HStack(spacing: 8) {
            ProgressView().controlSize(.small)
            Text("Loading records…")
                .font(.system(size: 12.5, weight: .medium)).foregroundStyle(WT.ter)
        }
        .frame(maxWidth: .infinity).padding(.vertical, 40)
    }

    private func message(_ text: String, icon: String) -> some View {
        VStack(spacing: 14) {
            RoundedRectangle(cornerRadius: 12).fill(WT.fill).frame(width: 44, height: 44)
                .overlay(Image(systemName: icon).font(.system(size: 20)).foregroundStyle(WT.ter))
            Text(text)
                .font(.system(size: 13.5, weight: .semibold)).foregroundStyle(WT.sub)
                .multilineTextAlignment(.center).lineSpacing(3).frame(maxWidth: 230)
        }
        .frame(maxWidth: .infinity).padding(.top, 48)
    }

    private func load(command: Bool, automatic: Bool) {
        guard !isLoading, service.isOpen, service.selectedPage == 4 else { return }
        isLoading = true
        service.loadRecords(command: command, automatic: automatic) { obj in
            isLoading = false
            guard let rows = obj?["sessions"] as? [[String: Any]] else {
                // Preserve useful older rows, but do not present a failed
                // first load as an empty leaderboard.
                if command, records?.isEmpty ?? true {
                    records = nil
                    errorText = "Leaderboard backend is not ready yet."
                }
                return
            }
            errorText = nil
            records = rows.compactMap(SessionRecord.fromJSON)
            // An older daemon sends no `now`: the section is not shown.
            going = (obj?["now"] as? [[String: Any]] ?? [])
                .compactMap(SessionRecord.fromJSON)
        }
    }
}

/// A stretch of work going on now: how long it has run and on what. Its
/// length is the daemon's, so it moves at every refresh of the widget.
struct NowRowView: View {
    let record: SessionRecord

    var body: some View {
        HStack(spacing: 9) {
            Circle().fill(WT.working).frame(width: 7, height: 7)
            Text(record.durationHuman)
                .font(.system(size: 15, weight: .bold)).tracking(-0.2)
                .monospacedDigit()
                .foregroundStyle(WT.text)
            Text(record.topProject ?? "no project signal")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(record.topProject == nil ? WT.ter : WT.sub)
                .lineLimit(1).truncationMode(.tail)
            Spacer(minLength: 6)
            if let date = record.startedAt {
                Text("since " + Self.clock(date))
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(WT.ter)
            }
        }
        .padding(.vertical, 6).padding(.horizontal, 2)
    }

    static func clock(_ date: Date) -> String {
        let f = DateFormatter()
        f.dateFormat = Calendar.current.isDateInToday(date) ? "HH:mm" : "d MMM HH:mm"
        return f.string(from: date)
    }
}

struct RecordRowView: View {
    let rank: Int
    let record: SessionRecord

    private var isTop: Bool { rank == 1 }

    var body: some View {
        HStack(spacing: 11) {
            ZStack {
                RoundedRectangle(cornerRadius: 7)
                    .fill(isTop ? WT.accent.opacity(0.16) : WT.fill)
                    .frame(width: 26, height: 26)
                if isTop {
                    Image(systemName: "trophy.fill")
                        .font(.system(size: 12, weight: .semibold))
                        .foregroundStyle(WT.accent)
                } else {
                    Text("\(rank)")
                        .font(.system(size: 12, weight: .bold)).monospacedDigit()
                        .foregroundStyle(WT.sub)
                }
            }
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    // Backend-rendered duration ("1m 30s"), NOT fmtDur —
                    // fmtDur rounds to whole minutes and would show a
                    // 90-second record as "2m" (review point).
                    HStack(spacing: 6) {
                        Text(record.durationHuman)
                            .font(.system(size: 15, weight: .bold)).tracking(-0.2)
                            .monospacedDigit()
                            .foregroundStyle(isTop ? WT.accent : WT.text)
                        if record.live {
                            Circle().fill(WT.working).frame(width: 6, height: 6)
                            Text("now")
                                .font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(WT.working)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                    if let date = record.startedAt {
                        Text(Self.dayLabel(date))
                            .font(.system(size: 11, weight: .medium))
                            .foregroundStyle(WT.ter)
                    }
                }
                Text(record.topProject ?? "no project signal")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(record.topProject == nil ? WT.ter : WT.sub)
                    .lineLimit(1).truncationMode(.tail)
            }
        }
        .padding(.vertical, 10).padding(.horizontal, 2)
    }

    static func dayLabel(_ date: Date) -> String {
        let f = DateFormatter()
        f.dateFormat = Calendar.current.isDate(date, equalTo: Date(), toGranularity: .year)
            ? "d MMM" : "d MMM yyyy"
        return f.string(from: date)
    }
}
