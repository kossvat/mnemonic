import SwiftUI
import MnemonicShared

/// Shared component primitives that enforce the design system. Use these
/// instead of building cards/chips/empty-states ad-hoc in views.

// MARK: - Card

/// A surface card. Subtle background contrast + 1pt hairline border, no shadow.
public struct Card<Content: View>: View {
    let padding: CGFloat
    let content: Content

    public init(padding: CGFloat = Theme.Space.lg, @ViewBuilder content: () -> Content) {
        self.padding = padding
        self.content = content()
    }

    public var body: some View {
        content
            .padding(padding)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.md, style: .continuous)
                    .fill(Theme.Palette.bgSurface)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.md, style: .continuous)
                    .stroke(Theme.Palette.border, lineWidth: Theme.Stroke.thin)
            )
    }
}

// MARK: - KPI Card

public struct KPICard: View {
    let icon: String
    let value: String
    let label: String
    var trailing: String? = nil

    public init(icon: String, value: String, label: String, trailing: String? = nil) {
        self.icon = icon
        self.value = value
        self.label = label
        self.trailing = trailing
    }

    public var body: some View {
        Card {
            VStack(alignment: .leading, spacing: Theme.Space.md) {
                HStack(spacing: Theme.Space.sm) {
                    Image(systemName: icon)
                        .font(.system(size: 13, weight: .regular))
                        .foregroundStyle(Theme.Palette.textSubtle)
                    if let trailing {
                        Spacer()
                        Text(trailing)
                            .font(Theme.Font.caption)
                            .foregroundStyle(Theme.Palette.textSubtle)
                    }
                }
                Text(value)
                    .font(Theme.Font.display)
                    .tracking(Theme.Font.trackingDisplay)
                    .foregroundStyle(Theme.Palette.textPrimary)
                    .lineLimit(1)
                    .minimumScaleFactor(0.6)
                Text(label.uppercased())
                    .font(Theme.Font.caption)
                    .tracking(Theme.Font.trackingCaption)
                    .foregroundStyle(Theme.Palette.textMuted)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
        }
    }
}

// MARK: - Type Chip (memory_type)

/// Small pill showing a memory's type. Tinted by type color at 10% opacity
/// with a 15%-opacity stroke — never a solid bright pill.
public struct TypeChip: View {
    let type: String

    public init(type: String) { self.type = type }

    public var body: some View {
        let color = MemoryTypeIcon.color(for: type)
        let icon  = MemoryTypeIcon.icon(for: type)
        let label = type.replacingOccurrences(of: "_", with: " ").capitalized

        HStack(spacing: 4) {
            Image(systemName: icon)
                .font(.system(size: 9, weight: .semibold))
            Text(label)
                .font(Theme.Font.captionBold)
        }
        .padding(.horizontal, 6)
        .padding(.vertical, 2)
        .foregroundStyle(color)
        .background(
            RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                .fill(color.opacity(0.10))
        )
        .overlay(
            RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                .stroke(color.opacity(0.15), lineWidth: Theme.Stroke.hairline)
        )
    }
}

// MARK: - Alias Chip

/// A subtle gray pill for tags, aliases, and other secondary metadata.
public struct AliasChip: View {
    let text: String

    public init(text: String) { self.text = text }

    public var body: some View {
        Text(text)
            .font(Theme.Font.caption)
            .foregroundStyle(Theme.Palette.textMuted)
            .padding(.horizontal, Theme.Space.sm)
            .padding(.vertical, 2)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .fill(Theme.Palette.bgTint)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .stroke(Theme.Palette.border, lineWidth: Theme.Stroke.hairline)
            )
    }
}

// MARK: - Empty State

public struct EmptyStateView: View {
    let icon: String
    let title: String
    let subtitle: String
    var suggestions: [String] = []
    var onSuggestion: ((String) -> Void)? = nil

    public init(
        icon: String,
        title: String,
        subtitle: String,
        suggestions: [String] = [],
        onSuggestion: ((String) -> Void)? = nil
    ) {
        self.icon = icon
        self.title = title
        self.subtitle = subtitle
        self.suggestions = suggestions
        self.onSuggestion = onSuggestion
    }

    public var body: some View {
        VStack(spacing: Theme.Space.lg) {
            Image(systemName: icon)
                .font(.system(size: 40, weight: .light))
                .foregroundStyle(Theme.Palette.textSubtle)
            VStack(spacing: Theme.Space.xs) {
                Text(title)
                    .font(Theme.Font.heading)
                    .tracking(Theme.Font.trackingHeading)
                    .foregroundStyle(Theme.Palette.textMuted)
                Text(subtitle)
                    .font(Theme.Font.body)
                    .foregroundStyle(Theme.Palette.textSubtle)
                    .multilineTextAlignment(.center)
                    .frame(maxWidth: 360)
            }
            if !suggestions.isEmpty {
                HStack(spacing: Theme.Space.sm) {
                    ForEach(suggestions, id: \.self) { s in
                        Button {
                            onSuggestion?(s)
                        } label: {
                            Text(s)
                                .font(Theme.Font.caption)
                                .foregroundStyle(Theme.Palette.textMuted)
                                .padding(.horizontal, Theme.Space.md)
                                .padding(.vertical, 6)
                                .background(
                                    RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                                        .fill(Theme.Palette.bgTint)
                                )
                                .overlay(
                                    RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                                        .stroke(Theme.Palette.border, lineWidth: Theme.Stroke.hairline)
                                )
                        }
                        .buttonStyle(.plain)
                    }
                }
            }
        }
        .padding(Theme.Space.xxxl)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
    }
}

// MARK: - Section Label

/// Small uppercase tracker for grouping labels in sidebars / list section headers.
public struct SectionLabel: View {
    let text: String

    public init(_ text: String) { self.text = text }

    public var body: some View {
        Text(text.uppercased())
            .font(Theme.Font.caption)
            .tracking(Theme.Font.trackingCaption)
            .foregroundStyle(Theme.Palette.textSubtle)
    }
}

// MARK: - Subtle Button Style

public struct SubtleButtonStyle: ButtonStyle {
    public init() {}
    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.Font.bodyMedium)
            .foregroundStyle(Theme.Palette.textPrimary)
            .padding(.horizontal, Theme.Space.md)
            .padding(.vertical, Theme.Space.sm)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .fill(configuration.isPressed ? Theme.Palette.bgTint : Theme.Palette.bgSurface)
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .stroke(Theme.Palette.border, lineWidth: Theme.Stroke.thin)
            )
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(Theme.Motion.quick, value: configuration.isPressed)
    }
}

/// Destructive variant — only used for forget/cleanup/dedupe-apply etc.
public struct DangerButtonStyle: ButtonStyle {
    public init() {}
    public func makeBody(configuration: Configuration) -> some View {
        configuration.label
            .font(Theme.Font.bodyMedium)
            .foregroundStyle(Theme.Palette.feedback)
            .padding(.horizontal, Theme.Space.md)
            .padding(.vertical, Theme.Space.sm)
            .background(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .fill(configuration.isPressed
                          ? Theme.Palette.feedback.opacity(0.15)
                          : Theme.Palette.feedback.opacity(0.08))
            )
            .overlay(
                RoundedRectangle(cornerRadius: Theme.Radius.sm, style: .continuous)
                    .stroke(Theme.Palette.feedback.opacity(0.25), lineWidth: Theme.Stroke.thin)
            )
            .scaleEffect(configuration.isPressed ? 0.98 : 1)
            .animation(Theme.Motion.quick, value: configuration.isPressed)
    }
}

// MARK: - Entity type icon mapping

public enum EntityIcon {
    public static func symbol(for type: String) -> String {
        switch type.lowercased() {
        case "project":   return "square.stack.3d.up"
        case "tech":      return "chevron.left.forwardslash.chevron.right"
        case "person":    return "person.crop.circle"
        case "module":    return "cube"
        case "file":      return "doc.text"
        case "concept":   return "lightbulb"
        default:          return "circle.dotted"
        }
    }

    public static func color(for type: String) -> Color {
        switch type.lowercased() {
        case "project":   return Theme.Palette.entityProject
        case "tech":      return Theme.Palette.entityTech
        case "person":    return Theme.Palette.entityPerson
        case "module":    return Theme.Palette.entityModule
        case "file":      return Theme.Palette.entityFile
        case "concept":   return Theme.Palette.entityConcept
        default:          return Theme.Palette.textMuted
        }
    }
}

public enum MemoryTypeIcon {
    public static func color(for type: String) -> Color {
        switch type.lowercased() {
        case "decision":         return Theme.Palette.decision
        case "feedback":         return Theme.Palette.feedback
        case "security":         return Theme.Palette.security
        case "session_summary":  return Theme.Palette.session
        default:                 return Theme.Palette.note
        }
    }

    public static func icon(for type: String) -> String {
        switch type.lowercased() {
        case "decision":         return "checkmark.seal.fill"
        case "feedback":         return "exclamationmark.bubble.fill"
        case "security":         return "lock.fill"
        case "session_summary":  return "calendar"
        default:                 return "doc.text"
        }
    }
}
