import SwiftUI

// MARK: - Records contract (parsed from /api/leaderboard/sessions)

struct SessionRecord: Identifiable {
    let id: String
    let startedAt: Date?
    let durationSeconds: Double
    let durationHuman: String
    let topProject: String?

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
            topProject: obj["top_project"] as? String
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
    /// True while this deck page is the visible one. The deck keeps every
    /// page mounted for the process lifetime, so `onAppear` fires exactly
    /// once — re-fetching on activation is what keeps records fresh after
    /// later sessions complete (review point).
    let isActive: Bool

    @State private var records: [SessionRecord]? = nil
    @State private var isLoading = false
    @State private var errorText: String? = nil

    /// `previewRecords` lets the headless preview renderer inject already-
    /// fetched rows so snapshots show content, not the loading state (same
    /// pattern as JournalPageView.previewDay).
    init(service: MnemonicService, isActive: Bool = true, previewRecords: [SessionRecord]? = nil) {
        self.service = service
        self.isActive = isActive
        _records = State(initialValue: previewRecords)
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
                ForEach(Array(records.enumerated()), id: \.element.id) { i, r in
                    if i > 0 { Rectangle().fill(WT.sep).frame(height: 1) }
                    RecordRowView(rank: i + 1, record: r)
                }
            } else if isLoading {
                loadingState
            } else if let errorText {
                message(errorText, icon: "wifi.slash")
            } else {
                message("No completed sessions yet — records appear once a session ends.",
                        icon: "trophy")
            }
        }
        .onAppear {
            // Preview-injected rows render as-is; live launches fetch once.
            if records == nil { load() }
        }
        .onChange(of: isActive) { _, active in
            // Swiping back onto the page refreshes it — stale rows stay
            // visible while the new fetch runs.
            if active { load() }
        }
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

    private func load() {
        guard !isLoading else { return }
        isLoading = true
        errorText = nil
        DispatchQueue.global(qos: .utility).async {
            // HTTP first (fast when the dashboard API is on), CLI second —
            // the default config ships with the HTTP API DISABLED, and the
            // CLI serves the identical {"sessions": [...]} payload
            // (review point).
            let obj = service.httpObject("/api/leaderboard/sessions?limit=10")
                ?? service.cliObject(["session", "leaderboard", "--limit", "10", "--json"])
            DispatchQueue.main.async {
                isLoading = false
                guard let rows = obj?["sessions"] as? [[String: Any]] else {
                    // Keep stale ROWS over an error banner — but an empty
                    // cached list has nothing worth preserving, and hiding
                    // the failure behind "no completed sessions" would lie
                    // about backend health (review point).
                    if records?.isEmpty ?? true {
                        records = nil
                        errorText = "Leaderboard backend is not ready yet."
                    }
                    return
                }
                records = rows.compactMap(SessionRecord.fromJSON)
            }
        }
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
                    Text(record.durationHuman)
                        .font(.system(size: 15, weight: .bold)).tracking(-0.2)
                        .monospacedDigit()
                        .foregroundStyle(isTop ? WT.accent : WT.text)
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
