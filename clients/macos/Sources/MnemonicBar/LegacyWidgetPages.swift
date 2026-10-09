import SwiftUI
import MnemonicShared

// MARK: - Projects page

struct ProjectsPageView: View {
    let data: WidgetData
    let onOpenApp: () -> Void

    @State private var mode = "week"          // today / week
    @State private var openKey: String? = nil
    @State private var showWaiting = false

    private var projects: [Project] { data.projects }
    private func val(_ p: Project) -> Double? { mode == "today" ? p.todaySeconds : p.weekSeconds }
    private func soon(_ p: Project) -> Bool { !p.tracking || val(p) == nil }
    private var unattributedValue: Double {
        guard let u = data.unattributed else { return 0 }
        return mode == "today" ? u.todaySeconds : u.weekSeconds
    }

    var body: some View {
        if let key = openKey, let p = projects.first(where: { project in project.key == key }) {
            ProjectDetailView(data: data, project: p, onBack: { openKey = nil }, onOpenApp: onOpenApp)
        } else if projects.isEmpty {
            emptyState
        } else {
            list
        }
    }

    private var list: some View {
        let ranked = projects.sorted { first, second in (val(first) ?? -1) > (val(second) ?? -1) }
        // Projects with real time lead; the long tail without attributed
        // time used to render as a wall of identical "tracking soon" rows
        // (9 of 12 on real data) drowning the projects that matter.
        let tracked = ranked.filter { project in !soon(project) }
        let waiting = ranked.filter { project in soon(project) }
        let unattr = unattributedValue
        // Bars scale against real project time only — Unattributed can be
        // huge ("low signal" noise) and used to flatten every project bar.
        let maxV = max(1, tracked.map { project in val(project) ?? 0 }.max() ?? 0)
        let attributedTotal = projects.reduce(0.0) { total, project in total + (val(project) ?? 0) }
        return VStack(alignment: .leading, spacing: 0) {
            HStack {
                Text("Projects").font(.system(size: 19, weight: .bold)).tracking(-0.4).foregroundStyle(WT.text)
                Spacer()
                SegControl(items: [("today", "Today", nil), ("week", "Week", nil)],
                           value: mode) { selected in mode = selected }
            }
            .padding(.bottom, 12)

            (Text(fmtDur(attributedTotal)).font(.system(size: 11.5, weight: .bold)).foregroundStyle(WT.sub).monospacedDigit()
             + Text(" attributed \(mode == "today" ? "today" : "this week") · \(projects.count) projects")
                .font(.system(size: 11.5, weight: .medium)).foregroundStyle(WT.ter))
                .padding(.bottom, 6)

            ForEach(Array(tracked.enumerated()), id: \.element.id) { i, p in
                if i > 0 { Rectangle().fill(WT.sep).frame(height: 1) }
                ProjectRowView(project: p, value: val(p), maxValue: maxV) { openKey = p.key }
            }
            if unattr > 0.5 {
                if !tracked.isEmpty { Rectangle().fill(WT.sep).frame(height: 1) }
                UnattributedProjectRowView(value: unattr, maxValue: maxV)
            }
            if !waiting.isEmpty {
                Rectangle().fill(WT.sep).frame(height: 1)
                Button {
                    withAnimation(.easeOut(duration: 0.18)) { showWaiting.toggle() }
                } label: {
                    HStack(spacing: 6) {
                        Image(systemName: "chevron.right")
                            .font(.system(size: 10, weight: .bold))
                            .foregroundStyle(WT.ter)
                            .rotationEffect(.degrees(showWaiting ? 90 : 0))
                        Text("\(waiting.count) projects waiting for time data")
                            .font(.system(size: 12, weight: .semibold))
                            .foregroundStyle(WT.ter)
                        Spacer(minLength: 0)
                    }
                    .padding(.vertical, 10)
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain)
                if showWaiting {
                    ForEach(Array(waiting.enumerated()), id: \.element.id) { i, p in
                        if i > 0 { Rectangle().fill(WT.sep).frame(height: 1) }
                        ProjectRowView(project: p, value: val(p), maxValue: maxV) { openKey = p.key }
                    }
                }
            }
        }
    }

    private var emptyState: some View {
        VStack(spacing: 14) {
            RoundedRectangle(cornerRadius: 12).fill(WT.fill).frame(width: 44, height: 44)
                .overlay(Image(systemName: "folder").font(.system(size: 20)).foregroundStyle(WT.ter))
            Text("No project activity yet — it appears as you work in your repos.")
                .font(.system(size: 13.5, weight: .semibold)).foregroundStyle(WT.sub)
                .multilineTextAlignment(.center).lineSpacing(3).frame(maxWidth: 230)
        }
        .frame(maxWidth: .infinity).padding(.top, 60).padding(.horizontal, 16)
    }
}

struct ProjectRowView: View {
    let project: Project
    let value: Double?
    let maxValue: Double
    let onOpen: () -> Void

    private var soon: Bool { !project.tracking || value == nil }

    var body: some View {
        Button(action: onOpen) {
            HStack(spacing: 11) {
                VStack(alignment: .leading, spacing: 7) {
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(project.name).font(.system(size: 14, weight: .semibold)).tracking(-0.2)
                            .foregroundStyle(WT.text).lineLimit(1).truncationMode(.tail)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if soon {
                            Text("tracking soon").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(WT.ter)
                        } else {
                            HStack(spacing: 5) {
                                Text(fmtDur(value ?? 0)).font(.system(size: 13.5, weight: .bold))
                                    .monospacedDigit().foregroundStyle(WT.text)
                                ConfidenceDot(confidence: project.confidence)
                            }
                        }
                    }
                    HStack(spacing: 10) {
                        MiniShareBar(value: soon ? nil : value, maxValue: maxValue)
                        Text("\(project.memCount) mem").font(.system(size: 11, weight: .semibold))
                            .monospacedDigit().foregroundStyle(WT.ter)
                    }
                }
                Image(systemName: "chevron.right").font(.system(size: 12, weight: .semibold)).foregroundStyle(WT.ter)
            }
            .padding(.vertical, 10).padding(.horizontal, 2)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain)
    }
}

struct UnattributedProjectRowView: View {
    let value: Double
    let maxValue: Double

    var body: some View {
        HStack(spacing: 11) {
            VStack(alignment: .leading, spacing: 7) {
                HStack(alignment: .firstTextBaseline, spacing: 8) {
                    Text("Unattributed").font(.system(size: 14, weight: .semibold)).tracking(-0.2)
                        .foregroundStyle(WT.sub).lineLimit(1)
                        .frame(maxWidth: .infinity, alignment: .leading)
                    Text(fmtDur(value)).font(.system(size: 13.5, weight: .bold))
                        .monospacedDigit().foregroundStyle(WT.ter)
                }
                HStack(spacing: 10) {
                    MiniShareBar(value: value, maxValue: maxValue).opacity(0.45)
                    Text("low signal").font(.system(size: 11, weight: .semibold))
                        .foregroundStyle(WT.ter)
                }
            }
            Image(systemName: "questionmark.circle")
                .font(.system(size: 12, weight: .semibold))
                .foregroundStyle(WT.ter)
        }
        .padding(.vertical, 10).padding(.horizontal, 2)
        .help("Time Mnemonic could not honestly assign to a project")
    }
}

struct MiniShareBar: View {
    let value: Double?
    let maxValue: Double
    var body: some View {
        GeometryReader { geo in
            ZStack(alignment: .leading) {
                RoundedRectangle(cornerRadius: 3).fill(WT.barIdle)
                if let v = value, v > 0, maxValue > 0 {
                    // Cap at 1: Unattributed can exceed the project max it
                    // is drawn against, and an overflowing bar reads as a
                    // rendering bug.
                    let pct = min(1, max(0.03, v / maxValue))
                    RoundedRectangle(cornerRadius: 3)
                        .fill(LinearGradient(colors: [WT.accent.opacity(0.75), WT.accent],
                                             startPoint: .leading, endPoint: .trailing))
                        .frame(width: geo.size.width * pct)
                }
            }
        }
        .frame(height: 6)
        .frame(maxWidth: .infinity)
    }
}

struct ProjectDetailView: View {
    let data: WidgetData
    let project: Project
    let onBack: () -> Void
    let onOpenApp: () -> Void

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: onBack) {
                HStack(spacing: 3) {
                    Image(systemName: "chevron.left").font(.system(size: 11, weight: .semibold))
                    Text("Projects").font(.system(size: 13, weight: .semibold))
                }.foregroundStyle(WT.accent)
            }.buttonStyle(.plain).padding(.bottom, 10)

            Text(project.name).font(.system(size: 18, weight: .bold)).tracking(-0.3).foregroundStyle(WT.text)
                .padding(.bottom, 2)
            HStack(alignment: .firstTextBaseline, spacing: 6) {
                if project.tracking {
                    Text("This week").font(.system(size: 12.5, weight: .medium)).foregroundStyle(WT.sub)
                    Text(fmtDur(project.weekSeconds ?? 0)).font(.system(size: 12.5, weight: .bold))
                        .monospacedDigit().foregroundStyle(WT.text)
                    ConfidenceDot(confidence: project.confidence)
                    Text("· \(project.memCount) memories").font(.system(size: 12.5, weight: .medium)).foregroundStyle(WT.sub)
                } else {
                    Text("\(project.memCount) memories · time tracking coming soon")
                        .font(.system(size: 12.5, weight: .medium)).foregroundStyle(WT.sub)
                }
            }
            .padding(.bottom, 14)

            chartCard.padding(.bottom, 16)

            Text("LATEST MEMORIES").font(.system(size: 10.5, weight: .bold)).tracking(0.6)
                .foregroundStyle(WT.ter).padding(.bottom, 4)
            ForEach(Array(project.mems.enumerated()), id: \.element.id) { i, m in
                if i > 0 { Rectangle().fill(WT.sep).frame(height: 1) }
                HStack(spacing: 10) {
                    Image(systemName: memIcon(m.type)).font(.system(size: 14, weight: .medium)).foregroundStyle(memColor(m.type))
                    Text(m.title).font(.system(size: 13, weight: .semibold)).foregroundStyle(WT.text)
                        .lineLimit(1).truncationMode(.tail).frame(maxWidth: .infinity, alignment: .leading)
                    Text(fmtAgo(m.agoMinutes)).font(.system(size: 11, weight: .medium)).foregroundStyle(WT.ter)
                }.padding(.vertical, 9)
            }
            Button("View all in app →", action: onOpenApp).buttonStyle(.plain)
                .font(.system(size: 13, weight: .semibold)).foregroundStyle(WT.accent).padding(.top, 12)
        }
    }

    @ViewBuilder private var chartCard: some View {
        VStack {
            if project.tracking && !project.week.isEmpty {
                let days = projectWeekDays()
                WorkChartView(series: days, type: .bar, range: .week, selected: days.count - 1, height: 92) { _ in }
            } else {
                VStack(spacing: 6) {
                    Image(systemName: "clock").font(.system(size: 20)).foregroundStyle(WT.ter)
                    Text("Time tracking coming soon").font(.system(size: 12, weight: .semibold)).foregroundStyle(WT.ter)
                }.frame(height: 92).frame(maxWidth: .infinity)
            }
        }
        .padding(.horizontal, 13).padding(.vertical, 12)
        .background(RoundedRectangle(cornerRadius: 13).fill(WT.fill))
        .overlay(RoundedRectangle(cornerRadius: 13).stroke(WT.sep, lineWidth: 1))
    }

    /// Build a 7-day series from the project's per-day seconds, dated by the
    /// global day axis so the chart labels line up.
    private func projectWeekDays() -> [WorkDay] {
        let base = data.days.suffix(7)
        return Array(base.enumerated().map { i, day in
            let secs = i < project.week.count ? project.week[i] : 0
            return WorkDay(date: day.date, seconds: secs, isToday: day.isToday,
                           dowLetter: day.dowLetter, dayNum: day.dayNum, dow: day.dow, label: day.label)
        })
    }
}

// MARK: - Memory page

struct MemoryPageView: View {
    let data: WidgetData
    let onOpenApp: () -> Void

    @State private var query = ""
    @State private var filter = "all"   // all / decision / feedback / note

    private let chips = [("all", "All"), ("decision", "Decisions"),
                         ("feedback", "Feedback"), ("note", "Notes")]

    var body: some View {
        if data.recent.isEmpty {
            VStack(alignment: .leading, spacing: 0) {
                Text("Memory").font(.system(size: 19, weight: .bold)).tracking(-0.4).foregroundStyle(WT.text)
                VStack(spacing: 14) {
                    RoundedRectangle(cornerRadius: 12).fill(WT.fill).frame(width: 44, height: 44)
                        .overlay(Image(systemName: "lightbulb").font(.system(size: 20)).foregroundStyle(WT.ter))
                    Text("No memories yet — they appear here as you work and talk with your agents.")
                        .font(.system(size: 13.5, weight: .semibold)).foregroundStyle(WT.sub)
                        .multilineTextAlignment(.center).lineSpacing(3).frame(maxWidth: 230)
                }.frame(maxWidth: .infinity).padding(.top, 56)
            }
        } else {
            content
        }
    }

    private var filtered: [LatestMemory] {
        data.recent.filter { m in
            (filter == "all" || m.type == filter)
            && (query.isEmpty || (m.title + " " + m.content).localizedCaseInsensitiveContains(query))
        }
    }

    private var content: some View {
        VStack(alignment: .leading, spacing: 0) {
            Text("Memory").font(.system(size: 19, weight: .bold)).tracking(-0.4).foregroundStyle(WT.text)
                .padding(.bottom, 12)

            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").font(.system(size: 13)).foregroundStyle(WT.ter)
                TextField("Search memories…", text: $query)
                    .textFieldStyle(.plain).font(.system(size: 13)).foregroundStyle(WT.text)
                if !query.isEmpty {
                    Button { query = "" } label: { Image(systemName: "xmark.circle.fill").font(.system(size: 12)).foregroundStyle(WT.ter) }
                        .buttonStyle(.plain)
                }
            }
            .padding(.horizontal, 11).frame(height: 34)
            .background(RoundedRectangle(cornerRadius: 9).fill(WT.fill))
            .overlay(RoundedRectangle(cornerRadius: 9).stroke(WT.sep, lineWidth: 1))
            .padding(.bottom, 11)

            HStack(spacing: 6) {
                ForEach(chips, id: \.0) { v, label in
                    let on = filter == v
                    Button { filter = v } label: {
                        Text(label).font(.system(size: 11.5, weight: .semibold))
                            .foregroundStyle(on ? .white : WT.sub)
                            .padding(.horizontal, 11).padding(.vertical, 4)
                            .background(Capsule().fill(on ? WT.accent : WT.fill))
                    }.buttonStyle(.plain)
                }
            }
            .padding(.bottom, 4)

            let list = filtered
            if list.isEmpty {
                Text("No memories match.").font(.system(size: 13, weight: .medium)).foregroundStyle(WT.ter)
                    .frame(maxWidth: .infinity).padding(.vertical, 34)
            } else {
                ForEach(Array(list.enumerated()), id: \.element.id) { i, m in
                    if i > 0 { Rectangle().fill(WT.sep).frame(height: 1) }
                    MemoryPageRow(mem: m, onOpenApp: onOpenApp)
                }
                Button("View all in app →", action: onOpenApp).buttonStyle(.plain)
                    .font(.system(size: 13, weight: .semibold)).foregroundStyle(WT.accent).padding(.top, 12)
            }
        }
    }
}

struct MemoryPageRow: View {
    let mem: LatestMemory
    let onOpenApp: () -> Void
    @State private var open = false

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button { withAnimation(.easeOut(duration: 0.22)) { open.toggle() } } label: {
                HStack(alignment: .top, spacing: 10) {
                    ZStack {
                        RoundedRectangle(cornerRadius: 7).fill(Color.primary.opacity(0.05)).frame(width: 26, height: 26)
                        Image(systemName: memIcon(mem.type)).font(.system(size: 14, weight: .medium)).foregroundStyle(memColor(mem.type))
                    }.padding(.top, 1)
                    VStack(alignment: .leading, spacing: 3) {
                        HStack(alignment: .firstTextBaseline, spacing: 8) {
                            Text(mem.title).font(.system(size: 13, weight: .semibold)).tracking(-0.2)
                                .foregroundStyle(WT.text).lineLimit(1).truncationMode(.tail)
                                .frame(maxWidth: .infinity, alignment: .leading)
                            Text(fmtAgo(mem.agoMinutes)).font(.system(size: 11, weight: .medium)).foregroundStyle(WT.ter)
                        }
                        if !open {
                            Text(mem.content).font(.system(size: 12)).foregroundStyle(WT.sub).lineLimit(2).lineSpacing(2)
                        }
                    }
                    Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold)).foregroundStyle(WT.ter)
                        .rotationEffect(.degrees(open ? 90 : 0)).padding(.top, 3)
                }
                .padding(.vertical, 11).contentShape(Rectangle())
            }.buttonStyle(.plain)

            if open {
                VStack(alignment: .leading, spacing: 0) {
                    ScrollView { Text(mem.content).font(.system(size: 12)).foregroundStyle(WT.sub).lineSpacing(3)
                        .frame(maxWidth: .infinity, alignment: .leading).padding(.trailing, 4) }
                        .frame(maxHeight: 120)
                    HStack(spacing: 7) {
                        memAction("doc.on.doc", "Copy") {
                            NSPasteboard.general.clearContents(); NSPasteboard.general.setString(mem.content, forType: .string)
                        }
                        memAction("arrow.up.forward.square", "Open in App", action: onOpenApp)
                    }
                    .padding(.top, 10).padding(.top, 10)
                    .overlay(Rectangle().fill(WT.sep).frame(height: 1), alignment: .top)
                }
                .padding(.leading, 36).padding(.bottom, 12)
            }
        }
    }

    private func memAction(_ icon: String, _ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Image(systemName: icon).font(.system(size: 12)); Text(label).font(.system(size: 12, weight: .semibold))
            }
            .foregroundStyle(WT.text).padding(.horizontal, 11).padding(.vertical, 5)
            .background(RoundedRectangle(cornerRadius: 8).fill(WT.btnFill))
        }.buttonStyle(.plain)
    }
}

// MARK: - shared tint + formatting helpers

struct ConfidenceDot: View {
    let confidence: String?

    var body: some View {
        if let confidence, let label = confidenceLabel(confidence) {
            Circle()
                .fill(confidenceColor(confidence))
                .frame(width: 6, height: 6)
                .help(label)
        }
    }
}

func confidenceColor(_ confidence: String) -> Color {
    switch confidence.lowercased() {
    case "high": return WT.memDecision
    case "medium": return WT.accent
    case "low": return WT.ter
    default: return WT.ter
    }
}

func confidenceLabel(_ confidence: String) -> String? {
    switch confidence.lowercased() {
    case "high": return "High confidence"
    case "medium": return "Medium confidence"
    case "low": return "Low confidence"
    default: return nil
    }
}

func memIcon(_ type: String) -> String {
    switch type {
    case "decision": return "lightbulb"
    case "feedback": return "bubble.left"
    case "session_summary": return "clock"
    default: return "doc.text"
    }
}
func memColor(_ type: String) -> Color {
    switch type {
    case "decision": return WT.memDecision
    case "feedback": return WT.memFeedback
    default: return WT.memNote
    }
}
func fmtAgo(_ minutes: Int) -> String {
    if minutes < 60 { return "\(minutes)m" }
    if minutes < 1440 { return "\(minutes / 60)h" }
    return "\(minutes / 1440)d"
}
