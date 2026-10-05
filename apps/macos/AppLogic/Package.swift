// swift-tools-version:5.10
// App logic shared by the macOS app and its tests: UI state, presentation and
// the client backend seam, with no SwiftUI, and AppKit / Network only behind
// `#if canImport(...)` (the Logs text renderer, so its performance is
// tested; NWPathMonitor), so it also builds and tests on Linux.
import PackageDescription

let package = Package(
    name: "PPVPNAppLogic",
    platforms: [.macOS(.v13)],
    products: [.library(name: "PPVPNAppLogic", targets: ["PPVPNAppLogic"])],
    dependencies: [.package(path: "../PPVPNClient")],
    targets: [
        .target(
            name: "PPVPNAppLogic",
            dependencies: [.product(name: "PPVPNClient", package: "PPVPNClient")],
            swiftSettings: [.enableExperimentalFeature("StrictConcurrency")]
        ),
        .testTarget(
            name: "PPVPNAppLogicTests",
            dependencies: ["PPVPNAppLogic"]
        ),
    ]
)
