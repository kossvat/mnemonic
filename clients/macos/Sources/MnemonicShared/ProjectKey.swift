import Foundation

enum ProjectKey {
    /// Registry keys use Unicode alphanumerics and dashes, with a colon
    /// between nested project parts. Validate without changing the identity.
    static func isValid(_ value: String) -> Bool {
        guard !value.isEmpty else { return false }
        let parts = value.split(separator: ":", omittingEmptySubsequences: false)
        return parts.allSatisfy { part in
            !part.isEmpty && !part.hasPrefix("-") && !part.hasSuffix("-")
                && !part.contains("--") && part.unicodeScalars.allSatisfy { scalar in
                    scalar == "-" || scalar.properties.isAlphabetic
                        || scalar.properties.numericType != nil
                }
        }
    }

    static func encoded(_ value: String) -> String? {
        guard isValid(value) else { return nil }
        return value.addingPercentEncoding(withAllowedCharacters:
            CharacterSet(charactersIn: "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-"))
    }
}
