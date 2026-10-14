import PPVPNAppLogic
import PPVPNClient
import XCTest

final class ErrorMessagesTests: LogicTestCase {
    /// macOS says where to open PPVPN from, not the shared "reinstall".
    func testServiceClientRejectedPointsToApplications() {
        XCTAssertEqual(ErrorCode.serviceClientRejected.message, tr("x_serviceRejected"))
        XCTAssertTrue(ErrorCode.serviceClientRejected.message.contains("应用程序"))
    }
}
