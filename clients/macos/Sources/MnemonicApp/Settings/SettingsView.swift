import SwiftUI

struct SettingsView: View {
    @ObservedObject var model: MnemonicAppModel
    @State private var tokenPath = ""
    @State private var endpoint = ""

    var body: some View {
        Form {
            Section("Connection") {
                TextField("Endpoint", text: $endpoint)
                Button("Apply endpoint") { Task { await model.applyEndpoint(endpoint) } }
                if let message = model.setupMessage {
                    Text(verbatim: message).font(.caption).foregroundStyle(.secondary)
                }
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
                Text("Visible project and overview screens refresh every 30 seconds.")
                Text("Hidden windows make no automatic requests.")
                    .foregroundStyle(.secondary)
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
            endpoint = model.endpoint
            tokenPath = await model.tokenPath
        }
    }
}
