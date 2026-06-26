import Foundation
import SwiftUI

enum AppRoute: String, CaseIterable, Identifiable {
    case overview
    case memories
    case search
    case entities
    case graph
    case memoryMap
    case manage

    var id: String { rawValue }

    var title: String {
        switch self {
        case .overview: "Overview"
        case .memories: "Memories"
        case .search: "Search"
        case .entities: "Entities"
        case .graph: "Entity Graph"
        case .memoryMap: "Memory Map"
        case .manage: "Manage"
        }
    }

    var symbol: String {
        switch self {
        case .overview: "gauge.with.dots.needle.bottom.50percent"
        case .memories: "tray.full"
        case .search: "magnifyingglass"
        case .entities: "person.3.sequence"
        case .graph: "point.3.connected.trianglepath.dotted"
        case .memoryMap: "rectangle.connected.to.line.below"
        case .manage: "slider.horizontal.3"
        }
    }
}

@MainActor
final class MnemonicAppModel: ObservableObject {
    @Published var route: AppRoute = .overview
    @Published var status: StatusResponse?
    @Published var memories: [Memory] = []
    @Published var entities: [EntitySummary] = []
    @Published var daily: [DailyCount] = []
    @Published var graph: GraphResponse?
    @Published var isLoading = false
    @Published var setupMessage: String?
    @Published var toastMessage: String?
    @Published var endpoint: String {
        didSet {
            UserDefaults.standard.set(endpoint, forKey: "MnemonicEndpoint")
            Task { try? await client.setEndpoint(endpoint) }
        }
    }
    @Published var refreshInterval: Double {
        didSet { UserDefaults.standard.set(refreshInterval, forKey: "MnemonicRefreshInterval") }
    }
    @Published var theme: String {
        didSet { UserDefaults.standard.set(theme, forKey: "MnemonicTheme") }
    }

    let client = MnemonicClient()

    init() {
        self.endpoint = UserDefaults.standard.string(forKey: "MnemonicEndpoint")
            ?? "http://127.0.0.1:3737"
        let savedInterval = UserDefaults.standard.double(forKey: "MnemonicRefreshInterval")
        self.refreshInterval = savedInterval > 0 ? savedInterval : 30
        self.theme = UserDefaults.standard.string(forKey: "MnemonicTheme") ?? "system"
        Task { try? await client.setEndpoint(endpoint) }
    }

    var isOnline: Bool {
        status != nil && setupMessage == nil
    }

    var tokenPath: String {
        get async { await client.tokenPath() }
    }

    func refreshAll() async {
        isLoading = true
        defer { isLoading = false }
        do {
            async let status = client.fetchStatus()
            async let memories = client.fetchMemories(limit: 120)
            async let entities = client.fetchEntities(limit: 500)
            async let daily = client.fetchDaily(days: 14)
            async let graph = client.fetchGraph(limit: 120)
            self.status = try await status
            self.memories = try await memories.results
            self.entities = try await entities.results
            self.daily = try await daily.days
            self.graph = try await graph
            setupMessage = nil
        } catch {
            setupMessage = error.localizedDescription
        }
    }

    func refreshOverview() async {
        do {
            async let status = client.fetchStatus()
            async let daily = client.fetchDaily(days: 14)
            self.status = try await status
            self.daily = try await daily.days
            setupMessage = nil
        } catch {
            setupMessage = error.localizedDescription
        }
    }

    func refreshMemories() async {
        do {
            memories = try await client.fetchMemories(limit: 150).results
            setupMessage = nil
        } catch {
            setupMessage = error.localizedDescription
        }
    }

    func refreshEntities() async {
        do {
            entities = try await client.fetchEntities(limit: 500).results
            setupMessage = nil
        } catch {
            setupMessage = error.localizedDescription
        }
    }

    func showToast(_ message: String) {
        toastMessage = message
        Task {
            try? await Task.sleep(for: .seconds(2))
            if toastMessage == message {
                toastMessage = nil
            }
        }
    }
}
