#if PPVPN_INTERNAL_DIAGNOSTICS
import Foundation
import Network
  import CryptoKit // Keep inside the internal-only block when ci-build merges imports.

/// A one-shot, root-staged INTERNAL Release workload. Runs in Runner and uses
/// normal sockets/URLSession, not the NE engine's dial API. No credentials,
/// account credentials, cookies, backend login, route edits, trust exceptions or public listeners.
final class InternalLoadProbe: NSObject, URLSessionDataDelegate, URLSessionTaskDelegate, @unchecked Sendable {
  /// Fixed contract for the separately built internal 256 candidate only.
  /// A 128 snapshot is never accepted as a 256 result; no runtime override.
  static let expectedSessionLimit = 256
  static func hasExpectedSessionLimit(_ sessions: [String: Any]) -> Bool {
    guard let limit = sessions["limit"] as? NSNumber else { return false }
    return limit.doubleValue == Double(expectedSessionLimit)
  }

  /// Actual application payload accounting, distinct from offer reservations.
  /// Receive callbacks are charged in full, including unknown/late packets.
  struct ByteBudget {
    let limit: Int
    private(set) var sent = 0
    private(set) var received = 0
    private(set) var tripped = false
    var total: Int { sent + received }
    var remaining: Int { max(0, limit - total) }
    var overshoot: Int { max(0, total - limit) }

    mutating func account(_ count: Int, sending: Bool) -> Bool {
      precondition(count >= 0)
      if sending {
        guard !tripped, count <= remaining else { tripped = true; return false }
        sent += count
      } else {
        // Callbacks are bounded by their transport; never hide a delivered
        // datagram by clipping it to the remaining budget.
        received += count
      }
      if total > limit { tripped = true }
      return !tripped
    }
  }

  enum SharedDNSPacket: Equatable { case budgetExceeded, unexpected, reply(Int) }
  static func accountSharedDNS(_ data: Data, pending: Set<Int>, budget: inout ByteBudget) -> SharedDNSPacket {
    guard budget.account(data.count, sending: false) else { return .budgetExceeded }
    guard data.count >= 2 else { return .unexpected }
    let id = Int(data[data.startIndex]) << 8 | Int(data[data.startIndex + 1])
    return pending.contains(id) ? .reply(id) : .unexpected
  }

  @propertyWrapper
  struct CanonicalUUID: Decodable {
    let wrappedValue: String
    init(from decoder: Decoder) throws {
      let text = try decoder.singleValueContainer().decode(String.self)
      wrappedValue = UUID(uuidString: text)?.uuidString.lowercased() ?? text
    }
  }
  struct Spec: Decodable {
    let schema: Int
    @CanonicalUUID var runToken: String
    @CanonicalUUID var runId: String // public correlation only; never echo the capability token
    let expiresUnix: Double
    let kind: String // tcp, udp, dns, http, http3 (observed protocol required)
    let host: String // exact root-frozen manifest endpoint, private or owned public
    let port: UInt16
    let count: Int
    let concurrency: Int
    let perSecond: Int
    let durationSeconds: Int
    let maxApplicationBytes: Int
    let minAvailableBytes: UInt64
    let maxFootprintBytes: UInt64
    let maxConsecutiveFailures: Int
    let payloadBytes: Int
    let writeChunks: Int
    let writeIntervalMs: Int
    let readDelayMs: Int
    let targetManifestSha256: String
    let operationTimeoutSeconds: Int
    let rounds: Int
    let roundIntervalMs: Int
    let dnsNameMode: String?
    let dnsName: String?
    let dnsExpectedIpv4: [String]?
    let dnsExpectedRcode: UInt8?
    let dnsSharedSocket: Bool?
    let httpPath: String?
    let https: Bool?

    func validate(now: Double) -> Bool {
      schema == 1 && UUID(uuidString: runToken) != nil && UUID(uuidString: runId) != nil
        && UUID(uuidString: runId) != UUID(uuidString: runToken)
        && expiresUnix > now && expiresUnix <= now + 300
        && ["tcp", "udp", "dns", "http", "http3"].contains(kind)
        && Self.validTargetHost(host) && targetManifestSha256.count == 64
        && targetManifestSha256.utf8.allSatisfy { (48...57).contains($0) || (97...102).contains($0) }
        && port > 0 && (1...2048).contains(count)
        && (1...256).contains(concurrency) && (1...400).contains(perSecond)
        && (1...120).contains(durationSeconds) && (8192...(8 * 1024 * 1024)).contains(maxApplicationBytes)
        && minAvailableBytes >= 8 * 1024 * 1024 && maxFootprintBytes > 0
        && (1...32).contains(maxConsecutiveFailures) && (64...60000).contains(payloadBytes)
        && (1...64).contains(writeChunks) && writeChunks <= payloadBytes - 49
        && (0...1000).contains(writeIntervalMs) && writeChunks * writeIntervalMs <= 8000
        && (0...5000).contains(readDelayMs) && (kind == "tcp" || writeChunks == 1)
        && (1...30).contains(operationTimeoutSeconds) && (1...8).contains(rounds) && (0...1000).contains(roundIntervalMs)
        && (rounds == 1 || kind == "tcp" || kind == "udp")
        && rounds * ((writeChunks - 1) * writeIntervalMs + readDelayMs) + (rounds - 1) * roundIntervalMs + 1000 <= operationTimeoutSeconds * 1000
        && (count + perSecond - 1) / perSecond + operationTimeoutSeconds <= durationSeconds
        && (kind != "dns" || (port == 53 && Self.validDNSName(dnsName ?? "")
          && ["fixed_cache", "per_id"].contains(dnsNameMode ?? "")
          && (dnsNameMode != "per_id" || (dnsName?.count ?? 254) <= 230)
          && ([0, 3].contains(dnsExpectedRcode ?? 255))
          && (dnsExpectedRcode == 3 || !(dnsExpectedIpv4 ?? []).isEmpty)))
        && (kind != "http3" || https == true)
        && (httpPath == nil || (httpPath!.hasPrefix("/") && !httpPath!.contains("?") && !httpPath!.contains("#") && httpPath!.count <= 128))
    }

    static func privateIPv4(_ host: String) -> Bool {
      let octets = host.split(separator: ".", omittingEmptySubsequences: false).compactMap { UInt8($0) }
      guard octets.count == 4, host.split(separator: ".", omittingEmptySubsequences: false).count == 4 else { return false }
      guard octets.map(String.init).joined(separator: ".") == host else { return false }
      return octets[0] == 10 || (octets[0] == 172 && (16...31).contains(octets[1])) || (octets[0] == 192 && octets[1] == 168)
    }

    static func validTargetHost(_ host: String) -> Bool {
      guard !host.isEmpty, host == host.lowercased(), host != "localhost" else { return false }
      let parts = host.split(separator: ".", omittingEmptySubsequences: false)
      if parts.allSatisfy({ UInt8($0) != nil }) {
        let octets = parts.compactMap { UInt8($0) }
        return octets.count == 4 && octets.map(String.init).joined(separator: ".") == host
          && octets[0] != 0 && octets[0] != 127 && octets[0] < 224
      }
      return parts.count >= 2 && validDNSName(host)
    }

    static func validDNSName(_ name: String) -> Bool {
      name.count <= 253 && !name.isEmpty && name.split(separator: ".", omittingEmptySubsequences: false).allSatisfy {
        !$0.isEmpty && $0.utf8.count <= 63 && $0.utf8.allSatisfy { ($0 == 45 || (48...57).contains($0)) || (65...90).contains($0) || (97...122).contains($0) }
      }
    }
  }

  struct TargetManifest: Decodable {
    struct Endpoint: Decodable {
      let kind: String
      let host: String
      let port: UInt16
      let httpPath: String?
      let https: Bool?
      let dnsSuffix: String?
      let purpose: String
    }
    let schema: Int
    let expiresUnix: Double
    let requiredPath: String
    let endpoints: [Endpoint]

    func permits(_ spec: Spec, now: Double) -> Bool {
      schema == 1 && expiresUnix > now && ["ne_proxy", "ne_direct"].contains(requiredPath)
        && (1...8).contains(endpoints.count)
        && endpoints.contains { $0.kind == spec.kind && $0.host == spec.host && $0.port == spec.port
          && $0.httpPath == spec.httpPath && $0.https == spec.https
          && (spec.kind != "dns" || $0.dnsSuffix == spec.dnsName)
          && ($0.purpose == "fixture" || ($0.purpose == "h3_observer" && spec.kind == "http3"
            && spec.https == true && spec.count <= 3 && spec.concurrency == 1 && spec.perSecond == 1)) }
    }
  }

  private final class RunContext: @unchecked Sendable {
    let directory: URL
    let expectedLanes: Int
    private let lock = NSLock()
    private var reason: String?
    init(directory: URL, expectedLanes: Int) { self.directory = directory; self.expectedLanes = expectedLanes }
    func stop(_ value: String) { lock.lock(); defer { lock.unlock() }; if reason == nil { reason = value } }
    func stopped() -> String? { lock.lock(); defer { lock.unlock() }; return reason }
  }
  private static var active: [InternalLoadProbe] = [] // main queue only
  private static var settling: [InternalLoadProbe] = [] // bounded cancelled-callback accounting grace
  private static var preflightTimer: DispatchSourceTimer?
  private static var triggerTimer: DispatchSourceTimer?
  private static var triggerDeadline = 0.0
  private static var triggerGeneration = 0
  private static var exportNonces: [UUID] = []

  struct ExportCommand: Decodable {
    let schema: Int
    let action: String
    let triggerId: UUID
    let expiresUnix: Double
    func valid(now: Double) -> Bool { schema == 1 && action == "export" && expiresUnix > now && expiresUnix <= now + 300 }
  }

  static func decodeExportCommand(_ data: Data, now: Double) -> ExportCommand? {
    guard data.count <= 2048 else { return nil }
    let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
    guard let command = try? decoder.decode(ExportCommand.self, from: data), command.valid(now: now) else { return nil }
    return command
  }

  struct TriggerGate {
    private var observed: [String: String] = [:]
    private var claimed: [String: String] = [:]
    private var retryAfter: [String: Double] = [:]
    mutating func shouldAttempt(_ name: String, fingerprint: String, now: Double) -> Bool {
      guard claimed[name] != fingerprint else { return false }
      let changed = observed[name] != fingerprint
      observed[name] = fingerprint
      guard changed || now >= retryAfter[name, default: 0] else { return false }
      retryAfter[name] = now + 2
      return true
    }
    mutating func didClaim(_ name: String, fingerprint: String) { claimed[name] = fingerprint }
  }

  /// A bounded, metadata-only inbox check in the EXISTING Runner process.
  /// Startup/foreground can arm it; it never starts/stops/relaunches the VPN.
  static func armTriggerWindow() {
    let now = Date().timeIntervalSince1970
    if triggerTimer != nil && now < triggerDeadline { return }
    triggerTimer?.setEventHandler {}; triggerTimer?.cancel()
    guard let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first else { return }
    let base = documents.appendingPathComponent("PPVPNConcurrencyDiagnostics")
    try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
    triggerDeadline = now + 900
    triggerGeneration += 1
    let generation = triggerGeneration
    var gate = TriggerGate()
    let timer = DispatchSource.makeTimerSource(queue: .main)
    timer.schedule(deadline: .now(), repeating: .seconds(1), leeway: .milliseconds(100))
    func status(_ result: String, nonce: UUID? = nil, active: Bool = true) {
      var row: [String: Any] = ["schema": 1, "pid": ProcessInfo.processInfo.processIdentifier,
        "watcher_active": active, "window_expires_unix": triggerDeadline, "result": result,
        "unix_seconds": Date().timeIntervalSince1970]
      if let nonce { row["export_trigger_id"] = nonce.uuidString.lowercased() }
      if let bytes = try? JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]) {
        try? bytes.write(to: base.appendingPathComponent("trigger-status.json"), options: .atomic)
      }
    }
    timer.setEventHandler {
      guard generation == triggerGeneration else { return }
      if Date().timeIntervalSince1970 >= triggerDeadline {
        timer.setEventHandler {}; timer.cancel(); triggerTimer = nil
        status("window_expired", active: false); return
      }
      for name in ["request.json", "export-request.json"] {
        if name == "request.json" && !Self.active.isEmpty { continue }
        let path = base.appendingPathComponent(name)
        guard let attributes = try? FileManager.default.attributesOfItem(atPath: path.path),
          let size = attributes[.size] as? NSNumber, let modified = attributes[.modificationDate] as? Date else { continue }
        let fingerprint = "\(size)|\(modified.timeIntervalSince1970)|\(attributes[.systemFileNumber] ?? "")"
        guard gate.shouldAttempt(name, fingerprint: fingerprint, now: Date().timeIntervalSince1970) else { continue }
        if name == "request.json" {
          if consumeOnce(exportIdle: false) {
            gate.didClaim(name, fingerprint: fingerprint)
            status("request_claimed")
          } else { status("request_pending_or_rejected_see_consumed_marker") }
        } else {
          guard size.intValue <= 2048, let bytes = try? Data(contentsOf: path),
            let command = decodeExportCommand(bytes, now: Date().timeIntervalSince1970),
            !exportNonces.contains(command.triggerId),
            let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.com.peakpassvpn.ppvpn") else {
            status("export_rejected"); continue
          }
          do { try FileManager.default.removeItem(at: path) } catch { status("export_claim_failed"); continue }
          gate.didClaim(name, fingerprint: fingerprint)
          exportNonces.append(command.triggerId)
          if exportNonces.count > 32 { exportNonces.removeFirst() }
          exportRawSamples(sourceDirectory: container.appendingPathComponent("data/diagnostics"), destination: base)
          exportSelectionHint(container: container, destination: base)
          startPreflightMirror(base: base, source: container.appendingPathComponent("data/diagnostics/ne-latest.json"), restart: true)
          status("export_processed_check_raw_export_status", nonce: command.triggerId)
        }
      }
    }
    triggerTimer = timer
    status("window_armed")
    timer.resume()
  }
  private let spec: Spec
  private let directory: URL
  private let resourceSource: URL
  private let context: RunContext
  private let queue = DispatchQueue(label: "com.peakpassvpn.ppvpn.internal-load")
  private var timer: DispatchSourceTimer?
  private var session: URLSession!
  private var connections: [Int: NWConnection] = [:]
  private var tasks: [Int: URLSessionTask] = [:]
  private var taskOperations: [Int: Int] = [:]
  private var responseBytes: [Int: Int] = [:]
  private var protocols: [Int: String] = [:]
  private var starts: [Int: Double] = [:]
  private var started = 0.0
  private var offered = 0
  private var completed = 0
  private var failures: [String: Int] = [:]
  private var latencies: [Double] = []
  private var byteBudget: ByteBudget
  private var applicationBytes: Int { byteBudget.total }
  private var reservedBytes = 0
  private var tick = 0
  private var stopping: String?
  private var wroteResult = false
  private var expectedDNS: [Int: Data] = [:]
  private var sharedDNS: NWConnection?
  private var sharedDNSReady = false
  private var sentDNS: Set<Int> = []
  private var unexpectedDNS = 0
  private var consecutiveFailures = 0
  private var extensionPID: Int?
  private var lastResourceSequence: Int?
  private var activeRound: [Int: Int] = [:]
  private var initialSail: [String: Any]?
  private var latestSail: [String: Any]?
  private var pendingReceives = 0
  private var pendingHTTP: Set<Int> = []

  @discardableResult
  static func consumeOnce(exportIdle: Bool = true) -> Bool {
    guard active.isEmpty, let documents = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask).first,
      let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.com.peakpassvpn.ppvpn") else { return false }
    let base = documents.appendingPathComponent("PPVPNConcurrencyDiagnostics")
    try? FileManager.default.createDirectory(at: base, withIntermediateDirectories: true)
    try? Data("internal_release_entry_ready\n".utf8).write(to: base.appendingPathComponent("entry-ready.txt"), options: .atomic)
    if exportIdle {
      exportRawSamples(sourceDirectory: container.appendingPathComponent("data/diagnostics"), destination: base)
      exportSelectionHint(container: container, destination: base)
      startPreflightMirror(base: base, source: container.appendingPathComponent("data/diagnostics/ne-latest.json"))
    }
    let request = base.appendingPathComponent("request.json")
    guard let size = (try? request.resourceValues(forKeys: [.fileSizeKey]))?.fileSize, size <= 262144,
      let data = try? Data(contentsOf: request) else { return false }
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    guard let specs = decodeRequest(data, now: Date().timeIntervalSince1970), let spec = specs.first else {
      try? Data("invalid_or_expired_spec\n".utf8).write(to: base.appendingPathComponent("request-rejected.txt"), options: .atomic)
      return false
    }
    guard let manifestData = try? Data(contentsOf: base.appendingPathComponent("approved-targets.json")), manifestData.count <= 16384,
      SHA256.hash(data: manifestData).map({ String(format: "%02x", $0) }).joined() == spec.targetManifestSha256,
      let manifest = try? decoder.decode(TargetManifest.self, from: manifestData),
      specs.allSatisfy({ manifest.permits($0, now: Date().timeIntervalSince1970) }) else {
      try? Data("missing_mismatched_or_expired_target_manifest\n".utf8).write(to: base.appendingPathComponent("request-rejected.txt"), options: .atomic)
      return false
    }
    let directory = base.appendingPathComponent(spec.runId)
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let consumed = directory.appendingPathComponent("consumed.txt")
    guard !FileManager.default.fileExists(atPath: consumed.path) else { return false }
    let claimed = directory.appendingPathComponent("claimed-request.json")
    do {
      try FileManager.default.moveItem(at: request, to: claimed)
      try Data("consumed\n".utf8).write(to: consumed, options: .atomic)
      try FileManager.default.removeItem(at: claimed)
    } catch { return false }
    let context = RunContext(directory: directory, expectedLanes: specs.count)
    active = specs.enumerated().map { index, spec in
      let lane = directory.appendingPathComponent("lane-\(index)-\(spec.kind)")
      try? FileManager.default.createDirectory(at: lane, withIntermediateDirectories: true)
      return InternalLoadProbe(spec: spec, directory: lane,
        resourceSource: container.appendingPathComponent("data/diagnostics/ne-latest.json"), context: context)
    }
    for probe in active { probe.queue.async { probe.start() } }
    return true
  }

  /// A whitelist of ONE persisted identifier, never an account/profile export.
  /// This is a saved UI selection hint, not proof of the running NE route.
  static func selectionHint(_ bytes: Data) -> String? {
    guard bytes.count <= 4096,
      let value = (try? JSONSerialization.jsonObject(with: bytes)) as? [String: Any],
      Set(value.keys) == Set(["node_id"]), let identifier = value["node_id"] as? String,
      (1...256).contains(identifier.utf8.count),
      identifier.utf8.allSatisfy({ (48...57).contains($0) || (65...90).contains($0)
        || (97...122).contains($0) || $0 == 45 || $0 == 95 }) else { return nil }
    return identifier
  }

  private static func exportSelectionHint(container: URL, destination: URL) {
    let source = container.appendingPathComponent("data/selection.json")
    var result: [String: Any] = ["schema": 1, "unix_seconds": Date().timeIntervalSince1970,
      "runner_pid": ProcessInfo.processInfo.processIdentifier,
      "app_build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "unknown",
      "source": "persisted_selection_not_runtime_route_proof", "status": "missing_or_unreadable"]
    if let metadata = try? source.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey]),
      metadata.isRegularFile == true, metadata.isSymbolicLink == false,
      let size = metadata.fileSize, size <= 4096,
      let file = try? FileHandle(forReadingFrom: source) {
      defer { try? file.close() }
      if let bytes = try? file.read(upToCount: 4097), let identifier = selectionHint(bytes) {
        result["status"] = "copied_identifier_only"
        result["selected_node_id"] = identifier
      } else { result["status"] = "invalid_identifier_schema" }
    }
    if let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
      try? data.write(to: destination.appendingPathComponent("node-selection-hint.json"), options: .atomic)
    }
  }

  /// Copy ONLY the two bounded sampler streams via the app's real Group URL.
  /// On a later in-process export this also retrieves >5min ordinary-UDP
  /// drain history; it does not require Runner to stay alive during that wait.
  private static func exportRawSamples(sourceDirectory: URL, destination: URL) {
    var files: [String: Any] = [:]
    for name in ["ne-resources.jsonl", "ne-resources.previous.jsonl"] {
      let output = destination.appendingPathComponent(name)
      guard let file = try? FileHandle(forReadingFrom: sourceDirectory.appendingPathComponent(name)) else {
        try? FileManager.default.removeItem(at: output)
        files[name] = ["status": "missing_or_unreadable"]
        continue
      }
      defer { try? file.close() }
      do {
        var data = try file.read(upToCount: 2 * 1024 * 1024) ?? Data()
        // Concurrent append/rotation is permitted. Preserve complete JSONL
        // records only and report export time rather than claiming an atomic
        // cross-file snapshot.
        if let newline = data.lastIndex(of: 10) { data = data.prefix(through: newline) }
        else { data = Data() }
        try data.write(to: output, options: .atomic)
        files[name] = ["status": "copied_complete_records", "bytes": data.count]
      } catch { files[name] = ["status": "copy_failed"] }
    }
    let value: [String: Any] = ["schema": 1, "unix_seconds": Date().timeIntervalSince1970,
      "nontransactional_export": true, "files": files]
    if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
      try? data.write(to: destination.appendingPathComponent("raw-export-status.json"), options: .atomic)
    }
  }

  /// Allows root to obtain idle extension samples BEFORE choosing explicit load
  /// stop thresholds. No socket or traffic is opened by this 120-second bridge.
  private static func startPreflightMirror(base: URL, source: URL, restart: Bool = false) {
    if restart { preflightTimer?.setEventHandler {}; preflightTimer?.cancel(); preflightTimer = nil }
    guard preflightTimer == nil else { return }
    let queue = DispatchQueue(label: "com.peakpassvpn.ppvpn.ne-resource-mirror", qos: .utility)
    let path = base.appendingPathComponent("preflight-ne-resources.jsonl")
    let previous = base.appendingPathComponent("preflight-ne-resources.previous.jsonl")
    if FileManager.default.fileExists(atPath: path.path) {
      try? FileManager.default.removeItem(at: previous)
      try? FileManager.default.moveItem(at: path, to: previous)
    }
    FileManager.default.createFile(atPath: path.path, contents: nil)
    var ticks = 0
    let timer = DispatchSource.makeTimerSource(queue: queue)
    timer.schedule(deadline: .now(), repeating: .seconds(1))
    timer.setEventHandler {
      ticks += 1
      if let data = try? Data(contentsOf: source), data.count <= 32768, let file = try? FileHandle(forWritingTo: path) {
        defer { try? file.close() }
        _ = try? file.seekToEnd(); try? file.write(contentsOf: data + Data([10]))
      } else {
        let value: [String: Any] = ["unix_seconds": Date().timeIntervalSince1970, "ne_sample": "missing_or_unreadable", "app_group_resolved": true]
        if let data = try? JSONSerialization.data(withJSONObject: value) { try? data.write(to: base.appendingPathComponent("preflight-status.json"), options: .atomic) }
      }
      if ticks >= 120 { timer.setEventHandler {}; timer.cancel() }
    }
    preflightTimer = timer
    timer.resume()
  }

  /// Single lane or a simultaneous mix. Global guards/identity cannot be
  /// overridden by a lane, and aggregate offering/byte limits are bounded.
  static func decodeRequest(_ data: Data, now: Double) -> [Spec]? {
    guard var common = (try? JSONSerialization.jsonObject(with: data)) as? [String: Any] else { return nil }
    let decoder = JSONDecoder(); decoder.keyDecodingStrategy = .convertFromSnakeCase
    let rows: [[String: Any]]
    if let workloads = common.removeValue(forKey: "workloads") as? [[String: Any]] {
      guard (1...4).contains(workloads.count) else { return nil }
      let protected = Set(["run_token", "run_id", "expires_unix", "schema", "target_manifest_sha256", "min_available_bytes", "max_footprint_bytes", "duration_seconds"])
      guard workloads.allSatisfy({ protected.isDisjoint(with: $0.keys) }) else { return nil }
      rows = workloads.map { common.merging($0) { _, new in new } }
    } else { rows = [common] }
    var specs: [Spec] = []
    for row in rows {
      guard let bytes = try? JSONSerialization.data(withJSONObject: row),
        let spec = try? decoder.decode(Spec.self, from: bytes), spec.validate(now: now) else { return nil }
      specs.append(spec)
    }
    guard specs.reduce(0, { $0 + $1.count }) <= 2048, specs.reduce(0, { $0 + $1.concurrency }) <= 256,
      specs.reduce(0, { $0 + $1.perSecond }) <= 400,
      specs.reduce(0, { $0 + $1.maxApplicationBytes }) <= 8 * 1024 * 1024 else { return nil }
    return specs
  }

  private init(spec: Spec, directory: URL, resourceSource: URL, context: RunContext) {
    self.spec = spec; self.directory = directory; self.resourceSource = resourceSource; self.context = context
    self.byteBudget = ByteBudget(limit: spec.maxApplicationBytes)
  }

  private func accountBytes(_ count: Int, sending: Bool) -> Bool {
    if sending, let reason = context.stopped() { stop(reason); return false }
    let accepted = byteBudget.account(count, sending: sending)
    if !accepted { stop("application_byte_limit") }
    if let reason = context.stopped() { stop(reason) }
    if wroteResult { writeByteAccounting() }
    return accepted && stopping == nil
  }

  private func writeByteAccounting() {
    let value: [String: Any] = ["schema": 1, "sent_application_bytes": byteBudget.sent,
      "received_application_bytes": byteBudget.received, "application_bytes": byteBudget.total,
      "limit_bytes": byteBudget.limit, "observed_overshoot_bytes": byteBudget.overshoot,
      "pending_network_receives": pendingReceives, "pending_http_tasks": pendingHTTP.count,
      "callback_accounting_complete": pendingReceives == 0 && pendingHTTP.isEmpty,
      "udp_tcp_pending_callback_allowance_bytes": pendingReceives * (spec.kind == "tcp" ? 4096 : 65535),
      "http_pending_allowance_bytes": pendingHTTP.isEmpty ? 0 : NSNull(),
      "overshoot_note": "already-issued callbacks may arrive after cancellation; UDP charged whole up to65535 per pending read; TCP read<=4096; HTTP declared body<=4096 but each delivered callback charged whole; incomplete accounting is not a PASS; this is not a wire-byte cap" ]
    if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
      try? data.write(to: directory.appendingPathComponent("byte-accounting.json"), options: .atomic)
    }
  }

  private func start() {
    started = ProcessInfo.processInfo.systemUptime
    let configuration = URLSessionConfiguration.ephemeral
    configuration.httpCookieStorage = nil
    configuration.urlCache = nil
    configuration.urlCredentialStorage = nil
    configuration.requestCachePolicy = .reloadIgnoringLocalCacheData
    configuration.timeoutIntervalForRequest = 10
    configuration.timeoutIntervalForResource = 12
    configuration.httpMaximumConnectionsPerHost = min(spec.concurrency, 128)
    let delegates = OperationQueue()
    delegates.maxConcurrentOperationCount = 1
    session = URLSession(configuration: configuration, delegate: self, delegateQueue: delegates)
    let timer = DispatchSource.makeTimerSource(queue: queue)
    timer.schedule(deadline: .now(), repeating: .milliseconds(100))
    timer.setEventHandler { [weak self] in self?.step() }
    self.timer = timer
    timer.resume()
  }

  private func writeProgress() {
    let value: [String: Any] = ["schema": 1, "run_id": spec.runId, "kind": spec.kind,
      "pid": ProcessInfo.processInfo.processIdentifier, "ne_pid": extensionPID as Any? ?? NSNull(),
      "unix_seconds": Date().timeIntervalSince1970, "elapsed_seconds": ProcessInfo.processInfo.systemUptime - started,
      "offered": offered, "completed": completed, "failed_by_cause": failures,
      "outstanding": starts.count, "application_bytes": applicationBytes,
      "admitted": NSNull(), "expected_session_limit": Self.expectedSessionLimit,
      "stop_reason": stopping as Any? ?? NSNull()]
    if let data = try? JSONSerialization.data(withJSONObject: value, options: [.sortedKeys]) {
      try? data.write(to: directory.appendingPathComponent("progress.json"), options: .atomic)
    }
  }

  private func step() {
    let now = ProcessInfo.processInfo.systemUptime
    if let reason = context.stopped() { stop(reason) }
    if now - started >= Double(spec.durationSeconds) { stop("duration_limit") }
    if FileManager.default.fileExists(atPath: context.directory.appendingPathComponent("STOP").path) { stop("operator_stop") }
    if tick % 10 == 0, stopping == nil {
      guard let data = try? Data(contentsOf: resourceSource),
        let sample = try? JSONSerialization.jsonObject(with: data) as? [String: Any],
        let at = sample["unix_seconds"] as? Double, abs(Date().timeIntervalSince1970 - at) < 5,
        sample["final"] as? Bool == false,
        let available = sample["available_memory_bytes"] as? NSNumber,
        let footprint = sample["phys_footprint_bytes"] as? NSNumber,
        let pid = sample["pid"] as? Int else { stop("no_fresh_ne_resource_sample"); return }
      if let previous = extensionPID, previous != pid { stop("ne_process_changed"); return }
      extensionPID = pid
      guard let sail = sample["sail"] as? [String: Any],
        (sail["schema_version"] as? NSNumber)?.intValue == 1,
        let epoch = sail["epoch"] as? NSNumber, let instance = sail["instance_id"] as? NSNumber,
        let sessions = sail["sessions"] as? [String: Any],
        Self.hasExpectedSessionLimit(sessions),
        sessions["live"] is NSNumber, sessions["waiting"] is NSNumber,
        sessions["admitted_total"] is NSNumber, sessions["refused_total"] is NSNumber else {
        stop("missing_current_cap256_sail_snapshot"); return
      }
      if let initialEpoch = initialSail?["epoch"] as? NSNumber, initialEpoch != epoch { stop("sail_dispatcher_changed"); return }
      if let initialInstance = initialSail?["instance_id"] as? NSNumber, initialInstance != instance { stop("sail_instance_changed"); return }
      if initialSail == nil { initialSail = sail }
      latestSail = sail
      if let sequence = sample["sequence"] as? Int, lastResourceSequence != sequence {
        lastResourceSequence = sequence
        let mirror = directory.appendingPathComponent("ne-resources.jsonl")
        if !FileManager.default.fileExists(atPath: mirror.path) { FileManager.default.createFile(atPath: mirror.path, contents: nil) }
        if let file = try? FileHandle(forWritingTo: mirror) {
          defer { try? file.close() }
          _ = try? file.seekToEnd(); try? file.write(contentsOf: data + Data([10]))
        }
      }
      if available.uint64Value < spec.minAvailableBytes || footprint.uint64Value >= spec.maxFootprintBytes { stop("ne_resource_guard") }
    }
    tick += 1
    if tick % 10 == 1 { writeProgress() }
    for (id, at) in Array(starts) where now - at >= Double(spec.operationTimeoutSeconds) { finish(id, failure: "operation_timeout") }
    if stopping == nil {
      let allowed = min(spec.count, Int((now - started) * Double(spec.perSecond)) + 1)
      while starts.count < spec.concurrency && offered < allowed {
        // Reserve the maximum application payload before offering work. Wire
        // headers/retransmits are not counted as application bytes.
        let reservation = (spec.kind == "dns" || spec.kind == "http" || spec.kind == "http3") ? 8192 : spec.payloadBytes * 2 * spec.rounds + 4096
        guard reservedBytes + reservation <= spec.maxApplicationBytes else { stop("application_byte_reservation_limit"); break }
        reservedBytes += reservation
        let id = offered
        offered += 1
        starts[id] = now
        launch(id)
      }
    }
    if starts.isEmpty && (offered == spec.count || stopping != nil) { writeResult() }
  }

  private func launch(_ id: Int) {
    if spec.kind == "http" || spec.kind == "http3" {
      var components = URLComponents()
      components.scheme = spec.https == true ? "https" : "http"
      components.host = spec.host; components.port = Int(spec.port); components.path = spec.httpPath ?? "/"
      guard let url = components.url else { finish(id, failure: "invalid_url"); return }
      var request = URLRequest(url: url)
      if spec.kind == "http" { request.setValue(spec.runToken.lowercased(), forHTTPHeaderField: "X-PPVPN-Run") }
      request.setValue("bytes=0-1023", forHTTPHeaderField: "Range")
      if spec.kind == "http3" { request.assumesHTTP3Capable = true }
      let task = session.dataTask(with: request)
      tasks[id] = task; taskOperations[task.taskIdentifier] = id
      pendingHTTP.insert(task.taskIdentifier)
      task.resume()
      return
    }
    let payload: Data
    if spec.kind == "dns" {
      payload = dnsQuery(id)
      expectedDNS[id] = payload
    } else {
      activeRound[id] = 0
      payload = echoPayload(id, round: 0)
    }
    if spec.kind == "dns" && spec.dnsSharedSocket == true {
      if sharedDNS == nil { startSharedDNS() }
      sendPendingDNS()
      return
    }
    let connection = NWConnection(host: NWEndpoint.Host(spec.host), port: NWEndpoint.Port(rawValue: spec.port)!, using: spec.kind == "tcp" ? .tcp : .udp)
    connections[id] = connection
    connection.stateUpdateHandler = { [weak self, weak connection] state in
      guard let self, let connection, self.starts[id] != nil else { return }
      switch state {
      case .ready:
        self.sendChunks(id, connection: connection, payload: payload, chunk: 0)
      case .failed(let error): self.finish(id, failure: Self.networkFailure(error, phase: "connect"))
      default: break
      }
    }
    connection.start(queue: queue)
  }

  private func echoPayload(_ id: Int, round: Int) -> Data {
    Self.makeEchoPayload(token: spec.runToken, id: id, round: round, count: spec.payloadBytes)
  }

  static func makeEchoPayload(token: String, id: Int, round: Int, count: Int) -> Data {
    let header = [UInt8]("PPVPN1 \(token.lowercased()) \(id * 8 + round)\n".utf8)
    return Data(header + Array(repeating: UInt8(0x5a), count: count - header.count))
  }

  static func chunkRange(payload: Data, chunks: Int, chunk: Int) -> Range<Int> {
    let headerLength = (payload.firstIndex(of: 10) ?? 0) + 1
    let firstLength = max(headerLength, payload.count / chunks)
    let lower = chunk == 0 ? 0 : firstLength + (payload.count - firstLength) * (chunk - 1) / max(1, chunks - 1)
    let upper = chunk == 0 ? (chunks == 1 ? payload.count : firstLength) : firstLength + (payload.count - firstLength) * chunk / max(1, chunks - 1)
    return lower..<upper
  }

  private static func networkFailure(_ error: NWError, phase: String) -> String {
    switch error {
    case .posix(let code): return "\(phase)_posix_\(code.rawValue)"
    case .dns(let code): return "\(phase)_dns_\(code)"
    case .tls(let code): return "\(phase)_tls_\(code)"
    default: return "\(phase)_unknown"
    }
  }

  private func sendChunks(_ id: Int, connection: NWConnection, payload: Data, chunk: Int) {
    guard starts[id] != nil else { return }
    // The fixture authenticates the first line before echoing any bytes.
    let range = Self.chunkRange(payload: payload, chunks: spec.writeChunks, chunk: chunk)
    guard accountBytes(range.count, sending: true) else { return }
    connection.send(content: payload.subdata(in: range), completion: .contentProcessed { [weak self, weak connection] error in
      guard let self, let connection, self.starts[id] != nil else { return }
      if let error { self.finish(id, failure: Self.networkFailure(error, phase: "send")); return }
      if chunk + 1 < self.spec.writeChunks {
        self.queue.asyncAfter(deadline: .now() + .milliseconds(self.spec.writeIntervalMs)) { [weak self, weak connection] in
          guard let self, let connection else { return }
          self.sendChunks(id, connection: connection, payload: payload, chunk: chunk + 1)
        }
      } else {
        self.queue.asyncAfter(deadline: .now() + .milliseconds(self.spec.readDelayMs)) { [weak self, weak connection] in
          guard let self, let connection, self.starts[id] != nil else { return }
          self.receive(id, connection: connection, expected: payload, accumulated: Data())
        }
      }
    })
  }

  private func startSharedDNS() {
    let connection = NWConnection(host: NWEndpoint.Host(spec.host), port: NWEndpoint.Port(rawValue: spec.port)!, using: .udp)
    sharedDNS = connection
    connection.stateUpdateHandler = { [weak self] state in
      guard let self else { return }
      switch state {
      case .ready: self.sharedDNSReady = true; self.sendPendingDNS(); self.receiveSharedDNS()
      case .failed:
        for id in Array(self.starts.keys) { self.finish(id, failure: "dns_shared_connect_error") }
      default: break
      }
    }
    connection.start(queue: queue)
  }

  private func sendPendingDNS() {
    guard sharedDNSReady, let connection = sharedDNS else { return }
    for (id, data) in Array(expectedDNS) where !sentDNS.contains(id) {
      guard accountBytes(data.count, sending: true) else { return }
      sentDNS.insert(id)
      connection.send(content: data, completion: .contentProcessed { [weak self] error in
        if error != nil { self?.finish(id, failure: "dns_shared_send_error") }
      })
    }
  }

  private func receiveSharedDNS() {
    guard let connection = sharedDNS, stopping == nil, !wroteResult else { return }
    if let reason = context.stopped() { stop(reason); return }
    guard byteBudget.remaining > 0 else { stop("application_byte_limit"); return }
    pendingReceives += 1
    connection.receiveMessage { [weak self] data, _, _, error in
      guard let self else { return }
      self.pendingReceives -= 1
      defer { if self.wroteResult { self.writeByteAccounting() } }
      if let data {
        let packet = Self.accountSharedDNS(data, pending: Set(self.expectedDNS.keys), budget: &self.byteBudget)
        if let reason = self.context.stopped() { self.stop(reason); return }
        switch packet {
        case .budgetExceeded: self.stop("application_byte_limit"); return
        case .unexpected: self.unexpectedDNS += 1
        case .reply(let id):
          if self.stopping == nil, let expected = self.expectedDNS[id] {
            self.finish(id, failure: Self.dnsFailure(data, query: expected, expectedRcode: self.spec.dnsExpectedRcode!, expectedIPv4: self.spec.dnsExpectedIpv4 ?? []))
          }
        }
      }
      if let reason = self.context.stopped() { self.stop(reason) }
      guard self.stopping == nil, !self.wroteResult else { return }
      if error != nil {
        for id in Array(self.starts.keys) { self.finish(id, failure: "dns_shared_receive_error") }
      } else { self.receiveSharedDNS() }
    }
  }

  private func receive(_ id: Int, connection: NWConnection, expected: Data, accumulated: Data) {
    guard stopping == nil else { return }
    if let reason = context.stopped() { stop(reason); return }
    guard byteBudget.remaining > 0 else { stop("application_byte_limit"); return }
    // Never truncate a legal60000-byte UDP echo to fit a budget. TCP is a
    // stream and can safely bound each read to remaining payload/budget.
    let maximum = spec.kind == "tcp" ? min(4096, byteBudget.remaining, max(1, expected.count - accumulated.count)) : 65535
    pendingReceives += 1
    connection.receive(minimumIncompleteLength: 1, maximumLength: maximum) { [weak self, weak connection] data, _, done, error in
      guard let self else { return }
      self.pendingReceives -= 1
      defer { if self.wroteResult { self.writeByteAccounting() } }
      guard self.accountBytes(data?.count ?? 0, sending: false), let connection, self.starts[id] != nil else { return }
      let answer = accumulated + (data ?? Data())
      if answer.count > max(4096, expected.count) { self.finish(id, failure: "response_limit"); return }
      if self.spec.kind == "dns" { self.finish(id, failure: Self.dnsFailure(answer, query: expected, expectedRcode: self.spec.dnsExpectedRcode!, expectedIPv4: self.spec.dnsExpectedIpv4 ?? [])); return }
      if answer == expected {
        let round = self.activeRound[id, default: 0] + 1
        if round < self.spec.rounds {
          self.activeRound[id] = round
          self.queue.asyncAfter(deadline: .now() + .milliseconds(self.spec.roundIntervalMs)) { [weak self, weak connection] in
            guard let self, let connection, self.starts[id] != nil else { return }
            self.sendChunks(id, connection: connection, payload: self.echoPayload(id, round: round), chunk: 0)
          }
        } else { self.finish(id, failure: nil) }
      }
      else if let error { self.finish(id, failure: Self.networkFailure(error, phase: "receive")) }
      else if done || self.spec.kind == "udp" || answer.count >= expected.count { self.finish(id, failure: "echo_mismatch_or_early_eof") }
      else { self.receive(id, connection: connection, expected: expected, accumulated: answer) }
    }
  }

  private func finish(_ id: Int, failure: String?) {
    guard let at = starts.removeValue(forKey: id) else { return }
    connections.removeValue(forKey: id)?.cancel()
    if let task = tasks.removeValue(forKey: id) { taskOperations.removeValue(forKey: task.taskIdentifier); responseBytes.removeValue(forKey: task.taskIdentifier); protocols.removeValue(forKey: task.taskIdentifier); task.cancel() }
    expectedDNS.removeValue(forKey: id)
    sentDNS.remove(id)
    activeRound.removeValue(forKey: id)
    if let failure {
      failures[failure, default: 0] += 1
      consecutiveFailures += 1
      if consecutiveFailures >= spec.maxConsecutiveFailures { stop("consecutive_failure_guard") }
    } else { consecutiveFailures = 0; completed += 1; latencies.append((ProcessInfo.processInfo.systemUptime - at) * 1000) }
  }

  private func stop(_ reason: String) {
    guard stopping == nil else { return }
    stopping = reason
    context.stop(reason)
    sharedDNS?.cancel()
    for id in Array(starts.keys) { finish(id, failure: "cancelled_\(reason)") }
  }

  private func writeResult() {
    guard !wroteResult else { return }
    wroteResult = true
    timer?.cancel(); timer = nil
    session.invalidateAndCancel()
    sharedDNS?.cancel(); sharedDNS = nil
    writeByteAccounting()
    let sorted = latencies.sorted()
    func percentile(_ q: Double) -> Any { sorted.isEmpty ? NSNull() : sorted[min(sorted.count - 1, Int(Double(sorted.count - 1) * q))] }
    let firstPool = initialSail?["sessions"] as? [String: Any]
    let lastPool = latestSail?["sessions"] as? [String: Any]
    func delta(_ key: String) -> Any {
      guard let before = firstPool?[key] as? NSNumber, let after = lastPool?[key] as? NSNumber,
        after.uint64Value >= before.uint64Value else { return NSNull() }
      return after.uint64Value - before.uint64Value
    }
    let result: [String: Any] = ["schema": 1, "run_id": spec.runId, "kind": spec.kind,
      "offered": offered, "completed": completed, "failed_by_cause": failures,
      "outstanding": starts.count, "admitted": NSNull(), "admitted_note": "logical_operations_not_one_to_one_with_Sail_permit_grants",
      "sail_first_snapshot": initialSail as Any? ?? NSNull(), "sail_last_snapshot": latestSail as Any? ?? NSNull(),
      "sail_admitted_counter_window_delta": delta("admitted_total"), "sail_refused_counter_window_delta": delta("refused_total"),
      "sail_counter_note": "periodic_current_pool_samples_nontransactional_includes_other_application_traffic_not_per_lane_attribution",
      "dns_shared_socket": spec.dnsSharedSocket == true, "dns_unexpected_or_late_id": unexpectedDNS,
      "dns_name_mode": spec.dnsNameMode as Any? ?? NSNull(), "rounds_per_echo_connection": spec.rounds,
      "target_manifest_sha256": spec.targetManifestSha256, "path_proof": "UNVERIFIED_requires_server_source_and_NE_route",
      "application_bytes": applicationBytes, "reserved_application_bytes": reservedBytes,
      "byte_accounting_file": "byte-accounting.json", "observed_overshoot_bytes": byteBudget.overshoot,
      "elapsed_seconds": ProcessInfo.processInfo.systemUptime - started,
      "successful_latency_ms": ["p50": percentile(0.5), "p95": percentile(0.95), "p99": percentile(0.99)],
      "stop_reason": stopping ?? "count_complete", "transport": "Runner_normal_sockets_NE_path_requires_independent_confirmation"]
    if let data = try? JSONSerialization.data(withJSONObject: result, options: [.sortedKeys]) {
      try? data.write(to: directory.appendingPathComponent("result.json"), options: .atomic)
    }
    // Retain the consumed marker; only a NEW random token may run again.
    let parent = context.directory
    let callbacksPending = pendingReceives > 0 || !pendingHTTP.isEmpty
    DispatchQueue.main.async {
      if callbacksPending {
        // Cancelled sockets may still deliver their one already-issued read.
        // Account those callbacks without keeping any traffic alive. If the OS
        // never settles them, the accounting file explicitly stays incomplete.
        Self.settling.append(self)
        DispatchQueue.main.asyncAfter(deadline: .now() + 2) { Self.settling.removeAll { $0 === self } }
      }
      Self.active.removeAll { $0 === self }
      if Self.active.isEmpty {
        Self.exportRawSamples(sourceDirectory: self.resourceSource.deletingLastPathComponent(), destination: parent)
        let files = (try? FileManager.default.contentsOfDirectory(at: parent, includingPropertiesForKeys: nil)) ?? []
        let results: [[String: Any]] = files.filter { $0.lastPathComponent.hasPrefix("lane-") }.compactMap {
          guard let data = try? Data(contentsOf: $0.appendingPathComponent("result.json")) else { return nil }
          return (try? JSONSerialization.jsonObject(with: data)) as? [String: Any]
        }
        let summary: [String: Any] = ["schema": 1, "run_id": self.spec.runId, "lanes": results,
          "expected_lanes": self.context.expectedLanes, "all_lane_results_present": results.count == self.context.expectedLanes,
          "path_proof": "UNVERIFIED_requires_server_source_and_NE_route",
          "note": "application operation counts are not Sail admitted sessions"]
        if let data = try? JSONSerialization.data(withJSONObject: summary, options: [.sortedKeys]) {
          try? data.write(to: parent.appendingPathComponent("summary.json"), options: .atomic)
        }
      }
    }
  }

  private func dnsQuery(_ id: Int) -> Data {
    Self.makeDNSQuery(id, runId: spec.runId, name: spec.dnsName!, mode: spec.dnsNameMode!)
  }

  static func makeDNSQuery(_ id: Int, runId: String, name: String, mode: String) -> Data {
    var bytes: [UInt8] = [UInt8(id >> 8), UInt8(id & 255), 1, 0, 0, 1, 0, 0, 0, 0, 0, 0]
    let name = mode == "per_id" ? "r\(runId.prefix(8).lowercased())-\(id).\(name)" : name
    for label in name.split(separator: ".") { bytes.append(UInt8(label.utf8.count)); bytes.append(contentsOf: label.utf8) }
    bytes.append(contentsOf: [0, 0, 1, 0, 1])
    return Data(bytes)
  }

  static func dnsFailure(_ answer: Data, query: Data, expectedRcode: UInt8, expectedIPv4: [String]) -> String? {
    let bytes = [UInt8](answer), request = [UInt8](query)
    guard request.count >= 17, bytes.count >= request.count, bytes.prefix(2) == request.prefix(2), bytes[2] & 0x80 != 0,
      bytes[2] & 0x02 == 0, bytes[4] == 0, bytes[5] == 1,
      bytes[12..<request.count] == request[12..<request.count], bytes[3] & 15 == expectedRcode else { return "dns_id_question_rcode_or_truncated" }
    if expectedRcode == 3 { return wordAt(bytes, 6) == 0 ? nil : "dns_nxdomain_with_answer" }
    var offset = request.count
    var ips: Set<String> = []
    func word(_ at: Int) -> Int { Int(bytes[at]) << 8 | Int(bytes[at + 1]) }
    for _ in 0..<word(6) {
      var compressed = false
      while offset < bytes.count && bytes[offset] != 0 {
        if bytes[offset] & 0xc0 == 0xc0 {
          guard offset + 1 < bytes.count else { return "dns_malformed" }
          let target = (Int(bytes[offset] & 0x3f) << 8) | Int(bytes[offset + 1])
          guard target >= 12 && target < offset else { return "dns_malformed" }
          offset += 2; compressed = true; break
        }
        let length = Int(bytes[offset]); guard length <= 63 else { return "dns_malformed" }
        offset += length + 1
      }
      if !compressed && offset < bytes.count && bytes[offset] == 0 { offset += 1 }
      guard offset + 10 <= bytes.count else { return "dns_malformed" }
      let type = word(offset), recordClass = word(offset + 2), length = word(offset + 8)
      offset += 10
      guard offset + length <= bytes.count else { return "dns_malformed" }
      if type == 1 && recordClass == 1 && length == 4 { ips.insert(bytes[offset..<offset+4].map(String.init).joined(separator: ".")) }
      offset += length
    }
    return ips == Set(expectedIPv4) ? nil : "dns_answer_mismatch"
  }

  private static func wordAt(_ bytes: [UInt8], _ at: Int) -> Int { Int(bytes[at]) << 8 | Int(bytes[at + 1]) }

  func urlSession(_ session: URLSession, task: URLSessionTask, didFinishCollecting metrics: URLSessionTaskMetrics) {
    queue.async { self.protocols[task.taskIdentifier] = metrics.transactionMetrics.last?.networkProtocolName ?? "unknown" }
  }

  func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive data: Data) {
    queue.async {
      guard self.accountBytes(data.count, sending: false) else { return }
      guard let id = self.taskOperations[dataTask.taskIdentifier] else { return }
      self.responseBytes[dataTask.taskIdentifier, default: 0] += data.count
      if self.responseBytes[dataTask.taskIdentifier, default: 0] > 4096 { self.finish(id, failure: "response_limit") }
    }
  }

  func urlSession(_ session: URLSession, dataTask: URLSessionDataTask, didReceive response: URLResponse, completionHandler: @escaping (URLSession.ResponseDisposition) -> Void) {
    // A reviewed fixture must advertise a small bounded response. Reject
    // unknown/large bodies before receiving them, even if it ignores Range.
    if (0...4096).contains(response.expectedContentLength) { completionHandler(.allow) }
    else {
      queue.async {
        if let id = self.taskOperations[dataTask.taskIdentifier] { self.finish(id, failure: "http_unbounded_response") }
        completionHandler(.cancel)
      }
    }
  }

  func urlSession(_ session: URLSession, task: URLSessionTask, didCompleteWithError error: Error?) {
    queue.async {
      self.pendingHTTP.remove(task.taskIdentifier)
      defer { if self.wroteResult { self.writeByteAccounting() } }
      guard let id = self.taskOperations[task.taskIdentifier] else { return }
      if let error = error as NSError? { self.finish(id, failure: "url_error_\(error.code)") }
      else if self.spec.kind == "http3" && self.protocols[task.taskIdentifier] != "h3" { self.finish(id, failure: "http3_not_observed") }
      else if let response = task.response as? HTTPURLResponse, (200..<300).contains(response.statusCode) { self.finish(id, failure: nil) }
      else { self.finish(id, failure: "http_status") }
    }
  }

  func urlSession(_ session: URLSession, task: URLSessionTask, willPerformHTTPRedirection response: HTTPURLResponse, newRequest request: URLRequest, completionHandler: @escaping (URLRequest?) -> Void) { completionHandler(nil) }
}
#endif
