import Foundation
import PPVPNAppLogic
import XCTest

final class LogsTests: LogicTestCase {
    private var directory: URL!

    override func setUpWithError() throws {
        try super.setUpWithError()
        directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    }

    override func tearDownWithError() throws {
        try? FileManager.default.removeItem(at: directory)
    }

    func testTailReplacesAppendsAndRestartsOnTruncation() throws {
        let file = directory.appendingPathComponent("ppvpn-client.2020-01-02.log")
        try Data("one\n".utf8).write(to: file)
        let tail = LogTail()
        guard case .replace("one\n") = tail.read(file) else { return XCTFail("first read replaces") }
        guard case .none = tail.read(file) else { return XCTFail("nothing new") }

        let handle = try FileHandle(forWritingTo: file)
        handle.seekToEndOfFile()
        handle.write(Data("two\n".utf8))
        try handle.close()
        guard case .append("two\n") = tail.read(file) else { return XCTFail("appends the new bytes") }

        try Data("x\n".utf8).write(to: file)
        guard case .replace("x\n") = tail.read(file) else { return XCTFail("truncation starts over") }
    }

    func testTailOfMissingFileClearsOnce() {
        let tail = LogTail()
        guard case .none = tail.read(nil) else { return XCTFail() }
        let file = directory.appendingPathComponent("gone.log")
        try? Data("a".utf8).write(to: file)
        _ = tail.read(file)
        try? FileManager.default.removeItem(at: file)
        guard case .replace("") = tail.read(file) else { return XCTFail("clears the view") }
        guard case .none = tail.read(file) else { return XCTFail("then stays quiet") }
    }

    func testFirstReadTakesOnlyTheEnd() throws {
        let file = directory.appendingPathComponent("big.log")
        // 1.2 MiB of 100-byte lines: the last MiB, from its first whole line.
        let line = String(repeating: "a", count: 99) + "\n"
        try Data(String(repeating: line, count: 12 * 1024 * 1024 / 1000).utf8).write(to: file)
        guard case .replace(let text) = LogTail().read(file) else { return XCTFail() }
        XCTAssertLessThanOrEqual(text.utf8.count, 1024 * 1024)
        XCTAssertGreaterThan(text.utf8.count, 1024 * 1024 - 100)
        XCTAssertTrue(text.hasPrefix(line))
    }

    func testLogFileNamesAndOrder() throws {
        let names = ["ppvpn-core.2020-01-01.log", "ppvpn-client.2020-01-01.log", "ppvpn-client.2020-01-02.log",
                     "other.2020-01-02.log", "ppvpn-client.2020-01-02.txt"]
        let files = names.compactMap { LogFile(url: directory.appendingPathComponent($0)) }.sorted()
        XCTAssertEqual(files.map(\.url.lastPathComponent),
                       ["ppvpn-client.2020-01-02.log", "ppvpn-client.2020-01-01.log", "ppvpn-core.2020-01-01.log"])
        XCTAssertEqual(files.first?.kind, .client)
        XCTAssertEqual(files.last?.title, "\(tr("core")) · 2020-01-01")
    }

    func testTodayTitle() {
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC")!
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = utc.timeZone
        formatter.dateFormat = "yyyy-MM-dd"
        let day = formatter.string(from: Date())
        let file = LogFile(url: directory.appendingPathComponent("ppvpn-client.\(day).log"))
        XCTAssertEqual(file?.title, "\(tr("client")) · \(tr("today"))")
    }
}
