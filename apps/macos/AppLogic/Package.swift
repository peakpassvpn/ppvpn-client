// swift-tools-version:5.10
// What the macOS push agent (PPVPNAgent) shares beyond the client bindings:
// its log and the app's directories. No AppKit, so it also builds and tests
// on Linux, with the bindings (scripts/build-swift-linux.sh).
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
