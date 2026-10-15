import Foundation

/// These words name only synthetic projects and their invented work.
enum FixtureVocabulary {
    static let projects = [
        "Harbor", "Beacon", "Ledger", "Atlas", "Juniper", "Orchard", "Lantern", "Quarry", "Meadow", "Pylon",
        "Tundra", "Vessel", "Willow", "Cobalt", "Ember", "Fathom", "Aspen", "Birch", "Cedar", "Dune",
        "Estuary", "Fern", "Grove", "Hearth", "Iris", "Jasper", "Kestrel", "Lagoon", "Maple", "Nectar",
        "Opal", "Pebble", "Quartz", "Reed", "Spruce", "Thistle", "Umber", "Vale", "Wren", "Zephyr"
    ]
    static let work = [
        "draft", "review", "guide", "cache", "response", "contract", "delivery", "cleanup", "first", "second",
        "the", "a", "of", "for", "and", "check", "record", "result", "local", "keep", "verify", "plan",
        "build", "use", "read", "task", "incomplete"
    ]
    // Schema labels are not words for invented project narratives.
    static let schemaLabels: Set<String> = [
        "no plan file", "claude", "codex", "owner", "program", "running", "quiet", "waiting",
        "active", "archived", "ignored", "proposed", "planned", "building", "review", "blocked", "done",
        "passed", "failed", "stale", "none", "registry", "checkbox", "funnel", "branch", "first_open",
        "high", "medium", "low", "contacted", "waiting_on_owner", "decision", "correction"
    ]
    private static let words = Set((projects + work).map { word in word.lowercased() })

    static func contains(_ value: String) -> Bool {
        if schemaLabels.contains(value) { return true }
        guard !value.isEmpty, value.unicodeScalars.allSatisfy({ scalar in
            (65...90).contains(scalar.value) || (97...122).contains(scalar.value)
                || (48...57).contains(scalar.value) || scalar == " " || scalar == "-"
        }) else { return false }
        if value.range(of: "^(segment|owner|fixture|unit|decision)-[1-9][0-9]*$", options: .regularExpression) != nil {
            return true
        }
        if value.range(of: "^T[0-9]{2,}$", options: .regularExpression) != nil { return true }
        if value.range(of: "^decision-[a-z]$", options: .regularExpression) != nil { return true }
        let tokens = value.split(separator: " ")
        guard !tokens.isEmpty else { return false }
        return tokens.allSatisfy { token in
            let text = String(token)
            let parts = text.split(separator: "-", omittingEmptySubsequences: false)
            return parts.allSatisfy { part in
                !part.isEmpty && (words.contains(part.lowercased())
                    || part.allSatisfy { character in character.isASCII && character.isNumber })
            }
        }
    }
}
