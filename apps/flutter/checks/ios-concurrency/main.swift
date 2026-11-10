// Focused native checks: no AppGroup access or network operation is invoked.
import Foundation

@main
struct InternalProbeChecks {
  static func main() throws {
    var checked = 0
    func check(_ value: Bool, _ name: String) {
      precondition(value, name); checked += 1
    }
    let defaults: [String: Any] = [
      "schema": 1, "run_token": "F20967AA-29CE-43EA-94F7-393F2DCA1128", "expires_unix": 1100,
      "run_id": "C49C6CF3-02CD-4CD1-9D07-8172D434D386",
      "target_manifest_sha256": String(repeating: "a", count: 64),
      "operation_timeout_seconds": 10, "rounds": 1, "round_interval_ms": 0, "dns_name_mode": "per_id",
      "kind": "dns", "host": "10.60.159.90", "port": 53, "count": 64, "concurrency": 32,
      "per_second": 32, "duration_seconds": 30, "max_application_bytes": 1048576,
      "min_available_bytes": 8388608, "max_footprint_bytes": 44040192,
      "max_consecutive_failures": 3, "payload_bytes": 1024, "write_chunks": 1,
      "write_interval_ms": 0, "read_delay_ms": 0, "dns_name": "example.com",
      "dns_expected_ipv4": ["93.184.216.34"], "dns_expected_rcode": 0, "dns_shared_socket": true
    ]
    func spec(_ changes: [String: Any] = [:]) throws -> InternalLoadProbe.Spec {
      let data = try JSONSerialization.data(withJSONObject: defaults.merging(changes) { _, new in new })
      let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
      return try decoder.decode(InternalLoadProbe.Spec.self, from: data)
    }
    check(try spec().validate(now: 1000), "valid staged spec with snake-case IPv4 field")
    check(try spec().dnsExpectedIpv4 == ["93.184.216.34"], "IPv4 acronym decodes")
    check(try spec(["host": "fixture.example.com"]).validate(now: 1000), "public owned endpoint requires manifest next")
    check(try !spec(["kind": "tcp", "write_chunks": 8, "write_interval_ms": 1000, "read_delay_ms": 5000]).validate(now: 1000), "fixture timing cannot exceed operation deadline")
    let approved = InternalLoadProbe.TargetManifest(schema: 1, expiresUnix: 1100, requiredPath: "ne_proxy", endpoints: [
      .init(kind: "dns", host: "10.60.159.90", port: 53, httpPath: nil, https: nil, dnsSuffix: "example.com", purpose: "fixture")])
    check(try approved.permits(spec(), now: 1000), "exact manifest match")
    check(try !approved.permits(spec(["port": 54]), now: 1000), "port mismatch refused")
    check(try !approved.permits(spec(["dns_name": "unreviewed.example.com"]), now: 1000), "DNS suffix mismatch refused")
    check(try !approved.permits(spec(), now: 1200), "expired manifest")
    let observer = InternalLoadProbe.TargetManifest(schema: 1, expiresUnix: 1100, requiredPath: "ne_proxy", endpoints: [
      .init(kind: "http3", host: "www.peakpass.dev", port: 443, httpPath: "/robots.txt", https: true, dnsSuffix: nil, purpose: "h3_observer")])
    let h3: [String: Any] = ["kind": "http3", "host": "www.peakpass.dev", "port": 443, "http_path": "/robots.txt", "https": true, "count": 3, "concurrency": 1, "per_second": 1]
    check(try observer.permits(spec(h3), now: 1000), "owned observer low-rate path")
    check(try !observer.permits(spec(h3.merging(["count": 4]) { _, new in new }), now: 1000), "production observer never becomes pressure fixture")
    let fixed1 = InternalLoadProbe.makeDNSQuery(1, runId: "abcdef12", name: "example.com", mode: "fixed_cache")
    let fixed2 = InternalLoadProbe.makeDNSQuery(2, runId: "abcdef12", name: "example.com", mode: "fixed_cache")
    let cold1 = InternalLoadProbe.makeDNSQuery(1, runId: "abcdef12", name: "example.com", mode: "per_id")
    let cold2 = InternalLoadProbe.makeDNSQuery(2, runId: "abcdef12", name: "example.com", mode: "per_id")
    check(fixed1.dropFirst(12) == fixed2.dropFirst(12), "fixed_cache explicitly shares qname")
    check(cold1.dropFirst(12) != cold2.dropFirst(12), "per_id has distinct qnames, not only IDs")
    check(cold1.dropFirst(12).contains(114), "per_id contains run-specific label")
    let echo = InternalLoadProbe.makeEchoPayload(token: "F20967AA-29CE-43EA-94F7-393F2DCA1128", id: 2047, round: 7, count: 60000)
    let head = InternalLoadProbe.chunkRange(payload: echo, chunks: 64, chunk: 0)
    check(echo.count == 60000, "large legal UDP application payload")
    check(echo.subdata(in: head).contains(10), "first TCP write covers full authenticated header")
    var rebuilt = Data()
    for chunk in 0..<64 { rebuilt.append(echo.subdata(in: InternalLoadProbe.chunkRange(payload: echo, chunks: 64, chunk: chunk))) }
    check(rebuilt == echo, "slow chunks preserve exact wire bytes")
    check(InternalLoadProbe.chunkRange(payload: echo, chunks: 1, chunk: 0) == 0..<60000, "UDP sent as single datagram")
    var batch = defaults
    batch["workloads"] = [["kind": "tcp", "count": 16], ["kind": "udp", "count": 16], ["kind": "dns", "count": 64]]
    let batchData = try JSONSerialization.data(withJSONObject: batch)
    check(InternalLoadProbe.decodeRequest(batchData, now: 1000)?.count == 3, "mixed lanes decode together")
    batch["workloads"] = [["kind": "tcp", "run_token": "override"]]
    check(InternalLoadProbe.decodeRequest(try JSONSerialization.data(withJSONObject: batch), now: 1000) == nil, "lane cannot replace run capability")
    batch["workloads"] = [["kind": "tcp", "concurrency": 200], ["kind": "udp", "concurrency": 200]]
    check(InternalLoadProbe.decodeRequest(try JSONSerialization.data(withJSONObject: batch), now: 1000) == nil, "aggregate concurrency bounded")
    for (name, changes) in [
      ("bad host", ["host": "bad host"]), ("loopback bypass", ["host": "127.0.0.1"]),
      ("noncanonical IP", ["host": "010.0.0.1"]), ("invalid token", ["run_token": "../../x"]),
      ("expired", ["expires_unix": 999]), ("far future", ["expires_unix": 1400]),
      ("too many operations", ["count": 100000]), ("oversize UDP", ["payload_bytes": 65536]),
      ("low memory guard", ["min_available_bytes": 1]), ("unbounded traffic", ["max_application_bytes": 100000000]),
      ("bad DNS name", ["dns_name": "a/b.example.com"]), ("missing answer expectation", ["dns_expected_ipv4": []]),
      ("unbounded failure tolerance", ["max_consecutive_failures": 1000]),
      ("too slow write", ["kind": "tcp", "write_chunks": 64, "write_interval_ms": 1000]),
      ("QUIC without TLS", ["kind": "http3", "https": false]),
      ("URL credential/query", ["http_path": "/?token=x"])
    ] as [(String, [String: Any])] {
      check(try !spec(changes).validate(now: 1000), name)
    }
    let question: [UInt8] = [7] + Array("example".utf8) + [3] + Array("com".utf8) + [0, 0, 1, 0, 1]
    let query = Data([0x12, 0x34, 1, 0, 0, 1, 0, 0, 0, 0, 0, 0] + question)
    let answer = Data([0x12, 0x34, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0] + question + [0xc0, 0x0c, 0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 93, 184, 216, 34])
    func failure(_ response: Data, ips: [String] = ["93.184.216.34"], rcode: UInt8 = 0) -> String? {
      InternalLoadProbe.dnsFailure(response, query: query, expectedRcode: rcode, expectedIPv4: ips)
    }
    check(failure(answer) == nil, "compressed valid answer")
    check(failure(answer, ips: ["1.2.3.4"]) == "dns_answer_mismatch", "wrong address is failure")
    var wrongID = answer; wrongID[0] = 0xff
    check(failure(wrongID) != nil, "wrong ID")
    var truncated = answer; truncated[2] |= 2
    check(failure(truncated) != nil, "TC bit not success")
    check(failure(answer.dropLast()) == "dns_malformed", "short RDATA")
    var badPointer = answer; badPointer[query.count+1] = 0xff
    check(failure(badPointer) == "dns_malformed", "out of range owner pointer")
    let nx = Data([0x12, 0x34, 0x81, 0x83, 0, 1, 0, 0, 0, 0, 0, 0] + question)
    check(failure(nx, ips: [], rcode: 3) == nil, "NXDOMAIN expected")
    check(failure(nx) != nil, "NXDOMAIN when A expected")
    var invalidNX = answer; invalidNX[3] = 0x83
    check(failure(invalidNX, ips: [], rcode: 3) == "dns_nxdomain_with_answer", "NX with A not success")
    let uncompressed = Data([0x12, 0x34, 0x81, 0x80, 0, 1, 0, 1, 0, 0, 0, 0] + question + question.dropLast(4) + [0, 1, 0, 1, 0, 0, 0, 60, 0, 4, 93, 184, 216, 34])
    check(failure(uncompressed) == nil, "uncompressed record owner")
    print("PASS \(checked) native spec/DNS checks; no network or device action")
  }
}
