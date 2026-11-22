// Late settings must not resurrect a stopped tunnel; startup completes once.
do {
  reset()
  let provider = PacketTunnelProvider()
  var completions: [Bool] = []
  provider.startTunnel(options: nil) { completions.append($0 == nil) }
  let late = settings(provider)
  stop(provider)
  late(nil)
  settle(provider)
  expect(NWPathMonitor.instances.isEmpty, "late settings completion started a monitor after stop")
  expect(TunnelHost.starts == 0, "stopped startup entered native code")
  expect(completions == [false], "interrupted startup did not complete exactly once")
}

// A canceled monitor's queued first path cannot start or publish into a new run.
do {
  reset()
  let provider = PacketTunnelProvider()
  var completions: [Bool] = []
  provider.startTunnel(options: nil) { completions.append($0 == nil) }
  settings(provider)(nil)
  let oldPath = monitor().delayed()
  stop(provider)
  provider.startTunnel(options: nil) { completions.append($0 == nil) }
  settings(provider)(nil)
  let current = monitor().delayed()
  oldPath()
  current()
  current()
  settle(provider)
  expect(TunnelHost.starts == 1, "a stale or repeated path started a second host")
  expect(TunnelHost.networks == 2, "a stale path published into a newer host")
  expect(completions == [false, true], "path startup completed incorrectly")
  stop(provider)
  expect(TunnelHost.stops == 1, "the current host was not stopped once")
}

// A newer start supersedes pending settings; old success AND error are ignored.
do {
  reset()
  let provider = PacketTunnelProvider()
  var first: [Bool] = []
  var second: [Bool] = []
  provider.startTunnel(options: nil) { first.append($0 == nil) }
  let old = settings(provider)
  provider.startTunnel(options: nil) { second.append($0 == nil) }
  settings(provider)(nil)
  let path = monitor().delayed()
  old(nil)
  old(TunnelError("old failure"))
  settle(provider)
  expect(NWPathMonitor.instances.count == 1, "superseded settings replaced the current monitor")
  path()
  expect(first == [false] && second == [true], "superseded startup completed more than once")
  stop(provider)
}

// Exit work carries its run identity; it cannot cancel a later tunnel.
do {
  reset()
  let provider = PacketTunnelProvider()
  provider.startTunnel(options: nil) { _ in }
  settings(provider)(nil)
  monitor().delayed()()
  let oldExit = TunnelHost.exits[0]
  stop(provider)
  provider.startTunnel(options: nil) { _ in }
  settings(provider)(nil)
  monitor().delayed()()
  oldExit("old exit")
  settle(provider)
  expect(provider.cancellations == 0 && TunnelHost.stops == 1, "old exit stopped a newer run")
  TunnelHost.exits[1]("current exit")
  settle(provider)
  expect(provider.cancellations == 1 && TunnelHost.stops == 2, "current exit did not stop its own run")
  stop(provider)
  expect(TunnelHost.stops == 2, "repeated stop stopped the host twice")
}

// Failed native startup cancels monitoring and reports its error once.
do {
  reset()
  let provider = PacketTunnelProvider()
  var completions: [Bool] = []
  TunnelHost.failStart = true
  provider.startTunnel(options: nil) { completions.append($0 == nil) }
  settings(provider)(nil)
  let latePath = monitor().delayed()
  latePath()
  latePath()
  stop(provider)
  expect(completions == [false] && TunnelHost.starts == 0, "failed native startup was retried or completed twice")
}
print("5 provider lifecycle cases passed")
