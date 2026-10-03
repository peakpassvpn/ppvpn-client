import AppKit
import PPVPNAppLogic
import SwiftUI

// Brand values from the design handoff. Everything else (surfaces, text,
// controls, fonts) comes from the system so the app looks native in both
// appearances; only the accent, the semantic tones and the app icon are ours.

enum Brand {
    static let success = Color("Success")
    static let warning = Color("Warning")
    static let danger = Color("Danger")
    static let badge = Color("Badge")
    static let badgeText = Color("BadgeText")
    static let onAccent = Color("OnAccent")
    /// accent-soft: intermediate switch track, install hint, tier tags.
    static let accentSoft = Color(nsColor: NSColor(name: nil) { appearance in
        appearance.isDark
            ? NSColor(srgbRed: 180 / 255, green: 197 / 255, blue: 1, alpha: 0.16)
            : NSColor(srgbRed: 31 / 255, green: 67 / 255, blue: 144 / 255, alpha: 0.12)
    })
    /// Grouped card surface: a white sheet on the grey Aqua window, a faint
    /// shade where the window itself is white (macOS 26), a lift in Dark Aqua.
    static let card = Color(nsColor: NSColor(name: nil) { appearance in
        if appearance.isDark { return NSColor.white.withAlphaComponent(0.05) }
        var brightness: CGFloat = 0
        appearance.performAsCurrentDrawingAppearance {
            brightness = NSColor.windowBackgroundColor.usingColorSpace(.sRGB)?.brightnessComponent ?? 0
        }
        return brightness > 0.98 ? NSColor.black.withAlphaComponent(0.035) : .controlBackgroundColor
    })
    static let cardStroke = Color(nsColor: .separatorColor).opacity(0.6)
}

extension NSAppearance {
    var isDark: Bool { bestMatch(from: [.aqua, .darkAqua]) == .darkAqua }
}

/// Colours of the shared `Tone`.
extension Tone {
    var color: Color {
        switch self {
        case .ok: Brand.success
        case .busy: .accentColor
        case .warn: Brand.warning
        case .err: Brand.danger
        case .idle: .secondary
        }
    }

    var soft: Color {
        switch self {
        case .idle: Color.primary.opacity(0.06)
        case .busy: Brand.accentSoft
        default: color.opacity(0.14)
        }
    }
}

enum Metrics {
    static let contentPadding = EdgeInsets(top: 20, leading: 24, bottom: 20, trailing: 24)
    static let sectionSpacing: CGFloat = 18
    static let cardRadius: CGFloat = 10
    static let rowMinHeight: CGFloat = 28
}
