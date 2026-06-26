import SwiftUI

/// Mnemonic menu-bar popover — v2 "Hybrid" design ported from Claude Design.
/// Neutral macOS surface, warm amber accent on the chart + primary button only.
/// Four jobs: worked-time hero, work-history (week/month · bars/curve · day
/// timeline), primary actions, and the Latest Memory card.
struct MenuBarView: View {
    @ObservedObject var service: MnemonicService

    /// When embedded as the Work page of the paged deck: the brand/search
    /// header moves up to the pager, the card is always expanded, and the
    /// share icon deep-links to the Share page instead of opening a sheet.
    var inDeck: Bool = false
    var onShare: (() -> Void)? = nil

    @State private var range: ChartRange = .week
    @State private var chartType: ChartType = .bar
    @State private var expanded: Bool = UserDefaults.standard.string(forKey: "mn-size") == "expanded"
    @State private var selected: Int = -1            // index into current series; -1 = last
    @State private var tappedDetail: WorkDay? = nil  // detail for a tapped non-today day
    @State private var memoryOpen = false
    @State private var showAddMemory = false
    @State private var showShare = false

    private let contentWidth: CGFloat = 336
    private var d: WidgetData { service.data }

    // Current chart series (week = 7, month = 30).
    private var series: [WorkDay] {
        range == .week ? d.days : service.monthDays
    }
    private var selIndex: Int {
        let n = series.count
        guard n > 0 else { return 0 }
        return selected < 0 || selected >= n ? n - 1 : selected
    }
    private var selectedDay: WorkDay? {
        guard !series.isEmpty else { return nil }
        let day = series[selIndex]
        if day.isToday { return d.today ?? day }
        if let t = tappedDetail, t.date == day.date { return t }
        return day
    }

    private var effExpanded: Bool { inDeck ? true : expanded }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            if !inDeck { header }
            hero
            historyCard
            actions
            if showAddMemory { addMemoryForm }
            LatestMemoryView(mem: d.latest, total: d.memoriesTotal,
                             open: $memoryOpen, onOpenApp: { service.openDashboard() })
            if d.state == .broken { brokenBanner }
        }
        .padding(.horizontal, 15)
        .padding(.vertical, 14)
        .frame(width: inDeck ? nil : contentWidth, alignment: .leading)
        .frame(maxWidth: inDeck ? .infinity : nil, alignment: .leading)
        .background(WT.bg)
        .sheet(isPresented: $showShare) {
            ShareComposerView(data: d, service: service)
        }
        .onChange(of: range) { _, r in
            if r == .month { service.loadMonthIfNeeded() }
            chartType = r == .month ? .curve : .bar
            selected = -1
            tappedDetail = nil
        }
    }

    // MARK: Header

    private var header: some View {
        HStack(spacing: 8) {
            Image(systemName: "brain.head.profile")
                .font(.system(size: 15, weight: .regular))
                .foregroundStyle(WT.sub)
            Text("Mnemonic")
                .font(.system(size: 13.5, weight: .bold))
                .tracking(-0.2)
                .foregroundStyle(WT.text)
            Spacer(minLength: 0)
            iconBtn(expanded ? "chevron.up" : "chevron.down", help: expanded ? "Collapse" : "Expand") {
                expanded.toggle()
                UserDefaults.standard.set(expanded ? "expanded" : "compact", forKey: "mn-size")
            }
            iconBtn("magnifyingglass", help: "Search memories") { service.openDashboard() }
            HeaderStatusView(state: d.state)
        }
        .padding(.bottom, 13)
    }

    // MARK: Hero

    private var hero: some View {
        let parts = hm(d.state == .empty ? 0 : d.workedTodaySeconds)
        return VStack(alignment: .leading, spacing: 0) {
            Text("WORKED TODAY")
                .font(.system(size: 11, weight: .bold))
                .tracking(0.6)
                .foregroundStyle(WT.ter)
                .padding(.bottom, 4)

            HStack(alignment: .firstTextBaseline, spacing: 2) {
                if parts.h > 0 {
                    Text("\(parts.h)").heroNum()
                    Text("h").heroUnit().padding(.trailing, 6)
                }
                Text("\(parts.m)").heroNum()
                Text("m").heroUnit()
            }
            .padding(.bottom, 7)

            heroSubline.padding(.bottom, d.state == .empty ? 0 : 7)

            if d.state != .empty, let wk = d.week, wk.totalSeconds > 0 {
                WeekChips(week: wk)
            }
        }
        .padding(.bottom, 14)
    }

    @ViewBuilder private var heroSubline: some View {
        switch d.state {
        case .working:
            HStack(spacing: 6) {
                Circle().fill(WT.working).frame(width: 6, height: 6)
                Text("In session · \(fmtDur(d.sessionSeconds ?? 0))")
                    .font(.system(size: 12.5, weight: .semibold)).foregroundStyle(WT.sub)
            }
        case .idle:
            sub("Stepped away")
        case .stopped:
            sub("Tracking stopped")
        case .broken:
            sub("Tracking unavailable")
        case .empty:
            sub("No activity tracked yet today")
        }
    }

    private func sub(_ t: String) -> some View {
        Text(t).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(WT.sub)
    }

    // MARK: Work-history card

    private var historyCard: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 8) {
                SegControl(items: [("week", "Week", nil), ("month", "Month", nil)],
                           value: range.rawValue) { range = ChartRange(rawValue: $0) ?? .week }
                iconBtn("square.and.arrow.up", size: 28, help: "Share work summary") {
                    if let onShare { onShare() } else { showShare = true }
                }
                Spacer(minLength: 0)
                SegControl(items: [("bar", nil, "chart.bar.fill"), ("curve", nil, "waveform.path")],
                           value: chartType.rawValue) { chartType = ChartType(rawValue: $0) ?? .bar }
            }
            .padding(.bottom, 11)

            WorkChartView(series: series, type: chartType, range: range,
                          selected: selIndex, height: effExpanded ? 116 : 92) { i in
                // series is computed (week/month switch, async month load)
                // — the index baked into a hit target can outlive the data
                // it pointed at. Stale taps must not crash the widget.
                guard series.indices.contains(i) else { return }
                selected = i
                let day = series[i]
                if !day.isToday && !day.detailLoaded {
                    service.loadDayDetail(date: day.date) { detail in
                        if let detail { tappedDetail = detail }
                    }
                } else {
                    tappedDetail = nil
                }
            }

            if effExpanded {
                Rectangle().fill(WT.sep).frame(height: 1).padding(.vertical, 11)
                DayDetailView(day: selectedDay, working: d.state == .working)
            }
        }
        .padding(EdgeInsets(top: 12, leading: 13, bottom: 13, trailing: 13))
        .background(RoundedRectangle(cornerRadius: WT.R.card).fill(WT.fill))
        .overlay(RoundedRectangle(cornerRadius: WT.R.card).stroke(WT.sep, lineWidth: 1))
        .padding(.bottom, 13)
    }

    // MARK: Actions

    private var actions: some View {
        HStack(spacing: 8) {
            PrimaryButton(icon: "rectangle.portrait.and.arrow.right", label: "Open App") {
                service.openDashboard()
            }
            SecondaryButton(icon: justSaved ? "checkmark" : "plus",
                            label: justSaved ? "Saved" : "Add Memory") {
                withAnimation(.easeOut(duration: 0.15)) { showAddMemory.toggle() }
            }
            OverflowMenuView(state: d.state, service: service)
        }
        .padding(.bottom, 13)
    }

    @State private var addTitle = ""
    @State private var addType = "note"
    @State private var justSaved = false
    @FocusState private var addFieldFocused: Bool

    private func saveNewMemory() {
        let t = addTitle.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !t.isEmpty else { return }
        service.addMemory(title: t, type: addType)
        addTitle = ""
        withAnimation(.easeOut(duration: 0.15)) {
            showAddMemory = false
            justSaved = true
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) { service.refresh() }
        // Flash "Saved ✓" on the button, then settle back.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.8) {
            withAnimation(.easeOut(duration: 0.3)) { justSaved = false }
        }
    }

    private var addMemoryForm: some View {
        VStack(alignment: .leading, spacing: 8) {
            TextField("What to remember…", text: $addTitle)
                .textFieldStyle(.plain)
                .font(.system(size: 13))
                .foregroundStyle(WT.text)
                .focused($addFieldFocused)
                .onSubmit { saveNewMemory() }
                .padding(.horizontal, 10).padding(.vertical, 7)
                .background(RoundedRectangle(cornerRadius: 8).fill(WT.bg))
                .overlay(RoundedRectangle(cornerRadius: 8).stroke(WT.sep, lineWidth: 1))
                .onAppear { addFieldFocused = true }
            HStack(spacing: 6) {
                ForEach([("note", "doc.text", WT.memNote),
                         ("decision", "lightbulb", WT.memDecision),
                         ("feedback", "bubble.left", WT.memFeedback)], id: \.0) { t, icon, c in
                    Button { addType = t } label: {
                        Image(systemName: icon).font(.system(size: 12, weight: .medium))
                            .foregroundStyle(addType == t ? c : WT.ter)
                            .padding(6)
                            .background(RoundedRectangle(cornerRadius: 7)
                                .fill(addType == t ? c.opacity(0.14) : .clear))
                    }.buttonStyle(.plain)
                }
                Spacer()
                Button("Save") { saveNewMemory() }
                .buttonStyle(.plain)
                .font(.system(size: 13, weight: .semibold))
                .foregroundStyle(.white)
                .padding(.horizontal, 14).padding(.vertical, 7)
                .background(RoundedRectangle(cornerRadius: 8).fill(WT.accent))
                .opacity(addTitle.trimmingCharacters(in: .whitespaces).isEmpty ? 0.5 : 1)
            }
        }
        .padding(12)
        .background(RoundedRectangle(cornerRadius: WT.R.inner).fill(WT.fill))
        .overlay(RoundedRectangle(cornerRadius: WT.R.inner).stroke(WT.sep, lineWidth: 1))
        .padding(.bottom, 13)
    }

    private var brokenBanner: some View {
        HStack(spacing: 7) {
            Image(systemName: "exclamationmark.triangle.fill")
                .font(.system(size: 12)).foregroundStyle(WT.stopped)
            Text("Daemon not responding")
                .font(.system(size: 12, weight: .semibold)).foregroundStyle(WT.stopped)
            Spacer()
            Button("Restart") { service.startDaemon() }
                .buttonStyle(.plain)
                .font(.system(size: 12, weight: .bold)).foregroundStyle(WT.stopped)
        }
        .padding(.horizontal, 11).padding(.vertical, 9)
        .background(RoundedRectangle(cornerRadius: WT.R.btn).fill(WT.stopped.opacity(0.12)))
        .padding(.top, 11)
    }

    // MARK: helpers

    private func iconBtn(_ system: String, size: CGFloat = 26, help: String, _ action: @escaping () -> Void) -> some View {
        Button(action: action) {
            Image(systemName: system)
                .font(.system(size: 14, weight: .medium))
                .foregroundStyle(WT.sub)
                .frame(width: size, height: size == 28 ? 28 : 26)
                .background(RoundedRectangle(cornerRadius: 7).fill(size == 28 ? WT.btnFill : .clear))
        }
        .buttonStyle(.plain)
        .help(help)
    }
}

// MARK: - Hero number styling

private extension Text {
    func heroNum() -> some View {
        self.font(.system(size: 50, weight: .bold)).tracking(-2.0)
            .monospacedDigit().foregroundStyle(WT.text)
    }
    func heroUnit() -> some View {
        self.font(.system(size: 22, weight: .semibold)).foregroundStyle(WT.sub)
    }
}

// MARK: - Header status

struct HeaderStatusView: View {
    let state: WorkState
    var body: some View {
        let (c, t, pulse) = map
        return HStack(spacing: 6) {
            ZStack {
                Circle().fill(c).frame(width: 7, height: 7)
                if pulse {
                    Circle().stroke(c.opacity(0.5), lineWidth: 1.5).frame(width: 12, height: 12)
                }
            }
            Text(t).font(.system(size: 12.5, weight: .semibold)).foregroundStyle(WT.sub)
        }
    }
    private var map: (Color, String, Bool) {
        switch state {
        case .working: return (WT.working, "Running", true)
        case .empty: return (WT.working, "Running", true)
        case .idle: return (WT.idle, "Idle", false)
        case .stopped: return (WT.ter, "Stopped", false)
        case .broken: return (WT.stopped, "Not responding", false)
        }
    }
}

// MARK: - Week stat chips

struct WeekChips: View {
    let week: WeekStat
    var body: some View {
        HStack(spacing: 6) {
            chip {
                Text(fmtDur(week.totalSeconds)).fontWeight(.bold).foregroundStyle(WT.text)
                    .monospacedDigit()
                + Text(" this week").foregroundStyle(WT.sub)
            }
            if let delta = week.deltaSeconds {
                chip {
                    let up = delta >= 0
                    Text((up ? "+" : "−") + fmtDur(abs(delta)))
                        .fontWeight(.bold).foregroundStyle(up ? WT.working : WT.sub)
                    + Text(" vs avg").foregroundStyle(WT.sub)
                    + Text(week.bestWeekday.map { "  ·  Best \($0)" } ?? "").foregroundStyle(WT.sub)
                }
            } else if let best = week.bestWeekday {
                chip { Text("Best day \(best)").foregroundStyle(WT.sub) }
            }
        }
        .font(.system(size: 11, weight: .medium))
        .fixedSize(horizontal: false, vertical: true)
    }
    private func chip<C: View>(@ViewBuilder _ content: () -> C) -> some View {
        content()
            .padding(.horizontal, 9).padding(.vertical, 3)
            .background(RoundedRectangle(cornerRadius: WT.R.chip).fill(WT.fill))
    }
}

// MARK: - Segmented control

struct SegControl: View {
    let items: [(String, String?, String?)]  // (value, label?, sfSymbol?)
    let value: String
    let onChange: (String) -> Void
    var body: some View {
        HStack(spacing: 2) {
            ForEach(items, id: \.0) { v, label, sym in
                let on = v == value
                Button { onChange(v) } label: {
                    HStack(spacing: 5) {
                        if let sym { Image(systemName: sym).font(.system(size: 11, weight: .semibold)) }
                        if let label { Text(label).font(.system(size: 12, weight: .semibold)) }
                    }
                    .foregroundStyle(on ? WT.thumbText : WT.sub)
                    .padding(.horizontal, label != nil ? 10 : 9).padding(.vertical, 4)
                    .background(RoundedRectangle(cornerRadius: 6.5).fill(on ? WT.thumb : .clear)
                        .shadow(color: on ? .black.opacity(0.18) : .clear, radius: 1, y: 1))
                }
                .buttonStyle(.plain)
            }
        }
        .padding(2)
        .background(RoundedRectangle(cornerRadius: 8).fill(WT.track))
    }
}

// MARK: - Buttons

struct PrimaryButton: View {
    let icon: String; let label: String; let action: () -> Void
    @State private var hover = false
    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                Image(systemName: icon).font(.system(size: 14, weight: .semibold))
                Text(label).font(.system(size: 14, weight: .semibold)).tracking(-0.2)
                    .lineLimit(1).minimumScaleFactor(0.85)
            }
            .foregroundStyle(.white)
            .frame(maxWidth: .infinity).frame(height: 38)
            .background(
                RoundedRectangle(cornerRadius: WT.R.btn)
                    .fill(LinearGradient(colors: [WT.accent, WT.accentDeep],
                                         startPoint: .top, endPoint: .bottom))
                    .shadow(color: WT.accentGlow, radius: hover ? 9 : 6, y: 2)
            )
            .brightness(hover ? 0.05 : 0)
        }
        .buttonStyle(.plain).onHover { hover = $0 }
    }
}

struct SecondaryButton: View {
    let icon: String; let label: String; let action: () -> Void
    @State private var hover = false
    var body: some View {
        Button(action: action) {
            HStack(spacing: 7) {
                Image(systemName: icon).font(.system(size: 14, weight: .semibold))
                Text(label).font(.system(size: 14, weight: .semibold)).tracking(-0.2)
                    .lineLimit(1).fixedSize()
            }
            .foregroundStyle(WT.text)
            .padding(.horizontal, 14).frame(height: 38)
            .background(RoundedRectangle(cornerRadius: WT.R.btn).fill(hover ? WT.fill : WT.btnFill))
        }
        .buttonStyle(.plain).onHover { hover = $0 }
    }
}

struct OverflowMenuView: View {
    let state: WorkState
    let service: MnemonicService
    var body: some View {
        Menu {
            Button { service.openLog() } label: { Label("View log", systemImage: "clock") }
            Button { service.generateContext() } label: { Label("Context for agents", systemImage: "sparkles") }
            Divider()
            if state == .stopped {
                Button { service.startDaemon() } label: { Label("Start daemon", systemImage: "power") }
            } else {
                Button { service.stopDaemon() } label: { Label("Stop daemon", systemImage: "pause.fill") }
            }
        } label: {
            Image(systemName: "ellipsis").font(.system(size: 16, weight: .semibold)).foregroundStyle(WT.sub)
                .frame(width: 38, height: 38)
                .background(RoundedRectangle(cornerRadius: WT.R.btn).fill(WT.btnFill))
        }
        .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize()
    }
}

// MARK: - Duration formatting (mirrors Rust fmt_hm)

func fmtDur(_ seconds: Double) -> String {
    let m = Int((seconds / 60).rounded())
    let h = m / 60, mm = m % 60
    if h > 0 { return mm > 0 ? "\(h)h \(mm)m" : "\(h)h" }
    return "\(mm)m"
}
func hm(_ seconds: Double) -> (h: Int, m: Int) {
    let m = max(0, Int(seconds.rounded()) / 60)
    return (m / 60, m % 60)
}
