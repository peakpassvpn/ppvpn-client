import PPVPNAppLogic
import XCTest

final class AppLocationTests: XCTestCase {
    private func decide(_ path: String, original: String? = nil, readOnly: String? = nil) -> AppLocation.Decision {
        AppLocation.decide(bundlePath: path, originalPath: original, readOnlyVolume: readOnly)
    }

    func testTheServicesPathsStay() {
        XCTAssertEqual(decide("/Applications/PPVPN.app"), .stay)
        XCTAssertEqual(decide("/Applications/PPVPN.app/"), .stay)
        XCTAssertEqual(decide("/Applications/PPVPN（开发版）.app"), .stay)
    }

    func testTheMountedDiskImageIsCopiedAndEjected() {
        XCTAssertEqual(decide("/Volumes/PPVPN/PPVPN.app", readOnly: "/Volumes/PPVPN"),
                       .move(source: "/Volumes/PPVPN/PPVPN.app", removeSource: false, volume: "/Volumes/PPVPN"))
    }

    func testAWritableCopyIsMovedAndItsSourceRemoved() {
        for path in ["/Volumes/Work/Downloads/PPVPN.app", "/Volumes/Work/Applications/PPVPN.app",
                     "/Applications/Utilities/PPVPN.app", "/Applications/PPVPN 2.app"] {
            XCTAssertEqual(decide(path), .move(source: path, removeSource: true, volume: nil), path)
        }
    }

    func testATranslocatedCopyMovesFromWhereItWasOpened() {
        let translocated = "/var/folders/x/T/AppTranslocation/0000-1111/d/PPVPN.app"
        XCTAssertTrue(AppLocation.isTranslocated(translocated))
        XCTAssertFalse(AppLocation.isTranslocated("/Applications/PPVPN.app"))
        XCTAssertEqual(decide(translocated, original: "/Volumes/Work/Downloads/PPVPN.app"),
                       .move(source: "/Volumes/Work/Downloads/PPVPN.app", removeSource: true, volume: nil))
        XCTAssertEqual(decide(translocated, original: "/Volumes/PPVPN/PPVPN.app", readOnly: "/Volumes/PPVPN"),
                       .move(source: "/Volumes/PPVPN/PPVPN.app", removeSource: false, volume: "/Volumes/PPVPN"))
        // Without the original path the running copy is the source.
        XCTAssertEqual(decide(translocated, readOnly: "/var/folders/x/T/AppTranslocation/0000-1111"),
                       .move(source: translocated, removeSource: false,
                             volume: "/var/folders/x/T/AppTranslocation/0000-1111"))
    }

    func testTranslocatedFromApplicationsStays() {
        // Moving cannot help, and opening it again would only loop.
        XCTAssertEqual(decide("/var/folders/x/T/AppTranslocation/0000/d/PPVPN.app",
                              original: "/Applications/PPVPN.app"), .stay)
    }
}
