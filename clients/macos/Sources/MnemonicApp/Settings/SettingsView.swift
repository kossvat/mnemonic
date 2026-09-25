import SwiftUI

struct SettingsView: View {
    @ObservedObject var model: MnemonicAppModel
    @State private var tokenPath = ""

    var body: some View {
        Form {
            Section("Connection") {
                TextField("Endpoint", text: $model.endpoint)
                HStack {
                    Text("Token file")
                    Spacer()
                    Text(tokenPath)
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.middle)
                }
            }

            Section("Refresh") {
                Stepper(
                    "Interval: \(Int(model.refreshInterval))s",
                    value: $model.refreshInterval,
                    in: 10...300,
                    step: 5
                )
            }

            Section("Appearance") {
                Picker("Theme", selection: $model.theme) {
                    Text("System").tag("system")
                    Text("Light").tag("light")
                    Text("Dark").tag("dark")
                }
                .pickerStyle(.segmented)
            }

            HStack {
                Spacer()
                Button("Retry connection") {
                    Task { await model.refreshAll() }
                }
                .buttonStyle(.borderedProminent)
            }
        }
        .padding(20)
        .task {
            tokenPath = await model.tokenPath
        }
    }
}
