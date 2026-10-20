// compare.py on macOS: prints "yes" when any of the given processes has an
// on-screen window at the normal level, "no" otherwise.
import CoreGraphics

let pids = Set(CommandLine.arguments.dropFirst().compactMap { Int32($0) })
let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID)
    as? [[String: Any]] ?? []
let shown = windows.contains { window in
    guard let pid = window[kCGWindowOwnerPID as String] as? Int32, pids.contains(pid) else { return false }
    return (window[kCGWindowLayer as String] as? Int) == 0
}
print(shown ? "yes" : "no")
