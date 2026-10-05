import Foundation
import XCTest

/// The shared strings.json against itself and against the macOS sources.
final class StringsTests: XCTestCase {
    private func load() throws -> [String: [String: String]] {
        try JSONDecoder().decode([String: [String: String]].self, from: Data(contentsOf: Repo.strings))
    }

    private func placeholders(_ text: String) -> Set<String> {
        let regex = try! NSRegularExpression(pattern: #"\{([A-Za-z0-9_]+)\}"#)
        let range = NSRange(text.startIndex..., in: text)
        return Set(regex.matches(in: text, range: range).compactMap {
            Range($0.range(at: 1), in: text).map { String(text[$0]) }
        })
    }

    func testLanguagesHaveTheSameKeys() throws {
        let strings = try load()
        let zh = Set(strings["zh"]?.keys ?? [:].keys), en = Set(strings["en"]?.keys ?? [:].keys)
        XCTAssertFalse(zh.isEmpty)
        XCTAssertEqual(zh.subtracting(en).sorted(), [], "zh keys missing in en")
        XCTAssertEqual(en.subtracting(zh).sorted(), [], "en keys missing in zh")
    }

    /// Both directions: a placeholder in either language must be in the other.
    func testPlaceholdersMatchAcrossLanguages() throws {
        let strings = try load()
        let zh = strings["zh"] ?? [:], en = strings["en"] ?? [:]
        for (key, zhText) in zh.sorted(by: { $0.key < $1.key }) {
            guard let enText = en[key] else { continue }
            let zhNames = placeholders(zhText), enNames = placeholders(enText)
            XCTAssertEqual(zhNames.subtracting(enNames).sorted(), [], "\(key): only in zh")
            XCTAssertEqual(enNames.subtracting(zhNames).sorted(), [], "\(key): only in en")
        }
    }

    /// Every literal `tr("key"...)` in the macOS sources resolves, and the
    /// `{names}` it fills are ones the text has.
    func testMacOSLookupsResolve() throws {
        let zh = try SharedStrings.load("zh").texts
        let call = try NSRegularExpression(pattern: #"\btr\("([A-Za-z0-9_]+)"(?=\s*[,)])(?:,\s*\[([^\]]*)\])?"#)
        let argument = try NSRegularExpression(pattern: #""([A-Za-z0-9_]+)":"#)
        var checked = 0
        let files = FileManager.default.enumerator(at: Repo.sources, includingPropertiesForKeys: nil)!
        for case let file as URL in files where file.pathExtension == "swift" && !file.path.contains("/.build/")
            && !file.path.contains("/PPVPNClient/") && !file.path.contains("/Tests/") {
            let source = try String(contentsOf: file, encoding: .utf8)
            for match in call.matches(in: source, range: NSRange(source.startIndex..., in: source)) {
                let key = String(source[Range(match.range(at: 1), in: source)!])
                let where_ = "\(file.lastPathComponent): \(key)"
                guard let text = zh[key] else {
                    XCTFail("\(where_) is not in strings.json or the x_ keys")
                    continue
                }
                checked += 1
                guard let argsRange = Range(match.range(at: 2), in: source) else { continue }
                let args = String(source[argsRange])
                let names = argument.matches(in: args, range: NSRange(args.startIndex..., in: args))
                    .map { String(args[Range($0.range(at: 1), in: args)!]) }
                XCTAssertEqual(Set(names), placeholders(text), where_)
            }
        }
        XCTAssertGreaterThan(checked, 100)
    }
}
