import AppKit
import PPVPNAppLogic
import PPVPNClient
import SwiftUI

// MARK: - Tri-state switch

/// 32×18 switch with an intermediate state (knob centred, spinner inside,
/// soft accent track with an accent hairline) for applying / preparing /
/// authorising / connecting / reconnecting / disconnecting. No state text
/// beside it: the card header carries the state.
struct TriStateSwitch: View {
    typealias Value = SwitchState

    let value: Value
    var isEnabled = true
    let action: () -> Void

    var body: some View {
        Button(action: action) {
            ZStack(alignment: knobAlignment) {
                Capsule()
                    .fill(trackFill)
                    .overlay(Capsule().strokeBorder(trackStroke, lineWidth: 1))
                Circle()
                    .fill(Color.white)
                    .shadow(color: .black.opacity(0.25), radius: 0.5, y: 0.5)
                    .frame(width: 14, height: 14)
                    .overlay {
                        if value == .pending {
                            ProgressView().controlSize(.mini).scaleEffect(0.55)
                        }
                    }
                    .padding(2)
            }
            .frame(width: 32, height: 18)
            .animation(.easeInOut(duration: 0.15), value: value)
        }
        .buttonStyle(.plain)
        .disabled(!isEnabled)
        .opacity(isEnabled ? 1 : 0.55)
        .accessibilityValue(Text(value == .on ? tr("on") : value == .off ? tr("off") : tr("st_connecting")))
    }

    private var knobAlignment: Alignment {
        switch value {
        case .off: .leading
        case .on: .trailing
        case .pending: .center
        }
    }

    private var trackFill: Color {
        switch value {
        case .off: Color.primary.opacity(0.16)
        case .on: .accentColor
        case .pending: Brand.accentSoft
        }
    }

    private var trackStroke: Color {
        switch value {
        case .off: Color.primary.opacity(0.08)
        case .on: .clear
        case .pending: .accentColor
        }
    }
}

// MARK: - Copy button

/// "复制" that turns into "✓ 已复制" for 1.6 s after copying.
struct CopyButton: View {
    let value: String
    var showsTitle = true
    @State private var copied = false

    var body: some View {
        Button {
            NSPasteboard.general.clearContents()
            NSPasteboard.general.setString(value, forType: .string)
            copied = true
            Task {
                try? await Task.sleep(for: .seconds(1.6))
                copied = false
            }
        } label: {
            if showsTitle {
                Label(copied ? tr("copied") : tr("copy"), systemImage: copied ? "checkmark" : "doc.on.doc")
            } else {
                Image(systemName: copied ? "checkmark" : "doc.on.doc")
            }
        }
        .buttonStyle(.borderless)
        .foregroundStyle(copied ? Brand.success : Color.accentColor)
        .help(tr("copy"))
    }
}

// MARK: - Unread badge

/// 0 hidden · 1–9 circle · 10–99 capsule · ">99" as 99+. Danger fill with a
/// ring in the window colour so it reads on top of the bell.
struct CountBadge: View {
    let count: UInt32

    var body: some View {
        if count > 0 {
            Text(count > 99 ? "99+" : String(count))
                .font(.system(size: 11, weight: .bold).monospacedDigit())
                .foregroundStyle(Brand.badgeText)
                .padding(.horizontal, count > 9 ? 5 : 0)
                .frame(minWidth: 18, minHeight: 18)
                .background(Capsule().fill(Brand.badge))
                .overlay(Capsule().strokeBorder(Color(nsColor: .windowBackgroundColor), lineWidth: 1.5))
                .fixedSize()
        }
    }
}

// MARK: - Card

/// Grouped surface used by the overview sections (radius 10, hairline).
struct Card<Content: View>: View {
    @ViewBuilder var content: Content

    var body: some View {
        VStack(alignment: .leading, spacing: 0) { content }
            .background(RoundedRectangle(cornerRadius: Metrics.cardRadius).fill(Brand.card))
            .overlay(RoundedRectangle(cornerRadius: Metrics.cardRadius).strokeBorder(Brand.cardStroke, lineWidth: 0.5))
    }
}

/// A card row: title + optional one-line weakest description, trailing accessory.
struct CardRow<Accessory: View>: View {
    let title: String
    var detail: String?
    @ViewBuilder var accessory: Accessory

    var body: some View {
        HStack(alignment: .center, spacing: 12) {
            VStack(alignment: .leading, spacing: 2) {
                Text(title)
                if let detail {
                    Text(detail)
                        .font(.system(size: 11))
                        .foregroundStyle(.tertiary)
                        .lineLimit(1)
                }
            }
            Spacer(minLength: 8)
            accessory
        }
        .frame(minHeight: Metrics.rowMinHeight)
        .padding(.horizontal, 14)
        .padding(.vertical, 9)
    }
}

/// Section title with the weakest-level note on the same line.
struct SectionHeader: View {
    let title: String
    var note: String?

    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Text(title).font(.system(size: 13, weight: .semibold))
            if let note {
                Text(note).font(.system(size: 11)).foregroundStyle(.tertiary).lineLimit(1)
            }
        }
        .padding(.leading, 2)
    }
}

// MARK: - Empty state

/// Blocking persistent state: icon + title + one sentence + actions.
struct EmptyStateView<Actions: View>: View {
    let systemImage: String
    var tint: Color = .secondary
    let title: String
    let message: String
    var busy = false
    @ViewBuilder var actions: Actions

    var body: some View {
        VStack(spacing: 10) {
            ZStack {
                Circle().fill(tint == .secondary ? Color.primary.opacity(0.06) : tint.opacity(0.14))
                if busy {
                    ProgressView().controlSize(.regular)
                } else {
                    Image(systemName: systemImage)
                        .font(.system(size: 24, weight: .medium))
                        .foregroundStyle(tint)
                }
            }
            .frame(width: 56, height: 56)
            .padding(.bottom, 4)
            Text(title).font(.system(size: 15, weight: .semibold))
            Text(message)
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true)
            HStack(spacing: 8) { actions }
                .padding(.top, 8)
        }
        .padding(32)
        .frame(maxWidth: .infinity)
    }
}

// MARK: - Flag

/// 4:3 flag image (flag-icons, MIT) with a hairline so white flags don't melt
/// into the background; a globe when the country is unknown.
struct FlagView: View {
    let countryCode: String?
    var height: CGFloat = 12

    var body: some View {
        Group {
            if let code = countryCode?.lowercased(), NSImage(named: "Flags/\(code)") != nil {
                Image("Flags/\(code)")
                    .resizable()
                    .interpolation(.high)
            } else {
                Image(systemName: "globe")
                    .resizable()
                    .scaledToFit()
                    .foregroundStyle(.secondary)
                    .padding(1)
            }
        }
        .frame(width: height * 4 / 3, height: height)
        .clipShape(RoundedRectangle(cornerRadius: 2))
        .overlay(RoundedRectangle(cornerRadius: 2).strokeBorder(Color.primary.opacity(0.15), lineWidth: 0.5))
    }
}

/// Flag as an NSImage sized for menus and pop-up buttons, which draw images
/// at their natural size.
@MainActor func flagMenuImage(_ countryCode: String?, height: CGFloat = 12) -> NSImage {
    let name = "Flags/\((countryCode ?? "").lowercased())"
    guard let image = NSImage(named: name)?.copy() as? NSImage else {
        return NSImage(systemSymbolName: "globe", accessibilityDescription: nil) ?? NSImage()
    }
    image.size = NSSize(width: height * 4 / 3, height: height)
    return image
}

// MARK: - Latency

/// Spinner while testing; green < 100, amber 100–199, red ≥ 200 ms;
/// timeout in secondary with a clock; failure in danger with an error glyph.
struct LatencyLabel: View {
    let outcome: ProbeOutcome?

    var body: some View {
        switch outcome {
        case nil:
            Text("—").foregroundStyle(.tertiary)
        case .running:
            ProgressView().controlSize(.small)
        case .latency(let ms):
            Text("\(ms) ms")
                .monospacedDigit()
                .foregroundStyle(ms < 100 ? Brand.success : ms < 200 ? Brand.warning : Brand.danger)
        case .failed(.timeout):
            Label(tr("timeout"), systemImage: "clock").foregroundStyle(.secondary)
        case .failed(let code):
            Label(tr("failed"), systemImage: "exclamationmark.circle.fill")
                .foregroundStyle(Brand.danger)
                .help(code.message)
        }
    }
}

// MARK: - Spinning icon

/// Tone icon; while a transition is in progress it shows the system spinner
/// (a native NSProgressIndicator: a SwiftUI repeatForever rotation kept the
/// main thread around 20% busy).
struct StatusGlyph: View {
    let systemImage: String
    let tone: Tone
    var size: CGFloat = 44

    var body: some View {
        ZStack {
            Circle().fill(tone.soft)
            if tone == .busy {
                ProgressView()
                    .controlSize(.small)
                    .tint(tone.color)
            } else {
                Image(systemName: systemImage)
                    .font(.system(size: size * 0.42, weight: .semibold))
                    .foregroundStyle(tone.color)
            }
        }
        .frame(width: size, height: size)
    }
}

func copyToPasteboard(_ value: String) {
    NSPasteboard.general.clearContents()
    NSPasteboard.general.setString(value, forType: .string)
}

/// A local proxy's user name and password, each with a copy button. The
/// password is masked (without revealing its length) until the eye toggle
/// shows it, and masked again whenever the proxy changes; copying always
/// copies the full values. Used by the overview card and the nodes page.
struct ProxyCredentials<Row: View>: View {
    let proxy: LocalProxy
    /// Lays out one line: label, value (with its full text as help), trailing buttons.
    let row: (_ label: String, _ value: String, _ trailing: AnyView) -> Row
    @State private var passwordRevealed = false

    var body: some View {
        Group {
            row(tr("user"), proxy.username, AnyView(CopyButton(value: proxy.username, showsTitle: false)))
            row(tr("authPass"), passwordRevealed ? proxy.password : "••••••", AnyView(passwordButtons))
        }
        .onChange(of: proxy) { _ in passwordRevealed = false }
    }

    private var passwordButtons: some View {
        HStack(spacing: 12) {
            Button {
                passwordRevealed.toggle()
            } label: {
                Image(systemName: passwordRevealed ? "eye.slash" : "eye")
            }
            .buttonStyle(.borderless)
            .foregroundStyle(Color.accentColor)
            .help(passwordRevealed ? tr("hidePassword") : tr("showPassword"))
            .accessibilityLabel(passwordRevealed ? tr("hidePassword") : tr("showPassword"))
            CopyButton(value: proxy.password, showsTitle: false)
        }
    }
}
