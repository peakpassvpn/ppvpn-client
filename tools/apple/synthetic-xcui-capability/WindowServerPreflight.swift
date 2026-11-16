import AppKit
import ApplicationServices
import Foundation
import Darwin

let session = CGSessionCopyCurrentDictionary() as? [String: Any]
let onConsole = session?[kCGSessionOnConsoleKey as String] as? Bool == true
var displays: UInt32 = 0
let displayStatus = CGGetActiveDisplayList(0, nil, &displays)
let ready = getuid() != 0 && onConsole && displayStatus == .success && displays > 0 && !NSScreen.screens.isEmpty
let value: [String: Any] = [
    "stage": "readonly_WindowServer_preflight",
    "on_console": onConsole,
    "non_root": getuid() != 0,
    "active_display_count": displays,
    "screen_count": NSScreen.screens.count,
    "wrapper_AX_trusted_without_prompt": AXIsProcessTrusted(),
    "wrapper_AX_trust_is_not_XCTest_runner_permission": true,
    "ready_to_attempt_standard_XCTest": ready,
    "App359_installed_or_launched": false,
]
let bytes = try JSONSerialization.data(withJSONObject: value, options: [.sortedKeys])
print(String(decoding: bytes, as: UTF8.self))
exit(ready ? 0 : 77)
