extension PacketTunnelProvider {
  func barrierForTest() { queue.sync {} }
  func pausePumpForTest() { queue.sync { outputTimer?.cancel(); outputTimer = nil } }
  func pumpForTest() { queue.sync { if let generation = packetGeneration { writePackets(generation: generation) } } }
}
final class ResultBox { var value = false }
func start(_ provider: PacketTunnelProvider) {
  let done = DispatchSemaphore(value: 0)
  provider.startTunnel(options: nil) { expect($0 == nil, "unexpected startup failure"); done.signal() }
  provider.nextSettings()(nil)
  monitor().delayed()()
  wait(done)
  provider.barrierForTest()
}
func configure(_ provider: PacketTunnelProvider, _ generation: UInt64, error: Error? = nil) -> Bool {
  let done = DispatchSemaphore(value: 0); let result = ResultBox()
  let callback = TunnelHost.configurations.last!
  DispatchQueue.global().async { result.value = callback(generation, "{}"); done.signal() }
  provider.nextSettings()(error)
  wait(done)
  provider.pausePumpForTest()
  return result.value
}
var count = 0

// Stop must finish the pending start before the OS ever replies; a late
// success/error cannot start monitoring or release a second completion.
do {
  reset(); let provider = PacketTunnelProvider(); var results: [Bool] = []
  provider.startTunnel(options: nil) { results.append($0 == nil) }
  let late = provider.nextSettings()
  stop(provider)
  expect(results == [false], "stop did not complete outstanding start")
  late(nil); late(TunnelError("late")); provider.barrierForTest()
  expect(TunnelHost.starts == 0 && NWPathMonitor.instances.isEmpty, "stale settings resurrected provider")
  expect(results == [false], "start completed twice")
  count += 1
}

// Queued path callbacks retain their old monitor identity and cannot touch
// the new host after stop/start. Repeated first-path delivery starts once.
do {
  reset(); let provider = PacketTunnelProvider(); var results: [Bool] = []
  provider.startTunnel(options: nil) { results.append($0 == nil) }
  provider.nextSettings()(nil)
  let old = monitor(); let oldPath = old.delayed()
  stop(provider)
  provider.startTunnel(options: nil) { results.append($0 == nil) }
  provider.nextSettings()(nil)
  let current = monitor().delayed()
  oldPath(); current(); current(); provider.barrierForTest()
  expect(TunnelHost.starts == 1 && TunnelHost.networks == 2, "stale path mutated newer host")
  expect(results == [false, true], "wrong path startup completion")
  stop(provider); count += 1
}

// A second start while OS settings are unsettled is refused; it does not
// steal the first completion or attempt another settings transaction.
do {
  reset(); let provider = PacketTunnelProvider(); var first: [Bool] = []; var second: [Bool] = []
  provider.startTunnel(options: nil) { first.append($0 == nil) }
  let pending = provider.nextSettings()
  provider.startTunnel(options: nil) { second.append($0 == nil) }
  provider.barrierForTest()
  expect(second == [false] && first.isEmpty, "parallel startup stole pending run")
  pending(TunnelError("settings failed")); provider.barrierForTest(); stop(provider)
  expect(first == [false] && TunnelHost.starts == 0, "failed start was not settled once")
  count += 1
}

// An old exit already queued by Rust cannot cancel a later provider run.
do {
  reset(); let provider = PacketTunnelProvider(); start(provider)
  let oldExit = TunnelHost.exits[0]
  stop(provider); start(provider)
  oldExit("old"); provider.barrierForTest()
  expect(provider.cancellations == 0 && TunnelHost.stops == 1, "old exit stopped newer host")
  TunnelHost.exits[1]("current"); wait(provider.cancelled); wait(TunnelHost.stopped)
  stop(provider)
  expect(provider.cancellations == 1 && TunnelHost.stops == 2, "current exit or repeated stop used wrong host")
  count += 1
}

// Failed host startup must cancel its monitor; replaying an already queued
// path cannot retry startup or publish into a future host.
do {
  reset(); let provider = PacketTunnelProvider(); var results: [Bool] = []
  TunnelHost.failStart = true
  provider.startTunnel(options: nil) { results.append($0 == nil) }
  provider.nextSettings()(nil)
  let watch = monitor(); let path = watch.delayed()
  path(); path(); provider.barrierForTest(); stop(provider)
  expect(results == [false] && TunnelHost.starts == 0 && TunnelHost.networks == 1, "failed host was retried by stale path")
  count += 1
}

// Production packetFlow calls carry complete IP packets and the correct
// family, and old read callbacks are fenced after detach/re-attachment.
do {
  reset(); let provider = PacketTunnelProvider(); start(provider)
  expect(configure(provider, 41), "public packet attach failed")
  let reader = provider.packetFlow.reads.last!
  reader([Data([0x45, 1]), Data([0x60, 2])], [NSNumber(value: AF_INET), NSNumber(value: AF_INET6)])
  provider.barrierForTest()
  expect(TunnelHost.sends.count == 2 && TunnelHost.sends.allSatisfy { $0.0 == 41 }, "input family/generation bridge failed")
  let oldReader = provider.packetFlow.reads.last!
  let callback = TunnelHost.configurations[0]
  expect(callback(41, nil), "public packet detach failed")
  expect(configure(provider, 42), "second packet generation failed")
  oldReader([Data([0x45, 9])], [NSNumber(value: AF_INET)])
  provider.barrierForTest()
  expect(TunnelHost.sends.count == 2, "old reader entered new generation")
  TunnelHost.output = [(42, Data([0x60, 3]))]
  provider.pumpForTest()
  expect(provider.packetFlow.writes.last!.0 == [Data([0x60, 3])], "output bytes changed")
  expect(provider.packetFlow.writes.last!.1 == [NSNumber(value: AF_INET6)], "output family changed")
  stop(provider); count += 1
}

// Detach must join a public write already dequeued from Rust, before it
// advertises completion. No real packetFlow/OS object participates here.
do {
  reset(); let provider = PacketTunnelProvider(); start(provider)
  expect(configure(provider, 51), "packet attach failed")
  let entered = DispatchSemaphore(value: 0); let release = DispatchSemaphore(value: 0)
  provider.packetFlow.writeEntered = entered; provider.packetFlow.writeRelease = release
  TunnelHost.output = [(51, Data([0x45, 1]))]
  let written = DispatchSemaphore(value: 0)
  DispatchQueue.global().async { provider.pumpForTest(); written.signal() }
  wait(entered)
  let stopped = DispatchSemaphore(value: 0); let beginning = DispatchSemaphore(value: 0)
  let callback = TunnelHost.configurations[0]
  DispatchQueue.global().async { beginning.signal(); expect(callback(51, nil), "detach failed"); stopped.signal() }
  wait(beginning)
  let early = stopped.wait(timeout: .now() + .milliseconds(100)) == .success
  release.signal(); wait(written)
  if !early { wait(stopped) }
  expect(!early, "detach returned while a public write was still active")
  provider.packetFlow.writeEntered = nil; provider.packetFlow.writeRelease = nil
  stop(provider); count += 1
}

// A failed public write closes native admission before Apple's stop arrives.
do {
  reset(); let provider = PacketTunnelProvider(); start(provider)
  expect(configure(provider, 61), "packet attach failed")
  provider.packetFlow.allowWrite = false
  TunnelHost.output = [(61, Data([0x45, 1]))]
  provider.pumpForTest(); wait(provider.cancelled)
  expect(!TunnelHost.configurations[0](62, "{}"), "cancelled provider admitted another capture")
  stop(provider); count += 1
}

// Settings failure cannot start reads or be reported as clean capture.
do {
  reset(); let provider = PacketTunnelProvider(); start(provider)
  expect(!configure(provider, 71, error: TunnelError("settings refused")), "settings error was acknowledged")
  expect(provider.packetFlow.reads.isEmpty, "failed settings started packet reading")
  wait(provider.cancelled); stop(provider); count += 1
}
// Sleep/wake use the same serialized host ownership as stop.
do {
  reset(); let provider = PacketTunnelProvider(); start(provider)
  let slept = DispatchSemaphore(value: 0)
  provider.sleep { slept.signal() }; wait(slept)
  provider.wake(); provider.barrierForTest()
  expect(TunnelHost.sleeps == 1 && TunnelHost.wakes == 1, "current lifecycle events were not forwarded")
  stop(provider)
  let stoppedSleep = DispatchSemaphore(value: 0)
  provider.sleep { stoppedSleep.signal() }; wait(stoppedSleep)
  provider.wake(); provider.barrierForTest()
  expect(TunnelHost.sleeps == 1 && TunnelHost.wakes == 1, "stopped host received lifecycle events")
  count += 1
}
print("\(count) production public-packet provider cases passed (controlled boundaries; not device acceptance)")
