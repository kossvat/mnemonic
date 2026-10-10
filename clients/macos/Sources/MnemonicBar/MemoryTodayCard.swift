import MnemonicShared
import SwiftUI

/// The card of memory today on the Work page, above the card of the
/// latest memory: what memory returned on this local day, from the recall
/// log. Its numbers come from the daemon's route, or from the command line
/// when the route does not answer (MnemonicService.recallToday); how the
/// card looks is a matter of taste.
struct MemoryTodayView: View {
    let state: RecallDayState

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 0) {
                Text("MEMORY TODAY")
                    .font(.system(size: 10, weight: .bold)).tracking(0.6).foregroundStyle(WT.ter)
                Spacer()
                if case .filled(let s) = state, s.hiddenAsNoise > 0 {
                    Text("\(s.hiddenAsNoise) hidden as noise")
                        .font(.system(size: 10.5, weight: .semibold)).monospacedDigit()
                        .foregroundStyle(WT.ter)
                }
            }
            .padding(.horizontal, 13).padding(.top, 9)

            switch state {
            case .filled(let s):
                numbers(s)
                if !s.top.isEmpty { top(s) }
            case .empty:
                line("No recorded recalls today")
            case .off:
                line("Recall log is off")
            case .unavailable:
                line("Recall data unavailable")
            }
        }
        .padding(.bottom, 11)
        .background(RoundedRectangle(cornerRadius: WT.R.inner).fill(WT.fill))
        .overlay(RoundedRectangle(cornerRadius: WT.R.inner).stroke(WT.sep, lineWidth: 1))
        .padding(.bottom, 13)
    }

    private func numbers(_ s: RecallDaySummary) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 0) {
            stat(s.contextsGenerated, "contexts")
            stat(s.callsAnswered, "calls")
            stat(s.memoriesReturned, "memories")
            stat(s.correctionsReturned, "corrections")
        }
        .padding(.horizontal, 13).padding(.top, 8)
    }

    private func stat(_ n: Int, _ label: String) -> some View {
        VStack(alignment: .leading, spacing: 1) {
            Text("\(n)")
                .font(.system(size: 17, weight: .bold)).tracking(-0.4).monospacedDigit()
                .foregroundStyle(WT.text)
            Text(label)
                .font(.system(size: 10.5, weight: .medium)).foregroundStyle(WT.ter)
        }
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    /// The first three of the memories returned most, each with the tint
    /// of its type and how many times it came back.
    private func top(_ s: RecallDaySummary) -> some View {
        VStack(alignment: .leading, spacing: 5) {
            ForEach(s.top.prefix(3), id: \.memoryId) { m in
                HStack(spacing: 7) {
                    Circle().fill(tint(m.memoryType)).frame(width: 6, height: 6)
                    Text(m.title)
                        .font(.system(size: 12, weight: .medium)).foregroundStyle(WT.sub)
                        .lineLimit(1).truncationMode(.tail)
                    Spacer(minLength: 6)
                    Text("×\(m.times)")
                        .font(.system(size: 11, weight: .semibold)).monospacedDigit()
                        .foregroundStyle(WT.ter)
                }
            }
        }
        .padding(.horizontal, 13).padding(.top, 9)
    }

    private func line(_ t: String) -> some View {
        Text(t)
            .font(.system(size: 12.5, weight: .medium)).foregroundStyle(WT.ter)
            .frame(maxWidth: .infinity, alignment: .leading)
            .padding(.horizontal, 13).padding(.top, 6)
    }

    private func tint(_ type: String) -> Color {
        switch type {
        case "decision": return WT.memDecision
        case "feedback": return WT.memFeedback
        case "session_summary": return WT.sub
        default: return WT.memNote
        }
    }
}
