// Offline golden from actual prepare_stage -> Seq.prepare -> guarded main.
// Never starts InternalLoadProbe, NWConnection, AppGroup or any device action.
import Foundation
import CryptoKit

@main
struct Linked256ContractChecks {
  static func main() throws {
    precondition(CommandLine.arguments.count == 2, "golden directory required")
    let directory = URL(fileURLWithPath: CommandLine.arguments[1], isDirectory: true)
    let requestData = try Data(contentsOf: directory.appendingPathComponent("request.json"))
    let manifestData = try Data(contentsOf: directory.appendingPathComponent("approved-targets.json"))
    let metadata = try JSONSerialization.jsonObject(with: Data(contentsOf:
      directory.appendingPathComponent("contract.json"))) as! [String: Any]
    precondition(metadata["test_only"] as? Bool == true, "fictional fixture only")
    let now = (metadata["now"] as! NSNumber).doubleValue
    let token = metadata["token"] as! String
    let runID = metadata["run_id"] as! String
    let count = (metadata["count"] as! NSNumber).intValue
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    var checked = 0
    func check(_ ok: Bool, _ label: String) { precondition(ok, label); checked += 1 }
    func accepts(_ bytes: Data, _ targets: Data) -> Bool {
      guard let spec = try? decoder.decode(InternalLoadProbe.Spec.self, from: bytes),
        let manifest = try? decoder.decode(InternalLoadProbe.TargetManifest.self, from: targets)
      else { return false }
      let hash = SHA256.hash(data: targets).map { String(format: "%02x", $0) }.joined()
      return spec.validate(now: now) && manifest.permits(spec, now: now)
        && spec.targetManifestSha256 == hash && spec.runToken == token && spec.runId == runID
        && spec.kind == "tcp" && spec.count == count && spec.concurrency == count
        && spec.maxApplicationBytes == 1_048_576 && spec.minAvailableBytes >= 12_582_912
        && spec.maxFootprintBytes <= 33_554_432
        && count * (spec.payloadBytes * 2 * spec.rounds + 4096) <= spec.maxApplicationBytes
    }
    func changed(_ key: String, _ value: Any) throws -> Data {
      var object = try JSONSerialization.jsonObject(with: requestData) as! [String: Any]
      object[key] = value
      return try JSONSerialization.data(withJSONObject: object)
    }
    check(accepts(requestData, manifestData), "real Python-produced golden accepted by native contracts")
    check(InternalLoadProbe.expectedSessionLimit == 256, "diagnostic source expects exactly256")
    check(InternalLoadProbe.hasExpectedSessionLimit(["limit": 256]), "actual256 snapshot limit")
    check(!InternalLoadProbe.hasExpectedSessionLimit(["limit": 128]), "old128 snapshot rejected")
    check(!InternalLoadProbe.hasExpectedSessionLimit(["limit": 512]), "other cap rejected")
    check(!accepts(requestData, manifestData + Data([32])), "changed manifest hash rejected")
    check(try !accepts(changed("run_token", runID), manifestData), "public UUID cannot be capability")
    check(try !accepts(changed("run_token", UUID().uuidString.lowercased()), manifestData), "other capability rejected")
    check(try !accepts(changed("count", count + 1), manifestData), "count binding rejected")
    check(try !accepts(changed("max_application_bytes", 1_048_577), manifestData), "byte ceiling not enlarged")
    check(try !accepts(changed("min_available_bytes", 12_582_911), manifestData), "12MiB guard retained")
    check(try !accepts(changed("max_footprint_bytes", 33_554_433), manifestData), "32MiB guard retained")
    check(try !accepts(changed("expires_unix", now - 1), manifestData), "expired request rejected")
    check(try !accepts(changed("host", "other.invalid"), manifestData), "exact endpoint retained")
    let spec = try decoder.decode(InternalLoadProbe.Spec.self, from: requestData)
    let payload = InternalLoadProbe.makeEchoPayload(token: spec.runToken, id: 0, round: 0, count: spec.payloadBytes)
    check(payload.count == spec.payloadBytes, "native authenticated payload has reserved length")
    print("PASS \(checked) linked256 golden checks; fictional metadata; no network/device action")
  }
}
