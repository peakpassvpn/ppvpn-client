import Foundation
@testable import PPVPNAppLogic
import XCTest

final class AboutNoticeTests: LogicTestCase {
    func testLinksOpenTheSourceThenTheLicense() {
        XCTAssertEqual(AboutNotice.sourceURL.absoluteString, "https://github.com/peakpassvpn/ppvpn-client")
        XCTAssertEqual(AboutNotice.licenseURL.absoluteString, "https://www.gnu.org/licenses/gpl-3.0.html")
        XCTAssertEqual(AboutNotice.links.map(\.url), [AboutNotice.sourceURL, AboutNotice.licenseURL])
        XCTAssertEqual(AboutNotice.links.map(\.title), [tr("aboutSource"), tr("aboutViewLicense")])
    }

    func testNoticeIsTheSharedText() {
        XCTAssertEqual(AboutNotice.notice, tr("aboutLicense"))
        XCTAssertNotEqual(AboutNotice.notice, "aboutLicense", "the key resolves")
        XCTAssertTrue(AboutNotice.notice.contains("GNU"))
    }

    /// The bundles' NSHumanReadableCopyright (About panel) says the same.
    func testCopyrightMatchesTheBundles() throws {
        XCTAssertEqual(AboutNotice.copyright, "© 2026 PeakPass Labs LLC")
        let project = try String(contentsOf: Repo.sources.appendingPathComponent("project.yml"), encoding: .utf8)
        let lines = project.split(separator: "\n").filter { $0.contains("INFOPLIST_KEY_NSHumanReadableCopyright") }
        XCTAssertEqual(lines.count, 2, "the app and the push agent")
        for line in lines {
            XCTAssertTrue(line.hasSuffix("\"\(AboutNotice.copyright)\""), String(line))
        }
    }
}
