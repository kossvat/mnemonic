import AppKit
import SwiftUI

enum AppPalette {
    static let window = color(light: 0xF5F7FA, dark: 0x0D0F12)
    static let sidebar = color(light: 0xEEF1F5, dark: 0x111317)
    static let raised = color(light: 0xFFFFFF, dark: 0x181B20)
    static let border = color(light: 0xD6DCE5, dark: 0x272C34)
    static let textPrimary = color(light: 0x18202C, dark: 0xF3F4F6)
    static let textSecondary = color(light: 0x4D5B6C, dark: 0xA5AFBD)
    static let textMuted = color(light: 0x667384, dark: 0xA5AFBD)
    static let owner = color(light: 0x995000, dark: 0xFFA245)
    static let agents = color(light: 0x1D62AA, dark: 0x6CB4FF)
    static let activeRow = color(light: 0xDCE3EC, dark: 0x20252C)
    static let cardBorder = color(light: 0xCDD5E0, dark: 0x303640)
    static let progressTrack = color(light: 0xDCE3EC, dark: 0x333A45)
    static let stage = color(light: 0xE2EDFA, dark: 0x253246)
    static let badge = color(light: 0xEDF0F4, dark: 0x242A32)
    static let folded = color(light: 0xF0F3F7, dark: 0x14171B)
    static let focusBorder = color(light: 0xB1BECE, dark: 0x465365)
    static let selectedUnit = color(light: 0xE4EDF8, dark: 0x222C39)
    static let selectedUnitBorder = color(light: 0xA2B8D1, dark: 0x45576D)
    static let currentTask = color(light: 0xE5EFFB, dark: 0x202D3E)
    static let currentTaskBorder = color(light: 0x9BB7D8, dark: 0x465C77)
    static let stageComplete = color(light: 0x91ABC9, dark: 0x516985)
    static let stageTrack = color(light: 0xD4DDE8, dark: 0x343C47)
    static let stageOutline = color(light: 0x7B8DA3, dark: 0x687588)
    static let unitProgress = color(light: 0x71849C, dark: 0x8491A3)
    static let timelineGrid = color(light: 0xDCE2EB, dark: 0x2A3039)
    static let timelineClaude = color(light: 0xD4E5F8, dark: 0x26456A)
    static let timelineCodex = color(light: 0xEDF4FC, dark: 0x182B42)
    static let timelineFocus = color(light: 0xC1D8F1, dark: 0x40566F)
    static let timelineTrackBorder = color(light: 0xA9B7C8, dark: 0x414B58)

    private static func color(light: UInt32, dark: UInt32) -> Color {
        Color(nsColor: NSColor(name: nil, dynamicProvider: { appearance in
            let value = appearance.bestMatch(from: [.aqua, .darkAqua]) == .darkAqua ? dark : light
            return NSColor(srgbRed: Double((value >> 16) & 0xFF) / 255,
                           green: Double((value >> 8) & 0xFF) / 255,
                           blue: Double(value & 0xFF) / 255, alpha: 1)
        }))
    }
}
