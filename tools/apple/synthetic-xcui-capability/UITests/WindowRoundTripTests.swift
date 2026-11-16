import XCTest

@MainActor
final class WindowRoundTripTests: XCTestCase {
    func testSyntheticWindowAndTwoRealClicks() throws {
        continueAfterFailure = false
        let nonce = UUID().uuidString
        let app = XCUIApplication(bundleIdentifier: "com.peakpassvpn.qa.auto359.capability")
        app.launchEnvironment = ["PPVPN_SYNTHETIC_PROBE_NONCE": nonce]
        app.launch()
        defer { app.terminate() }
        XCTAssertTrue(app.wait(for: .runningForeground, timeout: 20), "WINDOWSERVER_OR_LAUNCH_FAILED")
        let window = app.windows["probe.window"]
        XCTAssertTrue(window.waitForExistence(timeout: 15), "SYNTHETIC_WINDOW_NOT_ACCESSIBLE")
        let nonceLabel = app.staticTexts["probe.nonce"]
        XCTAssertTrue(nonceLabel.exists, "SYNTHETIC_NONCE_NOT_ACCESSIBLE")
        XCTAssertTrue(nonceLabel.label == nonce || nonceLabel.value as? String == nonce,
                      "WRONG_SYNTHETIC_APP_INSTANCE")
        let count = app.staticTexts["probe.count"]
        let button = app.buttons["probe.increment"]
        XCTAssertTrue(button.exists && button.isHittable, "XCUI_INPUT_NOT_AVAILABLE")
        for expected in ["count:1", "count:2"] {
            button.click()
            let predicate = NSPredicate(format: "value == %@ OR label == %@", expected, expected)
            let changed = XCTNSPredicateExpectation(predicate: predicate, object: count)
            XCTAssertEqual(XCTWaiter.wait(for: [changed], timeout: 10), .completed,
                           "XCUI_REAL_CLICK_NOT_READ_BACK")
        }
        let image = XCTAttachment(screenshot: app.screenshot())
        image.name = "synthetic-app-only-after-two-real-clicks"
        image.lifetime = .keepAlways
        add(image)
        let receipt = XCTAttachment(string: "SYNTHETIC_XCUI_ROUNDTRIP_PASS nonce=\(nonce) observed_count=2; no real App/service tested")
        receipt.name = "synthetic-capability-receipt"
        receipt.lifetime = .keepAlways
        add(receipt)
    }
}
