import Foundation

/// Reports changes of the system's network path (interfaces, addresses, the
/// default route). Its first update describes the path at start, not a change.
public protocol NetworkPathSource: AnyObject, Sendable {
    func start(_ onUpdate: @escaping @Sendable () -> Void)
    func cancel()
}

/// Turns path updates into one `notify` per burst: the source's first update
/// (the current path) is ignored, later ones fire `notify` once no other came
/// for `debounce` (a Wi-Fi switch reports several in a row).
public final class NetworkChangeWatcher: @unchecked Sendable {
    private let source: NetworkPathSource
    private let debounce: DispatchTimeInterval
    private let notify: @Sendable () -> Void
    // State below only on `queue`.
    private let queue = DispatchQueue(label: "com.peakpassvpn.ppvpn.network-change")
    private var sawInitialPath = false
    private var pending: DispatchWorkItem?

    public init(source: NetworkPathSource, debounce: DispatchTimeInterval = .seconds(1),
                notify: @escaping @Sendable () -> Void) {
        self.source = source
        self.debounce = debounce
        self.notify = notify
    }

    public func start() {
        source.start { [weak self] in self?.pathUpdated() }
    }

    public func cancel() {
        source.cancel()
        queue.sync {
            pending?.cancel()
            pending = nil
        }
    }

    private func pathUpdated() {
        queue.async { [self] in
            guard sawInitialPath else {
                sawInitialPath = true
                return
            }
            pending?.cancel()
            let work = DispatchWorkItem { [notify] in notify() }
            pending = work
            queue.asyncAfter(deadline: .now() + debounce, execute: work)
        }
    }
}

#if canImport(Network)
import Network

/// `NWPathMonitor` as a `NetworkPathSource` (macOS; the rest of AppLogic
/// builds on Linux).
public final class SystemNetworkPath: NetworkPathSource, @unchecked Sendable {
    private let monitor = NWPathMonitor()

    public init() {}

    public func start(_ onUpdate: @escaping @Sendable () -> Void) {
        monitor.pathUpdateHandler = { _ in onUpdate() }
        monitor.start(queue: DispatchQueue(label: "com.peakpassvpn.ppvpn.network-path"))
    }

    public func cancel() { monitor.cancel() }
}
#endif
