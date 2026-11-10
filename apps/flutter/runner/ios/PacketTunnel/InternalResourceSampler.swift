#if PPVPN_INTERNAL_DIAGNOSTICS
import Foundation
import Darwin
import os

/// Internal Release instrumentation. No observer, timer or file exists in an
/// ordinary build. Samples the NE process itself, never the containing app.
final class InternalResourceSampler {
  private let queue = DispatchQueue(label: "com.peakpassvpn.ppvpn.resource-sample", qos: .utility)
  private let directory: URL
  private let epoch: UInt64
  private var timer: DispatchSourceTimer?
  private var sequence = 0
  private var fd: [String: Any] = [:]
  private var closed = false

  init(directory: URL, epoch: UInt64) {
    self.directory = directory
    self.epoch = epoch
  }

  func start() {
    queue.async { [self] in
      guard !closed, timer == nil else { return }
      try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
      let source = DispatchSource.makeTimerSource(queue: queue)
      source.schedule(deadline: .now(), repeating: .seconds(1), leeway: .milliseconds(100))
      source.setEventHandler { [weak self] in self?.sample() }
      timer = source
      source.resume()
    }
  }

  func stop() {
    queue.async { [self] in
      closed = true
      timer?.cancel()
      timer = nil
      sample(final: true)
    }
  }

  private func sample(final: Bool = false) {
    let started = ProcessInfo.processInfo.systemUptime
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
    let capacity = Int(count)
    let status = withUnsafeMutablePointer(to: &info) { pointer in
      pointer.withMemoryRebound(to: integer_t.self, capacity: capacity) {
        task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
      }
    }
    if sequence % 5 == 0 { fd = Self.descriptors() }
    var row: [String: Any] = [
      "schema": 1, "kind": "ne_resource", "pid": getpid(), "epoch": epoch,
      "sequence": sequence, "unix_seconds": Date().timeIntervalSince1970,
      "uptime_seconds": started, "final": final,
      "task_info_status": status, "fd": fd,
      "packet_transport": TunnelHost.packetDiagnostics(),
      "available_memory_bytes": os_proc_available_memory(),
      "sample_elapsed_ms": (ProcessInfo.processInfo.systemUptime - started) * 1000,
      // Explicit nulls: these are NOT approximated from Engine.connections().
      "sessions_live": NSNull(), "sessions_waiting": NSNull(),
      "dns_carriers": NSNull(), "sail_metrics": "not_exposed_by_frozen_sail"
    ]
    if status == KERN_SUCCESS {
      row["phys_footprint_bytes"] = info.phys_footprint
      row["resident_size_bytes"] = info.resident_size
    }
    sequence += 1
    guard let data = try? JSONSerialization.data(withJSONObject: row, options: [.sortedKeys]) else { return }
    // One bounded stream plus an atomic latest sample for the load stop guard.
    try? data.write(to: directory.appendingPathComponent("ne-latest.json"), options: .atomic)
    let path = directory.appendingPathComponent("ne-resources.jsonl")
    if let size = (try? path.resourceValues(forKeys: [.fileSizeKey]))?.fileSize, size >= 2 * 1024 * 1024 {
      let previous = directory.appendingPathComponent("ne-resources.previous.jsonl")
      try? FileManager.default.removeItem(at: previous)
      try? FileManager.default.moveItem(at: path, to: previous)
    }
    if !FileManager.default.fileExists(atPath: path.path) { FileManager.default.createFile(atPath: path.path, contents: nil) }
    if let file = try? FileHandle(forWritingTo: path) {
      defer { try? file.close() }
      do { try file.seekToEnd(); try file.write(contentsOf: data + Data([10])) } catch { /* leave a missing sample, never block traffic */ }
    }
  }

  private static func descriptors() -> [String: Any] {
    var limit = rlimit()
    guard getrlimit(RLIMIT_NOFILE, &limit) == 0 else { return ["error": "getrlimit", "errno": errno] }
    let ceiling = min(UInt64(limit.rlim_cur), 8192)
    var open = 0
    for value in 0..<Int32(ceiling) {
      if fcntl(value, F_GETFD) != -1 || errno != EBADF { open += 1 }
    }
    return ["count": open, "scanned_below": ceiling, "soft_limit": UInt64(limit.rlim_cur),
      "truncated": UInt64(limit.rlim_cur) > ceiling, "sample_uptime": ProcessInfo.processInfo.systemUptime]
  }
}
#endif
