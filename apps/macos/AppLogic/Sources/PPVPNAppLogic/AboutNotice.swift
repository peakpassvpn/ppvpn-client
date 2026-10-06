import Foundation

/// The GPL §5 notice the About panel and Settings › General show: the
/// license statement, where the source is, the license text and the copyright
/// line (the same as App.Core's SettingsViewModel on Windows and Linux).
public enum AboutNotice {
    /// `aboutSource`: where the GPL source is published.
    public static let sourceURL = URL(string: "https://github.com/peakpassvpn/ppvpn-client")!
    /// `aboutViewLicense`: the GNU GPL version 3.
    public static let licenseURL = URL(string: "https://www.gnu.org/licenses/gpl-3.0.html")!
    /// The copyright line, the same in every language (also the bundles'
    /// NSHumanReadableCopyright).
    public static let copyright = "© 2026 PeakPass VPN LLC"

    /// `aboutLicense`: free software under the GPL, no warranty.
    public static var notice: String { tr("aboutLicense") }

    public struct Link: Identifiable, Equatable, Sendable {
        public let title: String
        public let url: URL
        public var id: URL { url }
    }

    /// Source code, then the license.
    public static var links: [Link] {
        [Link(title: tr("aboutSource"), url: sourceURL), Link(title: tr("aboutViewLicense"), url: licenseURL)]
    }
}
