import AppKit
import PPVPNAppLogic

/// 关于 PPVPN: the standard About panel (name, version, the bundle's
/// NSHumanReadableCopyright) with the GPL notice and its source and license
/// links as the credits.
enum AboutPanel {
    @MainActor
    static func show() {
        NSApp.activate(ignoringOtherApps: true)
        NSApp.orderFrontStandardAboutPanel(options: [.credits: credits()])
    }

    @MainActor
    static func credits() -> NSAttributedString {
        let paragraph = NSMutableParagraphStyle()
        paragraph.alignment = .center
        let plain: [NSAttributedString.Key: Any] = [
            .font: NSFont.systemFont(ofSize: NSFont.smallSystemFontSize),
            .foregroundColor: NSColor.secondaryLabelColor,
            .paragraphStyle: paragraph,
        ]
        let text = NSMutableAttributedString(string: AboutNotice.notice + "\n\n", attributes: plain)
        for (index, link) in AboutNotice.links.enumerated() {
            if index > 0 { text.append(NSAttributedString(string: "  ·  ", attributes: plain)) }
            var attributes = plain
            attributes[.link] = link.url
            text.append(NSAttributedString(string: link.title, attributes: attributes))
        }
        return text
    }
}
