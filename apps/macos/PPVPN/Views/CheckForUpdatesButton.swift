import PPVPNAppLogic
import SwiftUI

/// "Check for Updates…" for the app menu and the menu bar extra; absent in
/// builds without an update feed.
struct CheckForUpdatesButton: View {
    @ObservedObject private var updater = Updater.shared

    var body: some View {
        Button(tr("checkUpdates")) { updater.checkForUpdates() }
            .disabled(!updater.isConfigured || !updater.canCheckForUpdates)
    }
}
