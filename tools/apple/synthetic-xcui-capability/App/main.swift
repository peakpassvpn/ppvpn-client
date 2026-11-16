import AppKit

final class ProbeDelegate: NSObject, NSApplicationDelegate {
    private var window: NSWindow?
    private var count = 0
    private let countLabel = NSTextField(labelWithString: "count:0")

    func applicationDidFinishLaunching(_ notification: Notification) {
        guard let nonce = ProcessInfo.processInfo.environment["PPVPN_SYNTHETIC_PROBE_NONCE"],
              UUID(uuidString: nonce) != nil else {
            NSApp.terminate(nil)
            return
        }
        let window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 640, height: 280),
                              styleMask: [.titled, .closable], backing: .buffered, defer: false)
        window.title = "Auto359 synthetic UI capability only"
        window.setAccessibilityIdentifier("probe.window")
        let nonceLabel = NSTextField(labelWithString: nonce)
        nonceLabel.setAccessibilityIdentifier("probe.nonce")
        countLabel.setAccessibilityIdentifier("probe.count")
        let button = NSButton(title: "Increment synthetic counter", target: self, action: #selector(increment))
        button.setAccessibilityIdentifier("probe.increment")
        let stack = NSStackView(views: [
            NSTextField(labelWithString: "Synthetic fixture — no PPVPN Client, service or network"),
            nonceLabel, countLabel, button,
        ])
        stack.orientation = .vertical
        stack.spacing = 16
        stack.translatesAutoresizingMaskIntoConstraints = false
        let content = NSView()
        content.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerXAnchor.constraint(equalTo: content.centerXAnchor),
            stack.centerYAnchor.constraint(equalTo: content.centerYAnchor),
        ])
        window.contentView = content
        self.window = window
        window.center()
        window.makeKeyAndOrderFront(nil)
        NSApp.activate(ignoringOtherApps: true)
    }

    @objc private func increment() {
        count += 1
        countLabel.stringValue = "count:\(count)"
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { true }
}

let app = NSApplication.shared
app.setActivationPolicy(.regular)
let delegate = ProbeDelegate()
app.delegate = delegate
withExtendedLifetime(delegate) { app.run() }
