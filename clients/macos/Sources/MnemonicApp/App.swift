import SwiftUI

@main
struct MnemonicApp: App {
    @StateObject private var model = MnemonicAppModel()

    var body: some Scene {
        WindowGroup {
            ContentView(model: model)
                .frame(minWidth: 980, minHeight: 640)
                .preferredColorScheme(colorScheme)
        }
        .defaultSize(width: 1180, height: 760)
        .commands {
            CommandGroup(after: .newItem) {
                Button("Refresh") {
                    NotificationCenter.default.post(name: .mnemonicRefresh, object: nil)
                }
                .keyboardShortcut("r", modifiers: [.command])

                Button("Focus Search") {
                    NotificationCenter.default.post(name: .mnemonicFocusSearch, object: nil)
                }
                .keyboardShortcut("f", modifiers: [.command])
            }

            CommandMenu("Mnemonic") {
                ForEach(Array(AppRoute.allCases.enumerated()), id: \.element.id) { index, route in
                    Button(route.title) {
                        NotificationCenter.default.post(
                            name: .mnemonicSelectRoute,
                            object: route.rawValue
                        )
                    }
                    .keyboardShortcut(KeyEquivalent(Character("\(index + 1)")), modifiers: [.command])
                }
            }
        }

        Settings {
            SettingsView(model: model)
                .frame(width: 420)
        }
    }

    private var colorScheme: ColorScheme? {
        switch model.theme {
        case "light": .light
        case "dark": .dark
        default: nil
        }
    }
}
