import Foundation
import SwiftUI

// MARK: - Journal contract (parsed from /api/journal)

struct JournalDay: Identifiable {
    let id: String
    let title: String
    let subtitle: String
    let summary: String
    let projects: [JournalProject]
    let decisions: [JournalItem]
    let followUps: [JournalItem]
    let unattributedSeconds: Double

    /// A day is empty when there's no work of any kind. The backend sends a
    /// placeholder `summary` ("No work recorded for this day.") for empty days,
    /// so we judge emptiness by the data, not the summary text — otherwise the
    /// nice empty-state never shows.
    var isEmpty: Bool {
        projects.isEmpty
        && decisions.isEmpty
        && followUps.isEmpty
        && unattributedSeconds <= 0.5
    }

    static func fromJSON(_ obj: [String: Any], fallbackDate: Date) -> JournalDay? {
        let id = obj["day"] as? String ?? journalDayID(fallbackDate)
        let date = journalDate(from: id) ?? fallbackDate
        let projects = (obj["projects"] as? [[String: Any]] ?? []).map { p in
            let key = p["key"] as? String ?? (p["name"] as? String ?? UUID().uuidString)
            let events = (p["events"] as? [[String: Any]] ?? []).compactMap { event in
                JournalEvent.fromJSON(event)
            }
            return JournalProject(
                key: key,
                name: p["name"] as? String ?? "",
                seconds: journalNum(p["seconds"]),
                confidence: p["confidence"] is NSNull ? nil : p["confidence"] as? String,
                bullets: p["bullets"] as? [String] ?? [],
                events: events
            )
        }
        let decisions = (obj["decisions"] as? [[String: Any]] ?? []).map { item in
            JournalItem(
                id: item["memory_id"] as? String ?? UUID().uuidString,
                title: item["title"] as? String ?? "",
                memoryID: item["memory_id"] as? String ?? ""
            )
        }
        let followUps = (obj["follow_ups"] as? [[String: Any]] ?? []).map { item in
            JournalItem(
                id: item["memory_id"] as? String ?? UUID().uuidString,
                title: item["title"] as? String ?? "",
                memoryID: item["memory_id"] as? String ?? ""
            )
        }
        return JournalDay(
            id: id,
            title: journalTitle(for: date),
            subtitle: journalSubtitle(for: date),
            summary: obj["summary"] as? String ?? "",
            projects: projects,
            decisions: decisions,
            followUps: followUps,
            unattributedSeconds: journalNum(obj["unattributed_seconds"])
        )
    }
}

struct JournalProject: Identifiable {
    var id: String { key }
    let key: String
    let name: String
    let seconds: Double
    let confidence: String?
    let bullets: [String]
    let events: [JournalEvent]
}

struct JournalEvent: Identifiable {
    let id: String
    let text: String
    let timeLabel: String
    let timestamp: String
    let memoryID: String

    static func fromJSON(_ obj: [String: Any]) -> JournalEvent? {
        let text = (obj["text"] as? String ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        if text.isEmpty { return nil }
        let timeLabel = obj["time_label"] as? String ?? ""
        let timestamp = obj["timestamp"] as? String ?? ""
        let memoryID = obj["memory_id"] as? String ?? ""
        let id = memoryID.isEmpty ? "\(timestamp)|\(text)" : memoryID
        return JournalEvent(id: id, text: text, timeLabel: timeLabel, timestamp: timestamp, memoryID: memoryID)
    }
}

struct JournalItem: Identifiable {
    let id: String
    let title: String
    let memoryID: String
}

private func journalNum(_ v: Any?) -> Double {
    if let n = v as? NSNumber { return n.doubleValue }
    if let s = v as? String { return Double(s) ?? 0 }
    return 0
}

private let journalIDFormatter: DateFormatter = {
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.timeZone = .current
    f.dateFormat = "yyyy-MM-dd"
    return f
}()

private let journalSubtitleFormatter: DateFormatter = {
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.timeZone = .current
    f.dateFormat = "EEE, MMM d"
    return f
}()

private func journalDayID(_ date: Date) -> String {
    journalIDFormatter.string(from: date)
}

private func journalDate(from id: String) -> Date? {
    journalIDFormatter.date(from: id)
}

private func journalTitle(for date: Date) -> String {
    let cal = Calendar.current
    if cal.isDateInToday(date) { return "Today" }
    if cal.isDateInYesterday(date) { return "Yesterday" }
    let f = DateFormatter()
    f.locale = Locale(identifier: "en_US_POSIX")
    f.timeZone = .current
    f.dateFormat = "MMM d"
    return f.string(from: date)
}

private func journalSubtitle(for date: Date) -> String {
    journalSubtitleFormatter.string(from: date)
}

// MARK: - Journal page

struct JournalPageView: View {
    let service: MnemonicService

    @State private var selectedDate = Date()
    @State private var loadedDay: JournalDay? = nil
    @State private var isLoading = false
    @State private var errorText: String? = nil

    init(service: MnemonicService, previewDay: JournalDay? = nil) {
        self.service = service
        // Preview renderer injects a pre-fetched day so the snapshot shows
        // content instead of the async loading state.
        _loadedDay = State(initialValue: previewDay)
    }

    private var selectedID: String { journalDayID(selectedDate) }
    private var displayTitle: String { loadedDay?.title ?? journalTitle(for: selectedDate) }
    private var displaySubtitle: String { loadedDay?.subtitle ?? journalSubtitle(for: selectedDate) }
    private var canGoBack: Bool { true }
    private var canGoForward: Bool { selectedID < journalDayID(Date()) }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            header.padding(.bottom, 12)
            if isLoading && loadedDay == nil {
                loadingState
            } else if let day = loadedDay, !day.isEmpty {
                summarySection(day).padding(.bottom, 12)
                projectsSection(day).padding(.bottom, 12)
                decisionsSection(day)
            } else {
                emptyState
            }
        }
        .onAppear {
            if loadedDay == nil { loadDay(selectedDate) }
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                HStack(spacing: 6) {
                    Text("Journal")
                        .font(.system(size: 19, weight: .bold))
                        .tracking(-0.4)
                        .foregroundStyle(WT.text)
                    if isLoading {
                        ProgressView()
                            .scaleEffect(0.52)
                            .frame(width: 12, height: 12)
                    }
                }
                Text(displaySubtitle)
                    .font(.system(size: 11.5, weight: .semibold))
                    .foregroundStyle(WT.ter)
            }
            Spacer()
            HStack(spacing: 5) {
                navButton("chevron.left", enabled: canGoBack) {
                    moveDay(-1)
                }
                Text(displayTitle)
                    .font(.system(size: 11.5, weight: .bold))
                    .foregroundStyle(WT.sub)
                    .frame(minWidth: 66)
                navButton("chevron.right", enabled: canGoForward) {
                    moveDay(1)
                }
            }
        }
    }

    private func navButton(_ icon: String, enabled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 11, weight: .bold))
                .foregroundStyle(enabled ? WT.text : WT.ter.opacity(0.45))
                .frame(width: 25, height: 25)
                .background(RoundedRectangle(cornerRadius: 7).fill(WT.fill))
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    private var loadingState: some View {
        VStack(spacing: 12) {
            ProgressView()
                .scaleEffect(0.76)
            Text("Loading journal...")
                .font(.system(size: 12.5, weight: .semibold))
                .foregroundStyle(WT.ter)
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 64)
    }

    private var emptyState: some View {
        VStack(spacing: 12) {
            RoundedRectangle(cornerRadius: 12)
                .fill(WT.fill)
                .frame(width: 44, height: 44)
                .overlay(Image(systemName: "book.closed").font(.system(size: 19)).foregroundStyle(WT.ter))
            Text("No work recorded for this day.")
                .font(.system(size: 13.5, weight: .semibold))
                .foregroundStyle(WT.sub)
                .multilineTextAlignment(.center)
            if let errorText {
                Text(errorText)
                    .font(.system(size: 11.5, weight: .medium))
                    .foregroundStyle(WT.ter)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 230)
            }
        }
        .frame(maxWidth: .infinity)
        .padding(.top, 56)
    }

    private func summarySection(_ day: JournalDay) -> some View {
        VStack(alignment: .leading, spacing: 8) {
            sectionTitle("SUMMARY", icon: "text.alignleft")
            Text(day.summary)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(WT.sub)
                .lineSpacing(3)
                .fixedSize(horizontal: false, vertical: true)
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: 12).fill(WT.fill))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(WT.sep, lineWidth: 1))
    }

    private func projectsSection(_ day: JournalDay) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack {
                sectionTitle("BY PROJECT", icon: "folder")
                Spacer()
                if day.unattributedSeconds > 0.5 {
                    Text("Unattributed \(fmtDur(day.unattributedSeconds))")
                        .font(.system(size: 10.5, weight: .bold))
                        .monospacedDigit()
                        .foregroundStyle(WT.ter)
                }
            }
            .padding(.bottom, 4)

            ForEach(Array(day.projects.enumerated()), id: \.element.id) { i, project in
                if i > 0 { Rectangle().fill(WT.sep).frame(height: 1).padding(.leading, 24) }
                JournalProjectRow(project: project)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 12).fill(WT.fill))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(WT.sep, lineWidth: 1))
    }

    private func decisionsSection(_ day: JournalDay) -> some View {
        VStack(alignment: .leading, spacing: 0) {
            sectionTitle("DECISIONS & FOLLOW-UPS", icon: "checklist")
                .padding(.bottom, 5)

            let visibleDecisions = Array(day.decisions.prefix(3))
            ForEach(Array(visibleDecisions.enumerated()), id: \.element.id) { i, item in
                if i > 0 { Rectangle().fill(WT.sep).frame(height: 1).padding(.leading, 24) }
                JournalItemRow(item: item, icon: "lightbulb", color: WT.memDecision)
            }

            if day.decisions.count > visibleDecisions.count {
                Rectangle().fill(WT.sep).frame(height: 1).padding(.leading, 24)
                JournalMoreRow(count: day.decisions.count - visibleDecisions.count, label: "more decisions")
            }

            if !day.followUps.isEmpty && !visibleDecisions.isEmpty {
                Rectangle().fill(WT.sep).frame(height: 1).padding(.leading, 24)
            }

            let visibleFollowUps = Array(day.followUps.prefix(3))
            ForEach(Array(visibleFollowUps.enumerated()), id: \.element.id) { i, item in
                if i > 0 { Rectangle().fill(WT.sep).frame(height: 1).padding(.leading, 24) }
                JournalItemRow(item: item, icon: "arrow.triangle.2.circlepath", color: WT.accent)
            }

            if day.followUps.count > visibleFollowUps.count {
                Rectangle().fill(WT.sep).frame(height: 1).padding(.leading, 24)
                JournalMoreRow(count: day.followUps.count - visibleFollowUps.count, label: "more follow-ups")
            }

            if day.decisions.isEmpty && day.followUps.isEmpty {
                Text("No decisions or follow-ups captured.")
                    .font(.system(size: 12.5, weight: .medium))
                    .foregroundStyle(WT.ter)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.vertical, 10)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 12).fill(WT.fill))
        .overlay(RoundedRectangle(cornerRadius: 12).stroke(WT.sep, lineWidth: 1))
    }

    private func moveDay(_ delta: Int) {
        guard let next = Calendar.current.date(byAdding: .day, value: delta, to: selectedDate) else { return }
        guard delta < 0 || journalDayID(next) <= journalDayID(Date()) else { return }
        selectedDate = next
        loadedDay = nil
        loadDay(next)
    }

    private func loadDay(_ date: Date) {
        let id = journalDayID(date)
        isLoading = true
        errorText = nil
        DispatchQueue.global(qos: .utility).async {
            let obj = service.httpObject("/api/journal?day=\(id)")
            DispatchQueue.main.async {
                guard journalDayID(selectedDate) == id else { return }
                if let obj, let parsed = JournalDay.fromJSON(obj, fallbackDate: date) {
                    loadedDay = parsed
                    errorText = nil
                } else {
                    // No fabricated fallback on a live build — show the empty /
                    // error state rather than inventing a day's work.
                    loadedDay = nil
                    errorText = "Journal backend is not ready yet."
                }
                isLoading = false
            }
        }
    }

    private func sectionTitle(_ title: String, icon: String) -> some View {
        HStack(spacing: 6) {
            Image(systemName: icon)
                .font(.system(size: 11, weight: .bold))
                .foregroundStyle(WT.ter)
            Text(title)
                .font(.system(size: 10.5, weight: .bold))
                .tracking(0.6)
                .foregroundStyle(WT.ter)
        }
    }
}

struct JournalProjectRow: View {
    let project: JournalProject

    var body: some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(alignment: .firstTextBaseline, spacing: 7) {
                Text(project.name)
                    .font(.system(size: 13.5, weight: .bold))
                    .tracking(-0.2)
                    .foregroundStyle(WT.text)
                    .lineLimit(1)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Text(fmtDur(project.seconds))
                    .font(.system(size: 12.5, weight: .bold))
                    .monospacedDigit()
                    .foregroundStyle(WT.text)
                ConfidenceDot(confidence: project.confidence)
            }

            VStack(alignment: .leading, spacing: 5) {
                if !project.events.isEmpty {
                    ForEach(Array(project.events.prefix(3).enumerated()), id: \.element.id) { _, event in
                        JournalEventRow(event: event)
                    }
                } else {
                    ForEach(Array(project.bullets.prefix(3).enumerated()), id: \.offset) { _, bullet in
                        JournalBulletRow(text: bullet)
                    }
                    if project.bullets.isEmpty {
                        JournalTrackedOnlyRow()
                    }
                }
            }
        }
        .padding(.vertical, 9)
    }
}

struct JournalTrackedOnlyRow: View {
    var body: some View {
        HStack(alignment: .top, spacing: 7) {
            Image(systemName: "clock")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(WT.ter.opacity(0.8))
                .frame(width: 14)
                .padding(.top, 1)
            Text("Time tracked, no captured note yet.")
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(WT.ter)
                .lineLimit(1)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct JournalEventRow: View {
    let event: JournalEvent

    var body: some View {
        HStack(alignment: .top, spacing: 7) {
            if !event.timeLabel.isEmpty {
                Text(event.timeLabel)
                    .font(.system(size: 10.5, weight: .bold))
                    .monospacedDigit()
                    .foregroundStyle(WT.ter)
                    .frame(width: 48, alignment: .leading)
                    .padding(.top, 1)
                Text("·")
                    .font(.system(size: 11, weight: .bold))
                    .foregroundStyle(WT.ter.opacity(0.65))
                    .padding(.top, 1)
            }
            Text(event.text)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(WT.sub)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

struct JournalBulletRow: View {
    let text: String

    var body: some View {
        HStack(alignment: .top, spacing: 6) {
            Circle()
                .fill(WT.accent.opacity(0.75))
                .frame(width: 4, height: 4)
                .padding(.top, 6)
            Text(text)
                .font(.system(size: 12, weight: .medium))
                .foregroundStyle(WT.sub)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
        }
    }
}

struct JournalMoreRow: View {
    let count: Int
    let label: String

    var body: some View {
        Text("+\(count) \(label)")
            .font(.system(size: 11.5, weight: .bold))
            .foregroundStyle(WT.ter)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.vertical, 8)
            .padding(.leading, 25)
    }
}

struct JournalItemRow: View {
    let item: JournalItem
    let icon: String
    let color: Color

    var body: some View {
        HStack(alignment: .top, spacing: 9) {
            Image(systemName: icon)
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(color)
                .frame(width: 16, height: 18)
            // memoryID is kept on the model for a future tap-to-open, but not
            // rendered — on real data it's a raw UUID, not something to show.
            Text(item.title)
                .font(.system(size: 12.5, weight: .semibold))
                .foregroundStyle(WT.text)
                .lineLimit(2)
                .fixedSize(horizontal: false, vertical: true)
                .frame(maxWidth: .infinity, alignment: .leading)
            Spacer(minLength: 0)
        }
        .padding(.vertical, 9)
    }
}
