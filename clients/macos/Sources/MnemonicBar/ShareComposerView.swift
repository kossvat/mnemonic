import AppKit
import SwiftUI
import UniformTypeIdentifiers

enum ShareMode: String, CaseIterable {
    case today, week, clean

    var label: String {
        switch self {
        case .today: return "Today"
        case .week: return "Week"
        case .clean: return "Clean"
        }
    }
}

enum ShareTone: String {
    case light, dark

    var colorScheme: ColorScheme { self == .dark ? .dark : .light }
    var icon: String { self == .dark ? "moon.fill" : "sun.max.fill" }
}

struct ShareComposerView: View {
    let data: WidgetData
    /// Optional — when present, the day card can page back to earlier days
    /// (fetched on demand). nil (e.g. preview renderer) keeps it on today.
    var service: MnemonicService? = nil
    /// When embedded as the deck's Share page: fill the page width and drop
    /// the close button (the pager handles navigation).
    var inPage: Bool = false

    @Environment(\.dismiss) private var dismiss
    @State private var mode: ShareMode = .today
    @State private var tone: ShareTone = .light
    @State private var includeMemory = false

    // Day selection for the day card. 0 = today, -1 = yesterday, …
    @State private var dayOffset = 0
    @State private var loadedDays: [String: WorkDay] = [:]
    @State private var loadingDay = false

    /// Day cards (today/clean) support paging; the week card doesn't.
    private var dayCardMode: Bool { mode != .week }

    /// The day currently shown on the card. Today comes straight from
    /// `data`; earlier days are fetched into `loadedDays`.
    private var selectedDay: WorkDay? {
        if dayOffset == 0 { return data.today ?? data.days.last }
        return loadedDays[dateString(forOffset: dayOffset)]
    }

    /// Worked seconds for the selected day. Today keeps the summary's
    /// `worked_today` (matches the rest of the widget); earlier days use
    /// the fetched day total.
    private var selectedWorkedSeconds: Double {
        if dayOffset == 0 { return data.workedTodaySeconds }
        return selectedDay?.seconds ?? 0
    }

    private var canPageForward: Bool { dayOffset < 0 }
    // A year of history is plenty; the backend answers any date but we
    // don't want an unbounded back button.
    private var canPageBack: Bool { dayOffset > -365 }

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            header

            SegControl(
                items: ShareMode.allCases.map { ($0.rawValue, $0.label, nil) },
                value: mode.rawValue
            ) { raw in
                mode = ShareMode(rawValue: raw) ?? .today
            }

            if dayCardMode && service != nil { dayPager }

            preview

            Toggle(isOn: $includeMemory) {
                VStack(alignment: .leading, spacing: 1) {
                    Text("Include latest memory")
                        .font(.system(size: 13, weight: .semibold))
                        .foregroundStyle(WT.text)
                    Text(includeMemory ? "On" : "Off")
                        .font(.system(size: 11.5, weight: .medium))
                        .foregroundStyle(WT.ter)
                }
            }
            .toggleStyle(.switch)
            .disabled(data.latest == nil)
            .padding(.horizontal, 12)
            .padding(.vertical, 9)
            .background(RoundedRectangle(cornerRadius: WT.R.inner).fill(WT.fill))

            HStack(spacing: 9) {
                PrimaryButton(icon: "arrow.down.to.line", label: "Save Image") { saveImage() }
                composerButton(icon: "doc.on.doc", label: "Copy") { copyImage() }
                composerButton(icon: "square.and.arrow.up", label: "Share") { shareImage() }
            }
        }
        .padding(16)
        .frame(maxWidth: inPage ? .infinity : nil)
        .frame(width: inPage ? nil : 414)
        .background(WT.bg)
    }

    private var header: some View {
        HStack {
            Text("Share work summary")
                .font(.system(size: 17, weight: .bold))
                .foregroundStyle(WT.text)
            Spacer()
            Button {
                tone = tone == .light ? .dark : .light
            } label: {
                Image(systemName: tone.icon)
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(WT.sub)
                    .frame(width: 28, height: 28)
                    .background(RoundedRectangle(cornerRadius: 8).fill(WT.btnFill))
            }
            .buttonStyle(.plain)
            .help(tone == .light ? "Use dark card" : "Use light card")

            if !inPage {
                Button { dismiss() } label: {
                    Image(systemName: "xmark")
                        .font(.system(size: 12, weight: .bold))
                        .foregroundStyle(WT.ter)
                        .frame(width: 28, height: 28)
                        .background(RoundedRectangle(cornerRadius: 8).fill(WT.btnFill))
                }
                .buttonStyle(.plain)
                .help("Close")
            }
        }
    }

    /// Back / date / forward control for paging the day card.
    private var dayPager: some View {
        HStack(spacing: 8) {
            pagerButton("chevron.left", enabled: canPageBack) { step(-1) }
            HStack(spacing: 6) {
                Text(pagerTitle)
                    .font(.system(size: 12.5, weight: .bold))
                    .foregroundStyle(WT.text)
                if loadingDay {
                    ProgressView().scaleEffect(0.5).frame(width: 10, height: 10)
                }
            }
            .frame(maxWidth: .infinity)
            pagerButton("chevron.right", enabled: canPageForward) { step(1) }
        }
        .padding(.horizontal, 4)
    }

    private func pagerButton(_ icon: String, enabled: Bool, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: icon)
                .font(.system(size: 11, weight: .bold))
                .foregroundStyle(enabled ? WT.text : WT.ter.opacity(0.4))
                .frame(width: 30, height: 26)
                .background(RoundedRectangle(cornerRadius: 7).fill(WT.btnFill))
        }
        .buttonStyle(.plain)
        .disabled(!enabled)
    }

    private var pagerTitle: String {
        switch dayOffset {
        case 0: return "Today"
        case -1: return "Yesterday"
        default: return weekdayLongLabel(forOffset: dayOffset)
        }
    }

    private func step(_ delta: Int) {
        let next = dayOffset + delta
        guard next <= 0, next >= -365 else { return }
        dayOffset = next
        if next != 0 { loadDay(offset: next) }
    }

    private func loadDay(offset: Int) {
        let ds = dateString(forOffset: offset)
        if loadedDays[ds] != nil { return } // cached
        guard let service else { return }
        loadingDay = true
        service.loadDayDetail(date: ds) { day in
            loadingDay = false
            if let day { loadedDays[ds] = day }
        }
    }

    private var preview: some View {
        ZStack {
            RoundedRectangle(cornerRadius: WT.R.inner).fill(WT.fill)
            ShareCardView(data: data, mode: mode, tone: tone, includeMemory: includeMemory,
                          day: selectedDay, workedSeconds: selectedWorkedSeconds,
                          dateLabel: cardDateLabel)
                .frame(width: ShareCardView.cardSize.width, height: ShareCardView.cardSize.height)
                .scaleEffect(0.58)
                .frame(width: ShareCardView.cardSize.width * 0.58,
                       height: ShareCardView.cardSize.height * 0.58)
                .shadow(color: .black.opacity(tone == .dark ? 0.28 : 0.13), radius: 16, y: 8)
        }
        .frame(maxWidth: .infinity)
        .frame(height: 376)
        .overlay(RoundedRectangle(cornerRadius: WT.R.inner).stroke(WT.sep, lineWidth: 1))
    }

    private func composerButton(icon: String, label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 7) {
                Image(systemName: icon).font(.system(size: 13, weight: .semibold))
                Text(label).font(.system(size: 13, weight: .semibold))
            }
            .foregroundStyle(WT.text)
            .frame(maxWidth: .infinity)
            .frame(height: 38)
            .background(RoundedRectangle(cornerRadius: WT.R.btn).fill(WT.btnFill))
        }
        .buttonStyle(.plain)
    }

    @MainActor private func renderImage() -> NSImage? {
        let content = ShareCardView(data: data, mode: mode, tone: tone, includeMemory: includeMemory,
                                    day: selectedDay, workedSeconds: selectedWorkedSeconds,
                                    dateLabel: cardDateLabel)
            .frame(width: ShareCardView.cardSize.width, height: ShareCardView.cardSize.height)
            .environment(\.colorScheme, tone.colorScheme)
        let renderer = ImageRenderer(content: content)
        renderer.scale = 2
        // Force the appearance so NSColor-backed dynamic colors (the shared
        // WT.* tokens used by SessionTimelineView inside the card) resolve to
        // the CHOSEN card tone, not the system appearance. Without this a
        // light card could render the timeline track in dark colors.
        let saved = NSAppearance.current
        NSAppearance.current = NSAppearance(named: tone == .dark ? .darkAqua : .aqua)
        defer { NSAppearance.current = saved }
        return renderer.nsImage
    }

    @MainActor private func saveImage() {
        guard let image = renderImage(), let data = pngData(from: image) else { return }
        let panel = NSSavePanel()
        panel.allowedContentTypes = [.png]
        panel.canCreateDirectories = true
        let datePart = mode == .week ? fileDate() : dateString(forOffset: dayOffset)
        panel.nameFieldStringValue = "Mnemonic-\(mode.rawValue)-\(datePart).png"
        guard panel.runModal() == .OK, let url = panel.url else { return }
        try? data.write(to: url, options: .atomic)
    }

    @MainActor private func copyImage() {
        guard let image = renderImage() else { return }
        let pb = NSPasteboard.general
        pb.clearContents()
        pb.writeObjects([image])
    }

    @MainActor private func shareImage() {
        guard let image = renderImage(), let view = NSApp.keyWindow?.contentView else {
            copyImage()
            return
        }
        NSSharingServicePicker(items: [image]).show(relativeTo: .zero, of: view, preferredEdge: .minY)
    }

    private func pngData(from image: NSImage) -> Data? {
        guard let tiff = image.tiffRepresentation,
              let rep = NSBitmapImageRep(data: tiff)
        else { return nil }
        return rep.representation(using: .png, properties: [:])
    }

    private func fileDate() -> String {
        let f = DateFormatter()
        f.dateFormat = "yyyy-MM-dd"
        return f.string(from: Date())
    }

    // MARK: Day-offset date helpers

    private func date(forOffset offset: Int) -> Date {
        Calendar.current.date(byAdding: .day, value: offset, to: Date()) ?? Date()
    }

    private func dateString(forOffset offset: Int) -> String {
        let f = DateFormatter()
        f.locale = Locale(identifier: "en_US_POSIX")
        f.dateFormat = "yyyy-MM-dd"
        return f.string(from: date(forOffset: offset))
    }

    /// Header label on the card itself, e.g. "Wed · Jun 11".
    private var cardDateLabel: String {
        let f = DateFormatter()
        f.dateFormat = "EEE · MMM d"
        return f.string(from: date(forOffset: mode == .week ? 0 : dayOffset))
    }

    private func weekdayLongLabel(forOffset offset: Int) -> String {
        let f = DateFormatter()
        f.dateFormat = "EEE, MMM d"
        return f.string(from: date(forOffset: offset))
    }
}

struct ShareCardView: View {
    static let cardSize = CGSize(width: 360, height: 560)

    let data: WidgetData
    let mode: ShareMode
    let tone: ShareTone
    let includeMemory: Bool
    /// The day this card renders (today or a paged-back earlier day).
    var day: WorkDay? = nil
    /// Worked seconds for `day` (today uses the summary's worked_today).
    var workedSeconds: Double? = nil
    /// Header date label, e.g. "Wed · Jun 11".
    var dateLabel: String? = nil

    private var shownDay: WorkDay? { day ?? data.today ?? data.days.last }
    private var shownWorked: Double { workedSeconds ?? data.workedTodaySeconds }
    private var cardMemory: LatestMemory? { includeMemory ? data.latest : nil }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            cardHeader
            Spacer(minLength: mode == .week ? 44 : 54)
            mainBlock
            Spacer(minLength: 0)
            if let cardMemory { memoryBlock(cardMemory) }
            footer
        }
        .padding(.horizontal, 32)
        .padding(.vertical, 29)
        .frame(width: Self.cardSize.width, height: Self.cardSize.height, alignment: .topLeading)
        .background(background)
        .overlay(RoundedRectangle(cornerRadius: 26).stroke(stroke, lineWidth: 1))
        .clipShape(RoundedRectangle(cornerRadius: 26))
        .environment(\.colorScheme, tone.colorScheme)
    }

    private var cardHeader: some View {
        HStack(spacing: 10) {
            ZStack {
                RoundedRectangle(cornerRadius: 7).fill(accent.opacity(tone == .dark ? 0.16 : 0.10))
                Image(systemName: "brain.head.profile")
                    .font(.system(size: 13, weight: .semibold))
                    .foregroundStyle(accent)
            }
            .frame(width: 25, height: 25)
            Text("Mnemonic")
                .font(.system(size: 15, weight: .bold))
                .foregroundStyle(primary)
            Spacer()
            Text(displayDate())
                .font(.system(size: 13, weight: .medium))
                .foregroundStyle(secondary)
        }
    }

    @ViewBuilder private var mainBlock: some View {
        switch mode {
        case .today:
            todayBlock(showTimeline: true)
        case .week:
            weekBlock
        case .clean:
            todayBlock(showTimeline: false)
        }
    }

    private func todayBlock(showTimeline: Bool) -> some View {
        let isToday = shownDay?.isToday ?? true
        return VStack(alignment: .leading, spacing: 18) {
            label(isToday ? "WORKED TODAY" : "WORKED")
            timeLine(seconds: shownWorked)
            HStack(spacing: 0) {
                Text("\(shownDay?.sessionCount ?? 0) sessions")
                Text(" · ")
                Text("longest \(fmtDur(shownDay?.longestSeconds ?? 0))")
            }
            .font(.system(size: 14, weight: .semibold))
            .foregroundStyle(secondary)

            if showTimeline {
                VStack(alignment: .leading, spacing: 9) {
                    label(isToday ? "TODAY'S SESSIONS" : "SESSIONS")
                    SessionTimelineView(sessions: shownDay?.sessions ?? [], working: false)
                        .frame(height: 37)
                }
                .padding(.top, 19)
            }
        }
    }

    private var weekBlock: some View {
        VStack(alignment: .leading, spacing: 18) {
            label("THIS WEEK")
            timeLine(seconds: data.week?.totalSeconds ?? 0)
            Text(weekSubline)
                .font(.system(size: 14, weight: .semibold))
                .foregroundStyle(secondary)
                .lineLimit(2)
            ShareWeekBars(days: data.days, accent: accent, idle: barIdle,
                          text: secondary, selected: bestDayIndex)
                .frame(height: 132)
                .padding(.top, 22)
        }
    }

    private var weekSubline: String {
        var parts: [String] = []
        if let best = data.week?.bestWeekday { parts.append("Best day \(best)") }
        if let delta = data.week?.deltaSeconds {
            parts.append((delta >= 0 ? "+" : "-") + fmtDur(abs(delta)) + " vs avg")
        }
        return parts.isEmpty ? "Tracked with Mnemonic" : parts.joined(separator: " · ")
    }

    private var bestDayIndex: Int {
        guard let maxValue = data.days.map(\.seconds).max(),
              maxValue > 0,
              let i = data.days.firstIndex(where: { $0.seconds == maxValue })
        else { return Swift.max(data.days.count - 1, 0) }
        return i
    }

    private func label(_ text: String) -> some View {
        Text(text)
            .font(.system(size: 12, weight: .bold))
            .tracking(1.6)
            .foregroundStyle(accent)
    }

    private func timeLine(seconds: Double) -> some View {
        let parts = hm(seconds)
        return HStack(alignment: .firstTextBaseline, spacing: 5) {
            if parts.h > 0 {
                Text("\(parts.h)").shareNum()
                Text("h").shareUnit()
            }
            Text("\(parts.m)").shareNum()
            Text("m").shareUnit()
        }
    }

    private func memoryBlock(_ mem: LatestMemory) -> some View {
        VStack(alignment: .leading, spacing: 7) {
            HStack(spacing: 7) {
                Image(systemName: memoryIcon(mem.type))
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(accent)
                Text(mem.title)
                    .font(.system(size: 12, weight: .bold))
                    .foregroundStyle(primary)
                    .lineLimit(1)
                Spacer(minLength: 0)
            }
            Text(mem.content)
                .font(.system(size: 11.5, weight: .medium))
                .lineSpacing(2)
                .foregroundStyle(secondary)
                .lineLimit(3)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 12).fill(cardFill))
        .padding(.bottom, 22)
    }

    private var footer: some View {
        VStack(spacing: 17) {
            Rectangle().fill(stroke).frame(height: 1)
            HStack(spacing: 6) {
                Circle().fill(accent).frame(width: 4, height: 4)
                Text("tracked with Mnemonic")
                    .font(.system(size: 12, weight: .semibold))
                    .foregroundStyle(secondary)
            }
            .frame(maxWidth: .infinity)
        }
    }

    private func displayDate() -> String {
        if let dateLabel { return dateLabel }
        let f = DateFormatter()
        f.dateFormat = "EEE · MMM d"
        return f.string(from: Date())
    }

    private func memoryIcon(_ type: String) -> String {
        switch type {
        case "decision": return "lightbulb.fill"
        case "feedback": return "bubble.left.fill"
        default: return "doc.text.fill"
        }
    }

    private var background: Color {
        tone == .dark ? Color(hex: 0x1C130D) : Color(hex: 0xFFF4E4)
    }
    private var primary: Color {
        tone == .dark ? Color(hex: 0xFFF7EE) : Color(hex: 0x21160E)
    }
    private var secondary: Color {
        tone == .dark ? Color(hex: 0xFFF2E3, opacity: 0.68) : Color(hex: 0x443121, opacity: 0.68)
    }
    private var accent: Color {
        tone == .dark ? Color(hex: 0xF2A35E) : Color(hex: 0xC9722C)
    }
    private var stroke: Color {
        tone == .dark ? Color.white.opacity(0.10) : Color.black.opacity(0.08)
    }
    private var cardFill: Color {
        tone == .dark ? Color.white.opacity(0.055) : Color.white.opacity(0.48)
    }
    private var barIdle: Color {
        tone == .dark ? Color.white.opacity(0.13) : Color.black.opacity(0.12)
    }
}

private struct ShareWeekBars: View {
    let days: [WorkDay]
    let accent: Color
    let idle: Color
    let text: Color
    let selected: Int

    var body: some View {
        GeometryReader { geo in
            let n = max(days.count, 1)
            let w = geo.size.width
            let h = geo.size.height
            let labelH: CGFloat = 20
            let plotH = h - labelH
            let step = w / CGFloat(n)
            let maxSec = max(3600.0, days.map(\.seconds).max() ?? 0)

            ZStack(alignment: .bottomLeading) {
                ForEach(Array(days.enumerated()), id: \.offset) { i, day in
                    let barW = step * 0.52
                    let barH = max(5, plotH * CGFloat(min(day.seconds, maxSec) / maxSec))
                    VStack(spacing: 7) {
                        RoundedRectangle(cornerRadius: 5)
                            .fill(i == selected ? accent : idle)
                            .frame(width: barW, height: barH)
                            .frame(height: plotH, alignment: .bottom)
                        Text(day.dowLetter)
                            .font(.system(size: 11, weight: i == selected ? .bold : .semibold))
                            .foregroundStyle(i == selected ? accent : text)
                            .frame(width: step)
                    }
                    .position(x: step * (CGFloat(i) + 0.5), y: h / 2)
                }
            }
        }
    }
}

private extension Text {
    func shareNum() -> some View {
        self.font(.system(size: 62, weight: .bold)).tracking(-1.6)
            .monospacedDigit()
    }
    func shareUnit() -> some View {
        self.font(.system(size: 27, weight: .semibold))
    }
}
