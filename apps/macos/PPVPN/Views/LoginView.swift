import PPVPNAppLogic
import PPVPNClient
import SwiftUI

/// Sign-in: not signed in · waiting for the browser (device code, countdown) ·
/// error (code expired / denied in the browser / network).
struct LoginView: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Group {
            if case .awaitingBrowser(let code) = model.snapshot.auth {
                TimelineView(.periodic(from: .now, by: 1)) { context in
                    if let expiresAt = model.deviceCodeExpiresAt, expiresAt <= context.date {
                        LoginError(kind: .expired)
                    } else {
                        Waiting(code: code, expiresAt: model.deviceCodeExpiresAt, now: context.date)
                    }
                }
            } else if let kind = LoginError.Kind(model.snapshot.lastError?.code) {
                LoginError(kind: kind)
            } else {
                SignedOut()
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .padding(32)
    }
}

private struct AppIcon: View {
    let size: CGFloat

    var body: some View {
        Image(nsImage: NSApp.applicationIconImage)
            .resizable()
            .frame(width: size, height: size)
    }
}

private struct SignedOut: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(spacing: 0) {
            AppIcon(size: 76 * 1.22).padding(.bottom, 12)
            Text(tr("loginTitle")).font(.system(size: 20, weight: .bold))
            Text(tr("loginSub"))
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .padding(.top, 6)
            Button {
                model.signIn()
            } label: {
                Label(tr("loginBtn"), systemImage: "arrow.up.forward.square")
                    .padding(.horizontal, 6)
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .keyboardShortcut(.defaultAction)
            .padding(.top, 22)
        }
    }
}

private struct Waiting: View {
    @EnvironmentObject private var model: AppModel
    let code: DeviceCode
    let expiresAt: Date?
    let now: Date

    var body: some View {
        VStack(spacing: 0) {
            AppIcon(size: 52 * 1.22).padding(.bottom, 12)
            Text(tr("waitTitle")).font(.system(size: 17, weight: .semibold))
            Text(tr("waitSub"))
                .font(.callout)
                .foregroundStyle(.secondary)
                .multilineTextAlignment(.center)
                .padding(.top, 6)

            HStack(spacing: 14) {
                Text(code.userCode)
                    .font(.system(size: 38, weight: .semibold, design: .monospaced))
                    .tracking(3.8)
                    .textSelection(.enabled)
                CopyButton(value: code.userCode, showsTitle: false)
                    .font(.system(size: 15))
            }
            .padding(.horizontal, 24)
            .padding(.vertical, 18)
            .background(RoundedRectangle(cornerRadius: Metrics.cardRadius).fill(Brand.card))
            .overlay(RoundedRectangle(cornerRadius: Metrics.cardRadius).strokeBorder(Brand.cardStroke, lineWidth: 0.5))
            .padding(.top, 20)

            if let expiresAt {
                Label(tr("expiresIn", ["t": remaining(until: expiresAt)]), systemImage: "timer")
                    .font(.callout)
                    .monospacedDigit()
                    .foregroundStyle(.secondary)
                    .padding(.top, 12)
            }

            HStack(spacing: 10) {
                Button(tr("cancel")) { model.cancelSignIn() }
                    .keyboardShortcut(.cancelAction)
                Button(tr("reopen")) {
                    if let url = URL(string: code.verificationUrl) { NSWorkspace.shared.open(url) }
                }
                .buttonStyle(.borderedProminent)
                .keyboardShortcut(.defaultAction)
            }
            .padding(.top, 20)
        }
    }

    private func remaining(until expiresAt: Date) -> String {
        let seconds = max(0, Int(expiresAt.timeIntervalSince(now).rounded(.up)))
        return String(format: "%d:%02d", seconds / 60, seconds % 60)
    }
}

private struct LoginError: View {
    enum Kind {
        case expired, denied, network

        init?(_ code: ErrorCode?) {
            switch code {
            case .authExpired?: self = .expired
            case .authDenied?: self = .denied
            case .networkUnreachable?, .serverUnavailable?: self = .network
            default: return nil
            }
        }
    }

    @EnvironmentObject private var model: AppModel
    let kind: Kind

    var body: some View {
        let (title, message, image) = switch kind {
        case .expired: (tr("errExpiredT"), tr("errExpiredD"), "timer")
        case .denied: (tr("errDeniedT"), tr("errDeniedD"), "hand.raised.slash")
        case .network: (tr("errNetT"), tr("errNetD"), "wifi.slash")
        }
        EmptyStateView(systemImage: image, tint: Brand.danger, title: title, message: message) {
            Button(kind == .network ? tr("tryAgain") : tr("signInAgain")) {
                model.signIn()
            }
            .buttonStyle(.borderedProminent)
            .controlSize(.large)
            .keyboardShortcut(.defaultAction)
        }
        .frame(maxWidth: 420)
    }
}
