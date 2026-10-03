import Foundation
@testable import PPVPNAppLogic
import XCTest

/// A path source the test drives: `emit()` is one NWPathMonitor update.
final class FakeNetworkPath: NetworkPathSource, @unchecked Sendable {
    private var onUpdate: (@Sendable () -> Void)?
    private(set) var cancelled = false

    func start(_ onUpdate: @escaping @Sendable () -> Void) { self.onUpdate = onUpdate }
    func cancel() { cancelled = true }
    func emit() { onUpdate?() }
}

/// Counts notifications, thread-safely.
final class Counter: @unchecked Sendable {
    private let lock = NSLock()
    private var count = 0
    var value: Int {
        lock.lock()
        defer { lock.unlock() }
        return count
    }

    func increment() {
        lock.lock()
        count += 1
        lock.unlock()
    }
}

final class NetworkChangeWatcherTests: XCTestCase {
    private let debounce = DispatchTimeInterval.milliseconds(50)

    private func watcher(_ path: FakeNetworkPath, _ counter: Counter) -> NetworkChangeWatcher {
        let watcher = NetworkChangeWatcher(source: path, debounce: debounce) { counter.increment() }
        watcher.start()
        return watcher
    }

    private func wait(_ seconds: TimeInterval) {
        let done = expectation(description: "wait")
        DispatchQueue.global().asyncAfter(deadline: .now() + seconds) { done.fulfill() }
        wait(for: [done], timeout: seconds + 2)
    }

    func testTheInitialPathIsNotAChange() {
        let path = FakeNetworkPath()
        let counter = Counter()
        let watcher = watcher(path, counter)
        path.emit()
        wait(0.2)
        XCTAssertEqual(counter.value, 0)
        _ = watcher
    }

    func testABurstNotifiesOnceAfterItSettles() {
        let path = FakeNetworkPath()
        let counter = Counter()
        let watcher = watcher(path, counter)
        path.emit() // initial
        for _ in 0..<5 {
            path.emit()
            Thread.sleep(forTimeInterval: 0.01)
        }
        XCTAssertEqual(counter.value, 0, "not before the burst settles")
        wait(0.2)
        XCTAssertEqual(counter.value, 1)

        // A later change is its own burst.
        path.emit()
        wait(0.2)
        XCTAssertEqual(counter.value, 2)
        _ = watcher
    }

    func testCancelStopsTheSourceAndThePendingNotification() {
        let path = FakeNetworkPath()
        let counter = Counter()
        let watcher = watcher(path, counter)
        path.emit()
        path.emit()
        watcher.cancel()
        wait(0.2)
        XCTAssertTrue(path.cancelled)
        XCTAssertEqual(counter.value, 0)
    }
}

@MainActor
final class AppStateNetworkTests: LogicTestCase {
    func testNetworkChangesReachTheBackend() async {
        let backend = StubBackend()
        backend.current = .fixture()
        let path = FakeNetworkPath()
        let state = RecordingState(backend: backend, networkPath: path, networkDebounce: .milliseconds(50))
        path.emit() // initial path
        await settle(timeout: 0.2)
        XCTAssertFalse(backend.calls.contains("networkChanged"))

        path.emit()
        path.emit()
        await settle { backend.calls.contains("networkChanged") }
        await settle(timeout: 0.2)
        XCTAssertEqual(backend.calls.filter { $0 == "networkChanged" }.count, 1)
        _ = state
    }
}
