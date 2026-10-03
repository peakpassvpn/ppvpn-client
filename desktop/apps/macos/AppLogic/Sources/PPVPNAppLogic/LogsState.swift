import Foundation

/// The Logs page: the daily log files of the log directory, the followed
/// file's parsed entries and the ones the level filter and search let
/// through (`shown`).
///
/// Reading, parsing and filtering run on a background actor; the main actor
/// only receives the results, once per change. The level filter applies at
/// once, the search after `searchDelay` without typing.
///
/// Platform glue subclasses it: `willChange()` feeds the UI's change
/// notification.
@MainActor
open class LogsState {
    public let directory: URL
    /// Newest day first (see `LogFile`).
    public private(set) var files: [LogFile] = [] { willSet { willChange() } }
    public var selected: LogFile? {
        willSet { willChange() }
        didSet { if selected != oldValue { reload() } }
    }
    public var level = LogLevelFilter.all {
        willSet { willChange() }
        didSet { if level != oldValue { refilter(after: nil) } }
    }
    public var search = "" {
        willSet { willChange() }
        didSet { if search != oldValue { refilter(after: searchDelay) } }
    }
    /// The entries the filter and search let through, oldest first.
    public private(set) var shown: [LogEntry] = [] { willSet { willChange() } }
    /// The followed file has entries (`shown` may still be empty: no match).
    public private(set) var hasEntries = false { willSet { willChange() } }

    public var searchDelay: Duration = .milliseconds(200)
    /// Filter passes run (tests: the search waits for typing to pause).
    public private(set) var filterRuns = 0

    private let reader = LogReader()
    private var polling: Task<Void, Never>?
    private var filtering: Task<Void, Never>?
    /// The latest filter request; older results are dropped.
    private var request = 0

    public init(directory: URL) {
        self.directory = directory
    }

    open func willChange() {}

    /// Lists the files and reads what the followed one gained; call it
    /// periodically. A poll still running is not doubled.
    public func poll() {
        guard polling == nil else { return }
        polling = Task {
            let files = await reader.list(directory)
            if files != self.files { self.files = files }
            if selected == nil || !files.contains(selected!) {
                // Through didSet: reads the new file.
                selected = files.first { $0.kind == .client } ?? files.first
            } else {
                await read()
            }
            polling = nil
        }
    }

    /// The raw text of the shown entries (copy).
    public var shownText: String { shown.map(\.raw).joined(separator: "\n") }

    // MARK: Private

    private func reload() {
        Task { await read() }
    }

    private func read() async {
        guard await reader.read(selected?.url) else { return }
        await filter()
    }

    private func refilter(after delay: Duration?) {
        filtering?.cancel()
        filtering = Task {
            if let delay {
                try? await Task.sleep(for: delay)
                if Task.isCancelled { return }
            }
            await filter()
        }
    }

    private func filter() async {
        request += 1
        let current = request
        filterRuns += 1
        let (entries, total) = await reader.visible(level: level, search: search)
        guard current == request else { return }
        if entries.map(\.stamp) != shown.map(\.stamp) { shown = entries }
        if hasEntries != (total > 0) { hasEntries = total > 0 }
    }
}

/// The followed file's tail and parsed entries, off the main actor.
actor LogReader {
    private let tail = LogTail()
    private var buffer = LogBuffer()

    func list(_ directory: URL) -> [LogFile] {
        let listed = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        return listed.compactMap(LogFile.init(url:)).sorted()
    }

    /// Reads what `url` gained; true when the entries changed.
    func read(_ url: URL?) -> Bool {
        let change = tail.read(url)
        if case .none = change { return false }
        buffer.apply(change)
        return true
    }

    func visible(level: LogLevelFilter, search: String) -> ([LogEntry], total: Int) {
        (buffer.visible(level: level, search: search), buffer.entries.count)
    }
}

/// How the Logs text view gets from what it shows (`previous`, oldest first)
/// to `next`: drop entries the buffer let go from the front, redraw the last
/// one when it was continued, append the new ones. Anything else (another
/// file, filter or search) rebuilds.
public enum LogTextPlan: Equatable, Sendable {
    case unchanged
    case rebuild
    /// `dropFirst` entries leave the front; the last shown one is redrawn
    /// when `replaceLast`; `next[appendFrom...]` are added.
    case update(dropFirst: Int, replaceLast: Bool, appendFrom: Int)

    public static func make(previous: [LogEntry.Stamp], next: [LogEntry]) -> LogTextPlan {
        if previous.isEmpty { return next.isEmpty ? .unchanged : .rebuild }
        guard let first = next.first, let start = previous.firstIndex(where: { $0.id == first.id }) else {
            return .rebuild
        }
        let kept = previous.count - start
        guard next.count >= kept else { return .rebuild }
        for offset in 0..<kept {
            let old = previous[start + offset]
            let new = next[offset].stamp
            guard old.id == new.id else { return .rebuild }
            // Only the last shown entry may have grown (a continued line).
            if old.rawLength != new.rawLength, offset != kept - 1 { return .rebuild }
        }
        let replaceLast = previous[previous.count - 1].rawLength != next[kept - 1].stamp.rawLength
        if start == 0, !replaceLast, kept == next.count { return .unchanged }
        return .update(dropFirst: start, replaceLast: replaceLast, appendFrom: kept)
    }
}
