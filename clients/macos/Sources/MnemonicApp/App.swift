import SwiftUI
import AppKit
import MnemonicShared

@main
@MainActor
struct MnemonicApp: App {
    @NSApplicationDelegateAdaptor(NativeAppDelegate.self) private var delegate
    @StateObject private var model: MnemonicAppModel

    init() {
        let arguments = CommandLine.arguments
        if arguments.contains("--render-project-fixtures") {
            do {
                try AppFixturePreview.run(arguments: arguments)
                exit(0)
            } catch {
                FileHandle.standardError.write(Data("Project fixture rendering failed.\n".utf8))
                exit(2)
            }
        }
        if arguments.contains("--render-timeline-fixtures") {
            do {
                try TimelineFixturePreview.run(arguments: arguments)
                exit(0)
            } catch {
                FileHandle.standardError.write(Data("Timeline fixture rendering failed.\n".utf8))
                exit(2)
            }
        }
        if arguments.contains("--render-graph-fixtures") {
            do {
                try GraphFixturePreview.run(arguments: arguments)
                exit(0)
            } catch {
                FileHandle.standardError.write(Data("Graph fixture rendering failed.\n".utf8))
                exit(2)
            }
        }
        if arguments.contains("--fixture-graph"), !arguments.contains("--fixture-window") {
            FileHandle.standardError.write(Data("The graph fixture requires the fixture window.\n".utf8))
            exit(2)
        }
        if arguments.contains("--fixture-window"), !MnemonicProfile.current.isolated {
            FileHandle.standardError.write(Data("The fixture window requires an isolated profile.\n".utf8))
            exit(2)
        }
        let model = MnemonicAppModel()
        if arguments.contains("--fixture-graph") { model.route = .graph }
        if let index = arguments.firstIndex(of: "--open-url"), arguments.count > index + 1,
           let url = URL(string: arguments[index + 1]) {
            model.open(url)
        }
        if let identity = model.identity {
            AppLinkReadiness.announce(arguments: arguments, identity: identity)
        }
        _model = StateObject(wrappedValue: model)
    }

    var body: some Scene {
        WindowGroup(id: DashboardActivation.sceneID) {
            DashboardRoot(model: model)
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
                            object: route.sidebarRoute.rawValue
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

@MainActor
private struct DashboardRoot: View {
    @ObservedObject var model: MnemonicAppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        ContentView(model: model)
            .environment(\.mnemonicDashboardWindowAttached, {
                model.dashboardActivation.windowDidAttach()
            })
            .onAppear {
                let action = openWindow
                model.dashboardActivation.registerOpener {
                    action(id: DashboardActivation.sceneID)
                }
            }
    }
}

@MainActor
final class NativeAppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        NSApp.setActivationPolicy(.regular)
        NSApp.activate(ignoringOtherApps: true)
        DispatchQueue.main.async {
            DashboardActivation().activate()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}
