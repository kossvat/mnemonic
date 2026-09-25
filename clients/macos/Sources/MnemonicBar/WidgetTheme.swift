import SwiftUI

/// Design tokens for the menu-bar popover.
///
/// Multi-theme: the shipped default is v2 "Hybrid" (neutral macOS surface +
/// warm amber on the chart and primary button, ported 1:1 from the locked
/// Claude Design tokens). Alternative looks are switched via the MN_THEME
/// environment variable — the preview renderer launches one process per
/// theme so design directions can be compared as PNGs before any of them
/// ships:
///   - "hybrid"     — the current look (default)
///   - "noir"       — near-black surfaces, hotter neon-amber accent, glow
///   - "glass"      — lighter graphite, frosted card fills, large radii
///   - "editorial"  — flat: card chrome fades to hairlines, air does the work
enum WT {
    /// Picked Noir from the 2026-06-10 design round (rendered
    /// side-by-side via --render-previews). MN_THEME stays as the dev
    /// switch for future rounds.
    static let theme = ProcessInfo.processInfo.environment["MN_THEME"] ?? "noir"

    private static func pick<T>(_ hybrid: T, noir: T? = nil, glass: T? = nil, editorial: T? = nil) -> T {
        switch theme {
        case "noir": return noir ?? hybrid
        case "glass": return glass ?? hybrid
        case "editorial": return editorial ?? hybrid
        default: return hybrid
        }
    }

    // Surface / text
    static let bg = pick(
        Color(light: Color(hex: 0xFBFBFD), dark: Color(hex: 0x1C1C1E)),
        noir: Color(light: Color(hex: 0xFBFBFD), dark: Color(hex: 0x0B0B0D)),
        glass: Color(light: Color(hex: 0xF2F2F7), dark: Color(hex: 0x2A2A2E)),
        editorial: Color(light: Color(hex: 0xFCFCFD), dark: Color(hex: 0x161618))
    )
    static let text = pick(
        Color(light: Color(hex: 0x1D1D1F), dark: Color(hex: 0xF5F5F7)),
        noir: Color(light: Color(hex: 0x1D1D1F), dark: Color(hex: 0xFAFAFC))
    )
    static let sub = Color(light: Color(hex: 0x3C3C43, opacity: 0.62),
                           dark: Color(hex: 0xEBEBF5, opacity: 0.62))
    static let ter = Color(light: Color(hex: 0x3C3C43, opacity: 0.34),
                           dark: Color(hex: 0xEBEBF5, opacity: 0.32))
    static let sep = pick(
        Color(light: Color.black.opacity(0.09), dark: Color.white.opacity(0.10)),
        noir: Color(light: Color.black.opacity(0.09), dark: Color.white.opacity(0.08)),
        glass: Color(light: Color.black.opacity(0.10), dark: Color.white.opacity(0.16)),
        editorial: Color(light: Color.black.opacity(0.06), dark: Color.white.opacity(0.06))
    )
    static let fill = pick(
        Color(light: Color.black.opacity(0.035), dark: Color.white.opacity(0.05)),
        noir: Color(light: Color.black.opacity(0.035), dark: Color.white.opacity(0.06)),
        glass: Color(light: Color.white.opacity(0.55), dark: Color.white.opacity(0.09)),
        editorial: Color(light: Color.clear, dark: Color.clear)
    )
    static let btnFill = pick(
        Color(light: Color.black.opacity(0.05), dark: Color.white.opacity(0.08)),
        noir: Color(light: Color.black.opacity(0.05), dark: Color.white.opacity(0.10)),
        glass: Color(light: Color.white.opacity(0.7), dark: Color.white.opacity(0.13)),
        editorial: Color(light: Color.black.opacity(0.04), dark: Color.white.opacity(0.06))
    )

    // Amber accent (chart + primary button only)
    static let accent = pick(
        Color(light: Color(hex: 0xC9722C), dark: Color(hex: 0xF2A35E)),
        noir: Color(light: Color(hex: 0xC9722C), dark: Color(hex: 0xFFA245))
    )
    static let accentGlow = pick(
        Color(light: Color(hex: 0xC9722C, opacity: 0.18), dark: Color(hex: 0xF2A35E, opacity: 0.30)),
        noir: Color(light: Color(hex: 0xC9722C, opacity: 0.18), dark: Color(hex: 0xFFA245, opacity: 0.45))
    )
    /// Darker accent stop for gradient fills (primary button).
    static let accentDeep = pick(
        Color(light: Color(hex: 0xB35F1F), dark: Color(hex: 0xE08236)),
        noir: Color(light: Color(hex: 0xB35F1F), dark: Color(hex: 0xF07F2E))
    )

    // Chart neutrals. Idle bars were nearly invisible on dark (0.16) —
    // the week chart read as "one amber bar floating in nothing".
    static let barIdle = pick(
        Color(light: Color(hex: 0x3C3C43, opacity: 0.18), dark: Color(hex: 0xEBEBF5, opacity: 0.24)),
        noir: Color(light: Color(hex: 0x3C3C43, opacity: 0.18), dark: Color(hex: 0xEBEBF5, opacity: 0.20))
    )
    static let track = Color(light: Color(hex: 0x767680, opacity: 0.12),
                             dark: Color(hex: 0x767680, opacity: 0.30))
    static let thumb = Color(light: Color(hex: 0xFFFFFF), dark: Color(hex: 0x5B5B5E))
    static let thumbText = Color(light: Color(hex: 0x1D1D1F), dark: Color.white)

    // Status
    static let working = Color(light: Color(hex: 0x28A745), dark: Color(hex: 0x30D158))
    static let idle = Color(light: Color(hex: 0xE08600), dark: Color(hex: 0xFFB340))
    static let stopped = Color(light: Color(hex: 0xE0352B), dark: Color(hex: 0xFF453A))

    // Memory type tints (functional, not a second accent)
    static let memDecision = working
    static let memFeedback = Color(light: Color(hex: 0xD2453B), dark: Color(hex: 0xFF6B6B))
    static let memNote = sub

    enum R {
        static let card: CGFloat = pick(14, noir: 16, glass: 18, editorial: 12)
        static let inner: CGFloat = pick(12, noir: 14, glass: 16, editorial: 10)
        static let btn: CGFloat = pick(10, noir: 12, glass: 14, editorial: 9)
        static let chip: CGFloat = pick(8, noir: 8, glass: 10, editorial: 7)
    }
}

// MARK: - Color helpers (local to the widget target)

extension Color {
    init(hex: UInt32, opacity: Double = 1.0) {
        let r = Double((hex >> 16) & 0xFF) / 255.0
        let g = Double((hex >> 8) & 0xFF) / 255.0
        let b = Double(hex & 0xFF) / 255.0
        self.init(.sRGB, red: r, green: g, blue: b, opacity: opacity)
    }

    /// Resolves differently in light vs dark appearance via NSColor.
    init(light: Color, dark: Color) {
        self = Color(nsColor: NSColor(name: nil, dynamicProvider: { appearance in
            let isDark = appearance.bestMatch(from: [.darkAqua, .vibrantDark,
                .accessibilityHighContrastDarkAqua, .accessibilityHighContrastVibrantDark]) != nil
            return NSColor(isDark ? dark : light)
        }))
    }
}
