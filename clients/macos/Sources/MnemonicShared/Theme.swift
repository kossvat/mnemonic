import SwiftUI

/// Design tokens for Mnemonic.
///
/// Aesthetic: "library card catalog meets calm operations console".
/// Notion-inspired warm neutrals, single sparingly-used accent, content-first
/// typography, generous whitespace, no harsh shadows. Cards differentiate via
/// background contrast plus a 1pt 6%-opacity border — no drop shadows anywhere.
///
/// Every color has light + dark variants. The dark palette deepens the warm
/// neutrals rather than going pure black, so memories feel like paper in low
/// light instead of a glowing terminal.
public enum Theme {}

// MARK: - Colors

public extension Theme {
    enum Palette {
        // Backgrounds — warm offwhite in light, warm near-black in dark
        public static let bgPrimary = Color(
            light: Color(hex: 0xF7F6F3),
            dark:  Color(hex: 0x1F1E1C)
        )
        public static let bgSurface = Color(
            light: Color(hex: 0xFFFFFF),
            dark:  Color(hex: 0x2A2926)
        )
        public static let bgTint = Color(
            light: Color(hex: 0xEFEEE9),
            dark:  Color(hex: 0x35332F)
        )
        public static let border = Color(
            light: Color.black.opacity(0.06),
            dark:  Color.white.opacity(0.08)
        )

        // Text — warm black, never #000
        public static let textPrimary = Color(
            light: Color(hex: 0x37352F),
            dark:  Color(hex: 0xE8E6DF)
        )
        public static let textMuted = Color(
            light: Color(hex: 0x787569),
            dark:  Color(hex: 0xA3A09A)
        )
        public static let textSubtle = Color(
            light: Color(hex: 0xB6B3A7),
            dark:  Color(hex: 0x6B6864)
        )

        // Single accent — used sparingly: selected nav, focus rings, primary CTA.
        public static let accent = Color(
            light: Color(hex: 0x2383E2),
            dark:  Color(hex: 0x4A9CEC)
        )

        // Memory type colors — muted, never saturated
        public static let decision = Color(
            light: Color(hex: 0x297A3A),
            dark:  Color(hex: 0x4FA968)
        )
        public static let feedback = Color(
            light: Color(hex: 0xA14545),
            dark:  Color(hex: 0xC97373)
        )
        public static let note = Color(
            light: Color(hex: 0x6B6960),
            dark:  Color(hex: 0xA3A09A)
        )
        public static let security = Color(
            light: Color(hex: 0x8B5E1F),
            dark:  Color(hex: 0xC79555)
        )
        public static let session = Color(
            light: Color(hex: 0x5D4F8B),
            dark:  Color(hex: 0x9787C7)
        )

        // Entity type colors (slightly different palette than memory types
        // so the graph viz has its own visual identity)
        public static let entityProject = Color(
            light: Color(hex: 0x4A6D8C),
            dark:  Color(hex: 0x7BA3C4)
        )
        public static let entityTech = Color(
            light: Color(hex: 0x6B5B95),
            dark:  Color(hex: 0xA295C9)
        )
        public static let entityPerson = Color(
            light: Color(hex: 0xB07B4F),
            dark:  Color(hex: 0xD5A878)
        )
        public static let entityModule = Color(
            light: Color(hex: 0x5A8C5A),
            dark:  Color(hex: 0x88BB88)
        )
        public static let entityFile = Color(
            light: Color(hex: 0x8C5A6D),
            dark:  Color(hex: 0xBA8898)
        )
        public static let entityConcept = Color(
            light: Color(hex: 0x6B6960),
            dark:  Color(hex: 0xA3A09A)
        )
    }
}

// MARK: - Typography

public extension Theme {
    enum Font {
        // SF Pro Text via .system — Apple's body face on macOS.
        // Tracking is applied via .tracking() modifiers since SwiftUI Font
        // doesn't expose tracking directly.

        public static let display     = SwiftUI.Font.system(size: 32, weight: .bold,    design: .default)
        public static let heading     = SwiftUI.Font.system(size: 20, weight: .semibold,design: .default)
        public static let title       = SwiftUI.Font.system(size: 15, weight: .medium,  design: .default)
        public static let body        = SwiftUI.Font.system(size: 13, weight: .regular, design: .default)
        public static let bodyMedium  = SwiftUI.Font.system(size: 13, weight: .medium,  design: .default)
        public static let caption     = SwiftUI.Font.system(size: 11, weight: .regular, design: .default)
        public static let captionBold = SwiftUI.Font.system(size: 11, weight: .semibold,design: .default)
        public static let mono        = SwiftUI.Font.system(size: 12, weight: .regular, design: .monospaced)

        // Tracking presets — apply as `.tracking(Theme.Font.trackingDisplay)`
        public static let trackingDisplay: CGFloat = -0.5
        public static let trackingHeading: CGFloat = -0.2
        public static let trackingCaption: CGFloat = 0.4
    }
}

// MARK: - Spacing & Radii

public extension Theme {
    enum Space {
        public static let xs:   CGFloat = 4
        public static let sm:   CGFloat = 8
        public static let md:   CGFloat = 12
        public static let lg:   CGFloat = 16
        public static let xl:   CGFloat = 24
        public static let xxl:  CGFloat = 32
        public static let xxxl: CGFloat = 48
        public static let huge: CGFloat = 64
    }

    enum Radius {
        public static let sm: CGFloat = 6
        public static let md: CGFloat = 8
        public static let lg: CGFloat = 12
    }

    enum Stroke {
        public static let hairline: CGFloat = 0.5
        public static let thin:     CGFloat = 1
        public static let medium:   CGFloat = 1.5
        public static let thick:    CGFloat = 2
    }
}

// MARK: - Animation

public extension Theme {
    enum Motion {
        /// Quick UI feedback — hover, focus, selection.
        public static let quick      = Animation.easeOut(duration: 0.12)
        /// Standard transitions — drawers, panel reveals.
        public static let standard   = Animation.easeInOut(duration: 0.22)
        /// Smooth content shifts — list reorders, layout changes.
        public static let smooth     = Animation.spring(response: 0.45, dampingFraction: 0.85)
        /// Graph node settle — first-paint physics ease.
        public static let graphLand  = Animation.spring(response: 0.7,  dampingFraction: 0.78)
    }
}

// MARK: - Color helpers

/// Convenience initializer for hex literals (0xRRGGBB).
extension Color {
    init(hex: UInt32, opacity: Double = 1.0) {
        let r = Double((hex >> 16) & 0xFF) / 255.0
        let g = Double((hex >>  8) & 0xFF) / 255.0
        let b = Double( hex        & 0xFF) / 255.0
        self.init(.sRGB, red: r, green: g, blue: b, opacity: opacity)
    }

    /// Build a color that resolves differently in light vs dark appearance.
    /// Uses NSColor's dynamic appearance API under the hood.
    init(light: Color, dark: Color) {
        self = Color(nsColor: NSColor(name: nil, dynamicProvider: { appearance in
            if appearance.bestMatch(from: [.darkAqua, .vibrantDark, .accessibilityHighContrastDarkAqua, .accessibilityHighContrastVibrantDark]) != nil {
                return NSColor(dark)
            }
            return NSColor(light)
        }))
    }
}
