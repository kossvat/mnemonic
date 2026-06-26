import SwiftUI
import MnemonicShared

struct SetupView: View {
    let message: String
    @ObservedObject var model: MnemonicAppModel

    var body: some View {
        VStack(spacing: Theme.Space.xl) {
            Image(systemName: "bolt.horizontal.circle")
                .font(.system(size: 48, weight: .light))
                .foregroundStyle(Theme.Palette.textSubtle)

            VStack(spacing: Theme.Space.xs) {
                Text("Mnemonic daemon unavailable")
                    .font(Theme.Font.heading)
                    .tracking(Theme.Font.trackingHeading)
                    .foregroundStyle(Theme.Palette.textPrimary)
                Text(message)
                    .font(Theme.Font.body)
                    .foregroundStyle(Theme.Palette.textMuted)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 480)
            }

            HStack(spacing: Theme.Space.sm) {
                Button {
                    Task { await model.refreshAll() }
                } label: {
                    Label("Retry", systemImage: "arrow.clockwise")
                        .font(Theme.Font.bodyMedium)
                }
                .buttonStyle(SubtleButtonStyle())

                Button {
                    let config = FileManager.default.homeDirectoryForCurrentUser
                        .appendingPathComponent(".config/mnemonic/config.toml")
                    NSWorkspace.shared.open(config)
                } label: {
                    Label("Open config", systemImage: "gearshape")
                        .font(Theme.Font.bodyMedium)
                }
                .buttonStyle(SubtleButtonStyle())

                Button {
                    let log = FileManager.default.homeDirectoryForCurrentUser
                        .appendingPathComponent(".mnemonic/daemon.log")
                    NSWorkspace.shared.open(log)
                } label: {
                    Label("Open log", systemImage: "doc.text")
                        .font(Theme.Font.bodyMedium)
                }
                .buttonStyle(SubtleButtonStyle())
            }
        }
        .padding(Theme.Space.huge)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.Palette.bgPrimary)
    }
}
