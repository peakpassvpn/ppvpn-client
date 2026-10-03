import AppKit
import PPVPNAppLogic
import SwiftUI

/// Read-only, selectable log entries (`LogTextRenderer`): new entries are
/// appended and old ones dropped in place, and the view follows the end while
/// the user is scrolled to the bottom.
struct LogTextView: NSViewRepresentable {
    let entries: [LogEntry]

    final class Coordinator {
        let renderer = LogTextRenderer()
    }

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSTextView.scrollableTextView()
        scroll.hasHorizontalScroller = false
        scroll.drawsBackground = false
        guard let text = scroll.documentView as? NSTextView else { return scroll }
        text.isEditable = false
        text.isSelectable = true
        text.drawsBackground = false
        text.textContainerInset = NSSize(width: 10, height: 10)
        text.font = LogTextRenderer.mono
        text.defaultParagraphStyle = LogTextRenderer.paragraph
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        guard let textView = scroll.documentView as? NSTextView else { return }
        let atBottom = scroll.contentView.bounds.maxY >= textView.frame.height - 24
        switch context.coordinator.renderer.update(textView, to: entries) {
        case .unchanged:
            return
        case .rebuild:
            textView.scrollToEndOfDocument(nil)
        case .update:
            if atBottom { textView.scrollToEndOfDocument(nil) }
        }
    }
}
