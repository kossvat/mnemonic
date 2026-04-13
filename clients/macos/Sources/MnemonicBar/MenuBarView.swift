import SwiftUI

struct MenuBarView: View {
    @ObservedObject var service: MnemonicService
    @State private var searchText = ""
    @State private var searchResults: [MemoryEntry] = []
    @State private var isSearching = false
    @State private var showQuickSave = false
    @State private var quickTitle = ""
    @State private var quickType = "note"
    @State private var actionFeedback: String? = nil
    @State private var expandedId: UUID? = nil
    @State private var typeFilter: String? = nil
    @State private var filteredEntries: [MemoryEntry] = []

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            // Header
            header
            Divider().padding(.horizontal, 12)

            // Search
            searchBar
                .padding(.horizontal, 12)
                .padding(.top, 8)
                .padding(.bottom, 4)

            if isSearching {
                // Search results
                searchResultsSection
                    .padding(.horizontal, 12)
                    .padding(.bottom, 8)
            } else if showQuickSave {
                // Quick save form
                quickSaveForm
                    .padding(.horizontal, 12)
                    .padding(.bottom, 8)
            } else {
                // Stats cards
                statsGrid
                    .padding(.horizontal, 12)
                    .padding(.vertical, 6)

                // Last activity info
                lastActivityRow
                    .padding(.horizontal, 12)
                    .padding(.bottom, 4)

                Divider().padding(.horizontal, 12)

                // Recent memories
                recentSection
                    .padding(.horizontal, 12)
                    .padding(.vertical, 8)
            }

            // Action feedback toast
            if let feedback = actionFeedback {
                FeedbackToast(text: feedback)
                    .padding(.horizontal, 12)
                    .padding(.bottom, 4)
            }

            Divider().padding(.horizontal, 12)

            // Action buttons
            actionBar

            Divider().padding(.horizontal, 12)

            // Footer
            footer
        }
        .frame(width: 340)
    }

    // MARK: - Header

    private var header: some View {
        HStack {
            Image(systemName: "brain.head.profile")
                .font(.system(size: 18, weight: .medium))
                .foregroundStyle(.purple)

            VStack(alignment: .leading, spacing: 1) {
                Text("Mnemonic")
                    .font(.system(size: 13, weight: .semibold))
                Text("\(service.stats.total) memories")
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
            }

            Spacer()

            HStack(spacing: 4) {
                Circle()
                    .fill(service.stats.isRunning ? Color.green : Color.red)
                    .frame(width: 7, height: 7)
                Text(service.stats.isRunning ? "Running" : "Stopped")
                    .font(.system(size: 10))
                    .foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 10)
    }

    // MARK: - Search Bar

    private var searchBar: some View {
        HStack(spacing: 6) {
            Image(systemName: "magnifyingglass")
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)

            TextField("Search memories...", text: $searchText)
                .textFieldStyle(.plain)
                .font(.system(size: 12))
                .onSubmit {
                    if !searchText.isEmpty {
                        searchResults = service.search(query: searchText)
                        isSearching = true
                        showQuickSave = false
                    }
                }

            if isSearching {
                Button(action: {
                    searchText = ""
                    searchResults = []
                    isSearching = false
                }) {
                    Image(systemName: "xmark.circle.fill")
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                }
                .buttonStyle(.plain)
            }
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))
    }

    // MARK: - Search Results

    private var searchResultsSection: some View {
        VStack(alignment: .leading, spacing: 6) {
            Text("\(searchResults.count) results")
                .font(.system(size: 11, weight: .medium))
                .foregroundStyle(.secondary)

            if searchResults.isEmpty {
                Text("Nothing found")
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 12)
            } else {
                ForEach(searchResults.prefix(8)) { entry in
                    MemoryRow(entry: entry, expandedId: $expandedId)
                }
            }
        }
    }

    // MARK: - Quick Save

    private var quickSaveForm: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack {
                Button(action: { showQuickSave = false; quickTitle = "" }) {
                    HStack(spacing: 3) {
                        Image(systemName: "chevron.left")
                            .font(.system(size: 10, weight: .semibold))
                        Text("Back")
                            .font(.system(size: 11, weight: .medium))
                    }
                    .foregroundStyle(.purple)
                }
                .buttonStyle(.plain)

                Spacer()

                Text("QUICK SAVE")
                    .font(.system(size: 11, weight: .medium))
                    .foregroundStyle(.secondary)
            }

            TextField("What to remember...", text: $quickTitle)
                .textFieldStyle(.plain)
                .font(.system(size: 12))
                .padding(.horizontal, 8)
                .padding(.vertical, 5)
                .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 6))

            HStack(spacing: 6) {
                TypePicker(selected: $quickType)

                Spacer()

                Button("Save") {
                    if !quickTitle.isEmpty {
                        service.quickSave(title: quickTitle, type: quickType)
                        quickTitle = ""
                        showQuickSave = false
                        showFeedback("Memory saved")
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.5) {
                            service.refresh()
                        }
                    }
                }
                .buttonStyle(.borderedProminent)
                .tint(.purple)
                .controlSize(.small)
                .font(.system(size: 11, weight: .medium))
                .disabled(quickTitle.isEmpty)
            }
        }
    }

    // MARK: - Stats Grid

    private var statsGrid: some View {
        LazyVGrid(columns: [
            GridItem(.flexible()),
            GridItem(.flexible()),
            GridItem(.flexible()),
        ], spacing: 6) {
            StatCard(icon: "lightbulb.fill", label: "Decisions", value: "\(service.stats.decisions)", color: .orange, isActive: typeFilter == "decision") {
                toggleFilter("decision")
            }
            StatCard(icon: "bubble.left.fill", label: "Feedback", value: "\(service.stats.feedback)", color: .blue, isActive: typeFilter == "feedback") {
                toggleFilter("feedback")
            }
            StatCard(icon: "note.text", label: "Notes", value: "\(service.stats.notes)", color: .gray, isActive: typeFilter == "note") {
                toggleFilter("note")
            }
        }
    }

    private func toggleFilter(_ type: String) {
        if typeFilter == type {
            typeFilter = nil
            filteredEntries = []
        } else {
            typeFilter = type
            filteredEntries = service.filterByType(type)
        }
    }

    // MARK: - Recent

    private var recentSection: some View {
        let filter = typeFilter
        let entries = filter != nil ? filteredEntries : Array(service.recent.prefix(6))
        let title = filter.map { "\($0.capitalized)s" } ?? "Recent"
        let emptyText = filter.map { "No \($0)s yet" } ?? "No memories yet"

        return VStack(alignment: .leading, spacing: 6) {
            HStack {
                if filter != nil {
                    Button(action: { typeFilter = nil; filteredEntries = [] }) {
                        HStack(spacing: 3) {
                            Image(systemName: "chevron.left")
                                .font(.system(size: 10, weight: .semibold))
                            Text("Back")
                                .font(.system(size: 11, weight: .medium))
                        }
                        .foregroundStyle(.purple)
                    }
                    .buttonStyle(.plain)

                    Spacer()

                    Text(title)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                        .textCase(.uppercase)
                } else {
                    Text(title)
                        .font(.system(size: 11, weight: .medium))
                        .foregroundStyle(.secondary)
                        .textCase(.uppercase)
                }

                Spacer()
            }

            if entries.isEmpty {
                Text(emptyText)
                    .font(.system(size: 12))
                    .foregroundStyle(.tertiary)
                    .frame(maxWidth: .infinity, alignment: .center)
                    .padding(.vertical, 8)
            } else {
                ForEach(entries.prefix(8)) { entry in
                    MemoryRow(entry: entry, expandedId: $expandedId)
                }
            }
        }
    }

    // MARK: - Last Activity

    private var lastActivityRow: some View {
        HStack(spacing: 6) {
            if let hours = service.stats.silentHours {
                if hours >= 2.0 {
                    Image(systemName: "exclamationmark.circle.fill")
                        .font(.system(size: 10))
                        .foregroundStyle(.orange)
                    Text("Last activity: \(formatHours(hours)) ago")
                        .font(.system(size: 10))
                        .foregroundStyle(.orange)
                } else {
                    Image(systemName: "checkmark.circle.fill")
                        .font(.system(size: 10))
                        .foregroundStyle(.green)
                    Text("Active \(formatHours(hours)) ago")
                        .font(.system(size: 10))
                        .foregroundStyle(.secondary)
                }
            } else {
                Image(systemName: "minus.circle")
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
                Text("No activity yet")
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
            }
            Spacer()
        }
    }

    private func formatHours(_ h: Double) -> String {
        if h >= 48 { return "\(Int(h / 24))d" }
        if h >= 1 { return "\(Int(h))h" }
        return "\(Int(h * 60))m"
    }

    private func showFeedback(_ text: String) {
        actionFeedback = text
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) {
            actionFeedback = nil
        }
    }

    // MARK: - Action Bar

    private var actionBar: some View {
        HStack(spacing: 12) {
            ActionButton(icon: "plus.circle.fill", label: "Save", color: .purple) {
                showQuickSave.toggle()
                isSearching = false
            }

            if service.stats.isRunning {
                ActionButton(icon: "stop.circle.fill", label: "Stop", color: .red) {
                    service.stopDaemon()
                    showFeedback("Daemon stopped")
                    DispatchQueue.main.asyncAfter(deadline: .now() + 1) { service.refresh() }
                }
            } else {
                ActionButton(icon: "play.circle.fill", label: "Start", color: .green) {
                    service.startDaemon()
                    showFeedback("Daemon starting...")
                    DispatchQueue.main.asyncAfter(deadline: .now() + 2) { service.refresh() }
                }
            }

            ActionButton(icon: "doc.text.magnifyingglass", label: "Log", color: .secondary) {
                service.openLog()
                showFeedback("Log opened")
            }

            ActionButton(icon: "sparkles", label: "Context", color: .orange) {
                service.generateContext()
                showFeedback("Context generated ✓")
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) { service.refresh() }
            }
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }

    // MARK: - Footer

    private var footer: some View {
        HStack {
            Text("DB: \(String(format: "%.0f", service.stats.dbSizeKB)) KB")
                .font(.system(size: 10))
                .foregroundStyle(.tertiary)

            if let pid = service.stats.pid {
                Text("PID: \(pid)")
                    .font(.system(size: 10))
                    .foregroundStyle(.tertiary)
            }

            Spacer()

            Button(action: { service.refresh() }) {
                Image(systemName: "arrow.clockwise")
                    .font(.system(size: 10))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)

            Button(action: { NSApplication.shared.terminate(nil) }) {
                Image(systemName: "xmark.circle")
                    .font(.system(size: 10))
            }
            .buttonStyle(.plain)
            .foregroundStyle(.secondary)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
    }
}

// MARK: - Action Button

struct ActionButton: View {
    let icon: String
    let label: String
    let color: Color
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            VStack(spacing: 2) {
                Image(systemName: icon)
                    .font(.system(size: 14))
                    .foregroundStyle(color)
                Text(label)
                    .font(.system(size: 9))
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 4)
        }
        .buttonStyle(.plain)
        .contentShape(Rectangle())
    }
}

// MARK: - Type Picker

struct TypePicker: View {
    @Binding var selected: String

    private let types = [
        ("note", "note.text", Color.gray),
        ("decision", "lightbulb.fill", Color.orange),
        ("feedback", "bubble.left.fill", Color.blue),
    ]

    var body: some View {
        HStack(spacing: 4) {
            ForEach(types, id: \.0) { type, icon, color in
                Button(action: { selected = type }) {
                    Image(systemName: icon)
                        .font(.system(size: 11))
                        .foregroundStyle(selected == type ? color : Color.gray)
                        .padding(4)
                        .background(
                            selected == type ? color.opacity(0.15) : Color.clear,
                            in: RoundedRectangle(cornerRadius: 4)
                        )
                }
                .buttonStyle(.plain)
            }
        }
    }
}

// MARK: - Stat Card

struct StatCard: View {
    let icon: String
    let label: String
    let value: String
    let color: Color
    var isActive: Bool = false
    var action: () -> Void = {}

    var body: some View {
        Button(action: action) {
            VStack(spacing: 3) {
                Image(systemName: icon)
                    .font(.system(size: 14))
                    .foregroundStyle(color)
                Text(value)
                    .font(.system(size: 16, weight: .semibold, design: .rounded))
                Text(label)
                    .font(.system(size: 9))
                    .foregroundStyle(.secondary)
            }
            .frame(maxWidth: .infinity)
            .padding(.vertical, 8)
            .background(
                isActive ? color.opacity(0.15) : Color.primary.opacity(0.04),
                in: RoundedRectangle(cornerRadius: 8)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(isActive ? color.opacity(0.4) : Color.clear, lineWidth: 1)
            )
        }
        .buttonStyle(.plain)
    }
}

// MARK: - Memory Row

struct MemoryRow: View {
    let entry: MemoryEntry
    @Binding var expandedId: UUID?

    private var isExpanded: Bool { expandedId == entry.id }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            Button(action: {
                withAnimation(.easeInOut(duration: 0.15)) {
                    expandedId = isExpanded ? nil : entry.id
                }
            }) {
                HStack(spacing: 8) {
                    Image(systemName: iconForType(entry.type))
                        .font(.system(size: 10))
                        .foregroundStyle(colorForType(entry.type))
                        .frame(width: 16)

                    Text(entry.title)
                        .font(.system(size: 11))
                        .lineLimit(isExpanded ? 10 : 1)
                        .truncationMode(.tail)
                        .foregroundStyle(.primary)
                        .frame(maxWidth: .infinity, alignment: .leading)

                    Image(systemName: isExpanded ? "chevron.up" : "chevron.down")
                        .font(.system(size: 8, weight: .medium))
                        .foregroundStyle(.tertiary)
                }
                .padding(.vertical, 4)
                .padding(.horizontal, 4)
                .contentShape(Rectangle())
            }
            .buttonStyle(.plain)

            if isExpanded {
                VStack(alignment: .leading, spacing: 4) {
                    // Type + importance
                    HStack(spacing: 8) {
                        Label(entry.type, systemImage: iconForType(entry.type))
                            .font(.system(size: 10))
                            .foregroundStyle(colorForType(entry.type))

                        Spacer()

                        HStack(spacing: 3) {
                            Text("Importance:")
                                .font(.system(size: 9))
                                .foregroundStyle(.tertiary)
                            ImportanceBar(value: entry.importance)
                        }
                    }

                    // Copy button
                    Button(action: {
                        NSPasteboard.general.clearContents()
                        NSPasteboard.general.setString(entry.title, forType: .string)
                    }) {
                        HStack(spacing: 4) {
                            Image(systemName: "doc.on.doc")
                                .font(.system(size: 9))
                            Text("Copy")
                                .font(.system(size: 10))
                        }
                        .foregroundStyle(.secondary)
                        .padding(.horizontal, 6)
                        .padding(.vertical, 3)
                        .background(.quaternary.opacity(0.5), in: RoundedRectangle(cornerRadius: 4))
                    }
                    .buttonStyle(.plain)
                }
                .padding(.leading, 28)
                .padding(.trailing, 4)
                .padding(.bottom, 4)
            }
        }
        .background(
            RoundedRectangle(cornerRadius: 4)
                .fill(isExpanded ? Color.primary.opacity(0.03) : Color.clear)
        )
    }

    private func iconForType(_ type: String) -> String {
        switch type {
        case "decision": return "lightbulb.fill"
        case "feedback": return "bubble.left.fill"
        case "security": return "shield.fill"
        case "session_summary": return "clock.fill"
        default: return "note.text"
        }
    }

    private func colorForType(_ type: String) -> Color {
        switch type {
        case "decision": return .orange
        case "feedback": return .blue
        case "security": return .red
        case "session_summary": return .purple
        default: return .gray
        }
    }
}

// MARK: - Importance Bar

struct ImportanceBar: View {
    let value: Double

    var body: some View {
        HStack(spacing: 1) {
            ForEach(0..<5) { i in
                RoundedRectangle(cornerRadius: 1)
                    .fill(Double(i) / 5.0 < value ? barColor : Color.gray.opacity(0.2))
                    .frame(width: 3, height: 8)
            }
        }
    }

    private var barColor: Color {
        if value >= 0.8 { return .orange }
        if value >= 0.5 { return .green }
        return .gray
    }
}

// MARK: - Feedback Toast

struct FeedbackToast: View {
    let text: String

    var body: some View {
        HStack(spacing: 6) {
            Image(systemName: "info.circle.fill")
                .font(.system(size: 10))
                .foregroundStyle(.blue)
            Text(text)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            Spacer()
        }
        .padding(.horizontal, 8)
        .padding(.vertical, 5)
        .background(.blue.opacity(0.08), in: RoundedRectangle(cornerRadius: 6))
        .transition(.opacity)
    }
}
