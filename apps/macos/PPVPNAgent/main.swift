import AppKit

// A UI-less (LSUIElement) app rather than a bare tool: notifications need an
// app bundle, and its delegate receives the clicks.
MainActor.assumeIsolated {
    let app = NSApplication.shared
    let agent = Agent()
    app.delegate = agent
    app.setActivationPolicy(.accessory)
    app.run()
}
