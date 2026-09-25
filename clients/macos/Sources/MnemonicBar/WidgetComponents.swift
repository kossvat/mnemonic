import SwiftUI

enum ChartRange: String { case week, month }
enum ChartType: String { case bar, curve }

// MARK: - Work chart (bars + smoothed curve)

struct WorkChartView: View {
    let series: [WorkDay]
    let type: ChartType
    let range: ChartRange
    let selected: Int
    let height: CGFloat
    let onSelect: (Int) -> Void

    private let padX: CGFloat = 3
    private let padTop: CGFloat = 10
    private let padBottom: CGFloat = 22

    var body: some View {
        GeometryReader { geo in
            let w = geo.size.width
            let n = max(series.count, 1)
            let plotH = height - padTop - padBottom
            let base = padTop + plotH
            let maxMin = max(60.0, series.map { $0.minutes }.max() ?? 0)
            let xStep = (w - padX * 2) / CGFloat(n)
            let week = range == .week
            let cx: (Int) -> CGFloat = { i in padX + xStep * (CGFloat(i) + 0.5) }
            let yOf: (Double) -> CGFloat = { m in padTop + plotH * (1 - CGFloat(min(m, maxMin) / maxMin)) }

            ZStack(alignment: .topLeading) {
                // selection highlight column
                if selected >= 0 && selected < n {
                    RoundedRectangle(cornerRadius: 6).fill(WT.fill)
                        .frame(width: xStep, height: plotH + 8)
                        .position(x: cx(selected), y: padTop - 6 + (plotH + 8) / 2)
                }

                // marks
                Canvas { ctx, _ in
                    if type == .bar {
                        let bw = xStep * (week ? 0.52 : 0.6)
                        let r = min(bw / 2, week ? 4 : 2.5)
                        for (i, day) in series.enumerated() {
                            let zero = day.minutes <= 0
                            let h = max(zero ? 2.5 : 3, plotH * CGFloat(min(day.minutes, maxMin) / maxMin))
                            let rect = CGRect(x: cx(i) - bw / 2, y: base - h, width: bw, height: h)
                            let path = Path(roundedRect: rect, cornerRadius: r)
                            // Today is THE bar that matters — amber with a
                            // soft glow, whether or not it's selected.
                            if day.isToday && !zero {
                                var glow = ctx
                                glow.addFilter(.shadow(color: WT.accentGlow, radius: 5, y: 1))
                                glow.fill(path, with: .color(WT.accent))
                            } else {
                                let color: Color = (i == selected || day.isToday) ? WT.accent : WT.barIdle
                                ctx.fill(path, with: .color(color.opacity(zero ? 0.5 : 1)))
                            }
                        }
                    } else {
                        let raw = series.map { $0.minutes }
                        let disp = range == .month ? smoothVals(raw) : raw
                        let pts = disp.enumerated().map { CGPoint(x: cx($0.offset), y: yOf($0.element)) }
                        let line = catmullRom(pts)
                        var area = line
                        if let last = pts.last, let first = pts.first {
                            area.addLine(to: CGPoint(x: last.x, y: base))
                            area.addLine(to: CGPoint(x: first.x, y: base))
                            area.closeSubpath()
                        }
                        ctx.fill(area, with: .linearGradient(
                            Gradient(colors: [WT.accent.opacity(0.22), WT.accent.opacity(0)]),
                            startPoint: CGPoint(x: 0, y: padTop),
                            endPoint: CGPoint(x: 0, y: base)))
                        ctx.stroke(line, with: .color(WT.accent), style: StrokeStyle(lineWidth: 2.2, lineCap: .round, lineJoin: .round))
                        for (i, p) in pts.enumerated() {
                            let on = i == selected
                            if !on && !series[i].isToday && !week { continue }
                            let rr = on ? 4.2 : 2.6
                            let dot = Path(ellipseIn: CGRect(x: p.x - rr, y: p.y - rr, width: rr * 2, height: rr * 2))
                            if on { ctx.fill(dot, with: .color(WT.accent)) }
                            else {
                                ctx.fill(dot, with: .color(WT.bg))
                                ctx.stroke(dot, with: .color(WT.accent), lineWidth: 1.8)
                            }
                        }
                    }
                }

                // labels
                ForEach(Array(series.enumerated()), id: \.offset) { i, day in
                    if let lbl = label(i: i, day: day, n: n, week: week) {
                        Text(lbl.text)
                            .font(.system(size: week ? 11 : 9.5,
                                          weight: (day.isToday || i == selected) ? .bold : .medium))
                            .foregroundStyle(i == selected ? WT.accent : (day.isToday ? WT.sub : WT.ter))
                            .fixedSize()
                            .position(x: lbl.x(cx(i), w), y: height - 6)
                    }
                }

                // hit targets — series.count, not n: n is clamped to ≥1 for
                // layout math, so an empty chart would still get one
                // tappable column whose index crashes series[i] upstream.
                ForEach(0..<series.count, id: \.self) { i in
                    Color.clear.contentShape(Rectangle())
                        .frame(width: xStep, height: height)
                        .position(x: padX + xStep * (CGFloat(i) + 0.5), y: height / 2)
                        .onTapGesture { onSelect(i) }
                }
            }
        }
        .frame(height: height)
    }

    private struct Lbl { let text: String; let anchorEnd: Bool
        func x(_ cx: CGFloat, _ w: CGFloat) -> CGFloat { anchorEnd ? w - 3 - 14 : cx } }

    private func label(i: Int, day: WorkDay, n: Int, week: Bool) -> Lbl? {
        if week { return Lbl(text: day.dowLetter, anchorEnd: false) }
        if i == n - 1 { return Lbl(text: "Today", anchorEnd: true) }
        if day.dow == 0 { return Lbl(text: "\(day.dayNum)", anchorEnd: false) }
        return nil
    }
}

// Weighted moving average — calms month curves full of zero/short days.
func smoothVals(_ a: [Double], _ w: [Double] = [1, 2, 3, 2, 1]) -> [Double] {
    let half = (w.count - 1) / 2
    return a.indices.map { i in
        var s = 0.0, wt = 0.0
        for k in -half...half {
            let j = i + k
            if j < 0 || j >= a.count { continue }
            let ww = w[k + half]; s += a[j] * ww; wt += ww
        }
        return wt != 0 ? s / wt : 0
    }
}

// Catmull-Rom → smooth Bézier path.
func catmullRom(_ pts: [CGPoint]) -> Path {
    var path = Path()
    guard pts.count >= 2 else {
        if let p = pts.first { path.move(to: p) }
        return path
    }
    path.move(to: pts[0])
    for i in 0..<(pts.count - 1) {
        let p0 = i > 0 ? pts[i - 1] : pts[i]
        let p1 = pts[i]
        let p2 = pts[i + 1]
        let p3 = i + 2 < pts.count ? pts[i + 2] : p2
        let c1 = CGPoint(x: p1.x + (p2.x - p0.x) / 6, y: p1.y + (p2.y - p0.y) / 6)
        let c2 = CGPoint(x: p2.x - (p3.x - p1.x) / 6, y: p2.y - (p3.y - p1.y) / 6)
        path.addCurve(to: p2, control1: c1, control2: c2)
    }
    return path
}

// MARK: - Day detail + session timeline

struct DayDetailView: View {
    let day: WorkDay?
    let working: Bool
    var body: some View {
        if let day {
            let zero = day.sessions.isEmpty && day.seconds <= 0
            VStack(alignment: .leading, spacing: 0) {
                HStack(alignment: .firstTextBaseline) {
                    Text(day.isToday ? "Today" : day.label)
                        .font(.system(size: 13, weight: .bold)).tracking(-0.2).foregroundStyle(WT.text)
                    Spacer()
                    Text(zero ? "No work tracked" : fmtDur(day.seconds))
                        .font(.system(size: 13, weight: .bold)).monospacedDigit()
                        .foregroundStyle(zero ? WT.ter : WT.text)
                }
                .padding(.bottom, zero ? 0 : 7)

                if !zero {
                    HStack(spacing: 14) {
                        stat("\(day.sessionCount)", "sessions")
                        stat(fmtDur(day.longestSeconds), "longest")
                        if let span = day.spanHuman { stat(span, "span") }
                    }
                    // One line, slight shrink instead of the ugly mid-value
                    // wrap ("12:30am–\n1:24am") night spans used to get.
                    .lineLimit(1)
                    .minimumScaleFactor(0.8)
                }
                SessionTimelineView(sessions: day.sessions, working: working && day.isToday)
            }
        }
    }
    private func stat(_ v: String, _ l: String) -> some View {
        (Text(v).font(.system(size: 11.5, weight: .semibold)).foregroundStyle(WT.sub)
         + Text(" \(l)").font(.system(size: 11.5, weight: .medium)).foregroundStyle(WT.ter))
    }
}

/// Thin lane showing the day's work blocks. The window defaults to 6a–10p
/// and STRETCHES to cover real blocks — night-owl sessions (12:30am–1:24am)
/// used to be clipped away entirely, rendering an empty lane on a day with
/// tracked work. Hour ticks adapt to the stretched window.
struct SessionTimelineView: View {
    let sessions: [BlockMin]
    let working: Bool

    private var window: (w0: Double, w1: Double) {
        var w0 = 6.0 * 60, w1 = 22.0 * 60
        if let first = sessions.map(\.startMin).min() {
            w0 = min(w0, (first / 60).rounded(.down) * 60)
        }
        if let last = sessions.map(\.endMin).max() {
            w1 = max(w1, (last / 60).rounded(.up) * 60)
        }
        return (w0, w1)
    }
    private var span: Double { window.w1 - window.w0 }

    private var ticks: [(Double, String)] {
        let (w0, w1) = window
        // 3h grid for a normal day, 6h once the window stretches wide —
        // keeps 3–5 labels either way.
        let stepH: Double = (w1 - w0) / 60 > 17 ? 6 : 3
        var out: [(Double, String)] = []
        var h = (w0 / 60 / stepH).rounded(.up) * stepH
        while h * 60 <= w1 {
            let m = h * 60
            // Skip ticks hugging the lane edges — their labels would clip.
            if m > w0 + 29, m < w1 - 29 {
                out.append((m, hourLabel(Int(h))))
            }
            h += stepH
        }
        return out
    }

    private func hourLabel(_ h: Int) -> String {
        let hh = ((h % 24) + 24) % 24
        if hh == 0 { return "12a" }
        if hh < 12 { return "\(hh)a" }
        if hh == 12 { return "12p" }
        return "\(hh - 12)p"
    }

    var body: some View {
        let (w0, w1) = window
        VStack(spacing: 4) {
            GeometryReader { geo in
                let W = geo.size.width
                // Clip blocks to the (already stretched) window defensively.
                let visible: [(left: Double, width: Double, last: Bool)] = sessions.enumerated().compactMap { i, s in
                    let cs = max(s.startMin, w0)
                    let ce = min(s.endMin, w1)
                    guard ce > cs else { return nil }
                    return (Double(clamp((cs - w0) / span)),
                            Double(max(0.012, clamp((ce - cs) / span))),
                            i == sessions.count - 1)
                }
                ZStack(alignment: .leading) {
                    RoundedRectangle(cornerRadius: 5).fill(WT.barIdle)
                        .opacity(sessions.isEmpty ? 0.5 : 1)
                        .frame(height: 9)
                    ForEach(Array(visible.enumerated()), id: \.offset) { _, b in
                        RoundedRectangle(cornerRadius: 5).fill(WT.accent)
                            .frame(width: min(W - b.left * W, max(2, b.width * W)), height: 9)
                            .opacity(working && b.last ? 0.9 : 1)
                            .offset(x: b.left * W)
                    }
                }
                .frame(width: W, height: 9, alignment: .leading)
                .clipped()
            }
            .frame(height: 9)

            GeometryReader { geo in
                let W = geo.size.width
                ZStack(alignment: .leading) {
                    ForEach(ticks, id: \.1) { m, lbl in
                        Text(lbl)
                            .font(.system(size: 10.5, weight: .bold)).monospacedDigit()
                            .foregroundStyle(WT.sub)
                            .fixedSize()
                            .position(x: clamp((m - w0) / span) * W, y: 7)
                    }
                }
            }
            .frame(height: 14)
        }
        .padding(.top, 11)
    }
    private func clamp(_ v: Double) -> CGFloat { CGFloat(max(0, min(1, v))) }
}

// MARK: - Latest memory

struct LatestMemoryView: View {
    let mem: LatestMemory?
    let total: Int
    @Binding var open: Bool
    let onOpenApp: () -> Void

    var body: some View {
        Group {
            if let mem {
                VStack(alignment: .leading, spacing: 0) {
                    // header
                    HStack(spacing: 0) {
                        Text("LATEST MEMORY")
                            .font(.system(size: 10, weight: .bold)).tracking(0.6).foregroundStyle(WT.ter)
                        Spacer()
                        Text("\(total) total")
                            .font(.system(size: 10.5, weight: .semibold)).monospacedDigit()
                            .foregroundStyle(WT.ter).padding(.trailing, 10)
                        Button(action: onOpenApp) {
                            Text("View all →").font(.system(size: 11.5, weight: .semibold)).foregroundStyle(WT.accent)
                        }.buttonStyle(.plain)
                    }
                    .padding(.horizontal, 13).padding(.top, 9)

                    // collapsed/expanded body
                    Button { withAnimation(.easeOut(duration: 0.22)) { open.toggle() } } label: {
                        HStack(alignment: .top, spacing: 10) {
                            ZStack {
                                RoundedRectangle(cornerRadius: 7)
                                    .fill(Color.primary.opacity(0.05)).frame(width: 26, height: 26)
                                Image(systemName: meta.icon).font(.system(size: 14, weight: .medium))
                                    .foregroundStyle(meta.color)
                            }.padding(.top, 1)
                            VStack(alignment: .leading, spacing: 3) {
                                HStack(alignment: .firstTextBaseline, spacing: 8) {
                                    Text(mem.title).font(.system(size: 13, weight: .semibold)).tracking(-0.2)
                                        .foregroundStyle(WT.text).lineLimit(1).truncationMode(.tail)
                                    Spacer(minLength: 0)
                                    Text("\(mem.agoMinutes)m ago")
                                        .font(.system(size: 11, weight: .medium)).foregroundStyle(WT.ter)
                                }
                                if !open {
                                    Text(mem.content).font(.system(size: 12)).foregroundStyle(WT.sub)
                                        .lineLimit(2).lineSpacing(2)
                                }
                            }
                            Image(systemName: "chevron.right").font(.system(size: 11, weight: .semibold))
                                .foregroundStyle(WT.ter).rotationEffect(.degrees(open ? 90 : 0)).padding(.top, 3)
                        }
                        .padding(.horizontal, 13).padding(.top, 8).padding(.bottom, 11)
                        .contentShape(Rectangle())
                    }.buttonStyle(.plain)

                    if open {
                        VStack(alignment: .leading, spacing: 0) {
                            ScrollView {
                                Text(mem.content).font(.system(size: 12)).foregroundStyle(WT.sub)
                                    .lineSpacing(3).frame(maxWidth: .infinity, alignment: .leading)
                                    .padding(.trailing, 4)
                            }
                            .frame(maxHeight: 112)
                            HStack(spacing: 7) {
                                memAction("doc.on.doc", "Copy") {
                                    NSPasteboard.general.clearContents()
                                    NSPasteboard.general.setString(mem.content, forType: .string)
                                }
                                memAction("arrow.up.forward.square", "Open in App", action: onOpenApp)
                            }
                            .padding(.top, 11).padding(.top, 11)
                            .overlay(Rectangle().fill(WT.sep).frame(height: 1), alignment: .top)
                        }
                        .padding(.horizontal, 13).padding(.bottom, 12)
                    }
                }
                .background(RoundedRectangle(cornerRadius: WT.R.inner).fill(WT.fill))
                .overlay(RoundedRectangle(cornerRadius: WT.R.inner).stroke(WT.sep, lineWidth: 1))
                // Memory-type color edge — the card reads as a decision /
                // feedback / note at a glance, before any text.
                .overlay(alignment: .leading) {
                    UnevenRoundedRectangle(topLeadingRadius: WT.R.inner,
                                           bottomLeadingRadius: WT.R.inner)
                        .fill(meta.color.opacity(0.8))
                        .frame(width: 3)
                }
            } else {
                Text("No memories captured yet — they appear here as you work.")
                    .font(.system(size: 12.5, weight: .medium)).foregroundStyle(WT.ter)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .padding(.horizontal, 14).padding(.vertical, 13)
                    .background(RoundedRectangle(cornerRadius: WT.R.inner).fill(WT.fill))
                    .overlay(RoundedRectangle(cornerRadius: WT.R.inner).stroke(WT.sep, lineWidth: 1))
            }
        }
    }

    private var meta: (icon: String, color: Color) {
        switch mem?.type {
        case "decision": return ("lightbulb", WT.memDecision)
        case "feedback": return ("bubble.left", WT.memFeedback)
        case "session_summary": return ("clock", WT.sub)
        default: return ("doc.text", WT.memNote)
        }
    }

    private func memAction(_ icon: String, _ label: String, action: @escaping () -> Void) -> some View {
        Button(action: action) {
            HStack(spacing: 5) {
                Image(systemName: icon).font(.system(size: 12))
                Text(label).font(.system(size: 12, weight: .semibold))
            }
            .foregroundStyle(WT.text)
            .padding(.horizontal, 11).padding(.vertical, 5)
            .background(RoundedRectangle(cornerRadius: 8).fill(WT.btnFill))
        }.buttonStyle(.plain)
    }
}
