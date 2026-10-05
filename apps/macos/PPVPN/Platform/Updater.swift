import Foundation
import Sparkle

/// Sparkle 2 updater. The appcast URL and EdDSA public key are build settings
/// (`PPVPN_UPDATE_FEED_URL`, `PPVPN_SPARKLE_PUBLIC_KEY`); a build without both
/// ships with updates disabled rather than pointing at a guessed endpoint.
@MainActor
final class Updater: NSObject, ObservableObject, SPUUpdaterDelegate {
    static let shared = Updater()

    @Published private(set) var canCheckForUpdates = false
    let isConfigured: Bool

    private let feedURL: String?
    private var controller: SPUStandardUpdaterController!

    private override init() {
        let info = Bundle.main.infoDictionary ?? [:]
        let feed = (info["PPVPNUpdateFeedURL"] as? String)?.trimmingCharacters(in: .whitespaces) ?? ""
        let key = (info["SUPublicEDKey"] as? String)?.trimmingCharacters(in: .whitespaces) ?? ""
        feedURL = feed.isEmpty ? nil : feed
        isConfigured = feedURL != nil && !key.isEmpty
        super.init()
        controller = SPUStandardUpdaterController(startingUpdater: false, updaterDelegate: self, userDriverDelegate: nil)
        guard isConfigured else { return }
        controller.startUpdater()
        controller.updater.publisher(for: \.canCheckForUpdates).assign(to: &$canCheckForUpdates)
    }

    var automaticallyChecks: Bool {
        get { controller.updater.automaticallyChecksForUpdates }
        set {
            objectWillChange.send()
            controller.updater.automaticallyChecksForUpdates = newValue
        }
    }

    func checkForUpdates() {
        controller.checkForUpdates(nil)
    }

    nonisolated func feedURLString(for updater: SPUUpdater) -> String? {
        MainActor.assumeIsolated { feedURL }
    }
}
