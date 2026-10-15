import SwiftUI

struct TimelineAgentMark: View {
    let mark: TimelineDrawing.AgentMark
    let geometry: TimelineDrawing.Geometry

    var body: some View {
        Group {
            if geometry.point {
                if mark == .claude { Circle().fill(AppPalette.agents) }
                else if mark == .codex {
                    TimelineDiamond().fill(AppPalette.timelineFocus)
                        .overlay(TimelineDiamond().stroke(AppPalette.agents, lineWidth: 0.75))
                }
                else { Rectangle().stroke(AppPalette.textSecondary, lineWidth: 1) }
            } else {
                let radius: CGFloat = mark == .claude ? 10 : mark == .codex ? 4 : 2
                RoundedRectangle(cornerRadius: radius)
                    .fill(mark == .claude ? AppPalette.timelineClaude : mark == .codex ? AppPalette.timelineCodex : AppPalette.badge)
                    .overlay {
                        RoundedRectangle(cornerRadius: radius)
                            .strokeBorder(mark == .unknown ? AppPalette.textSecondary : AppPalette.agents,
                                          style: StrokeStyle(lineWidth: 1, dash: mark == .codex ? [3, 2] : []))
                    }
                    .overlay { TimelineContinuation(geometry: geometry, color: AppPalette.agents) }
            }
        }
        .clipped()
    }
}

struct TimelineContinuation: View {
    let geometry: TimelineDrawing.Geometry
    let color: Color

    var body: some View {
        HStack(spacing: 0) {
            if geometry.continuesBefore { Image(systemName: "chevron.left") }
            Spacer(minLength: 0)
            if geometry.continuesAfter { Image(systemName: "chevron.right") }
        }
        .font(.system(size: 8, weight: .bold)).foregroundStyle(color).clipped().accessibilityHidden(true)
    }
}

private struct TimelineDiamond: Shape {
    func path(in rect: CGRect) -> Path {
        Path { path in
            path.move(to: CGPoint(x: rect.midX, y: rect.minY))
            path.addLine(to: CGPoint(x: rect.maxX, y: rect.midY))
            path.addLine(to: CGPoint(x: rect.midX, y: rect.maxY))
            path.addLine(to: CGPoint(x: rect.minX, y: rect.midY))
            path.closeSubpath()
        }
    }
}
