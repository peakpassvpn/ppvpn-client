# Rust 版与 Go 版的行为对照

Go core 冻结在 v0.5.21（#45）。Rust 的 `ppvpn-engine` 硬切换前，下表每一行都要有结论：

- **todo**：还没有对应的 Rust 用例；
- **done**：`Rust 用例` 一列写明对应的测试（crate 路径和名称），行为与 Go 一致；
- **n-a**：不适用，`备注` 写明原因（例如只属于 sing-box 配置、或者没有宿主使用的接口）。

行为不一致、但决定就这样改的，也算 done，必须在 `备注` 里写明差异和决定出处。没有结论的行不能硬切换。

配套的语言无关基准在 [`testdata/golden/`](../testdata/golden/README.md)：Core API 契约（`contract/`）和路由判定（`routing/`）。Rust 跑同一组文件，即可覆盖其中的行为；对应的 Go 运行器（`TestGoldenContract`、`TestGoldenRouting`）在下表标为 n-a。

分组沿用 #45 评审意见里的八组。"行为摘要"取自 Go 测试的注释，没有注释的取测试名；细节以 Go 测试为准。

新增 Go 测试（只允许测试和文档）时，在对应分组里加一行。

## 1. 热切换和排空

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `internal/runtime` `TestApplyClosesConnectionsANewRuleRejects` | A new reject rule closes the connections it now matches. |  | todo |  |
| `internal/runtime` `TestApplyClosesConnectionsOfRemovedNodes` | Taking a node away closes its connections (and its local proxy user's) on the switch; a connection on a node that stays keeps running. |  | todo |  |
| `internal/runtime` `TestApplyDoesNotDeadlockWithStatusAndAWriter` | An apply in progress must not wedge the core's lock: Status reads the kernel while holding it (activeIngress), and the apply's prepare (pins) takes it. |  | todo |  |
| `internal/runtime` `TestApplyKeepsRunningConnections` | An apply while a download runs (new rules, same nodes) switches kernels without touching the download: it completes in full after the switch, new connections use the new … |  | todo |  |
| `internal/runtime` `TestApplyKernelStartFailureLeavesTheOldKernel` | A kernel that fails to start is discarded: ApplyProfile fails, the old kernel and its connections are untouched, and the old profile stays. |  | todo |  |
| `internal/runtime` `TestApplyReachesConnectionsOfOlderKernels` | A switch applies the new profile to every replaced kernel still draining, not only the one it replaces: a download started two applies earlier is counted as kept, and … |  | todo |  |
| `internal/runtime` `TestAtomicApplyRollback` | Atomic apply rollback |  | todo |  |
| `internal/runtime` `TestCloseOnSwitchDecidesByRecordedNode` | closeOnSwitch decides by the node the routing kernel recorded, not by looking the connection's tags up in the previous build, so it holds even if tags stop being stable … |  | todo |  |
| `internal/runtime` `TestConcurrentLifecycleOperationsDoNotLeakEngines` | Concurrent lifecycle operations do not leak engines |  | todo |  |
| `internal/runtime` `TestDrainClosesIdleConnections` | A keep-alive connection that goes quiet in a draining kernel is closed after drainIdleClose, and the kernel drains instead of waiting for drainLimit. |  | todo |  |
| `internal/runtime` `TestDrainDeadlineClosesTheRest` | A kernel past drainLimit is closed with what it still carries. |  | todo |  |
| `internal/runtime` `TestDrainKeepsActiveConnections` | A connection that keeps moving bytes is not idle, however long it runs in a draining kernel; the kernel drains once it ends. |  | todo |  |
| `internal/runtime` `TestDrainToleratesShortPauses` | Pauses shorter than the threshold do not close a draining connection. |  | todo |  |
| `internal/runtime` `TestEffectiveProfile` | Effective profile |  | todo |  |
| `internal/runtime` `TestFirstKernelDrainsAcrossSwitches` | The first kernel drains like any other across several switches: its idle keep-alive connection is closed after drainIdleClose, a connection with a heartbeat stays (its … |  | todo |  |
| `internal/runtime` `TestFullRestartReasons` | fullRestartReasons is a whitelist: only listener changes stop the engine. |  | todo |  |
| `internal/runtime` `TestKernelSwitchKeepsReverseMapping` | A kernel switch keeps the reverse mapping: a client that resolved a name through the old kernel and connects to the address through the new one still matches the name's … |  | todo |  |
| `internal/runtime` `TestLifecycleAndRuntimeRollback` | Lifecycle and runtime rollback |  | todo |  |
| `internal/runtime` `TestLifecycleLogsPhaseTimings` | Apply and start each write one info line with per-phase durations, so a slow /v1/start shows where the time went. |  | todo |  |
| `internal/runtime` `TestProfileIsCopiedBeforeRetention` | Profile is copied before retention |  | todo |  |
| `internal/runtime` `TestProfileRejectRuleDoesNotCrashCore` | hits a profile reject rule on a real sing-box. |  | todo |  |
| `internal/runtime` `TestRapidAppliesDrainEveryKernel` | Rapid applies stack draining kernels; each drains on its own once its connections end, and none is left behind. |  | todo |  |
| `internal/runtime` `TestRoutingModeSwitchKeepsConnections` | Switching routing_mode is a kernel switch: an existing connection keeps running (global only loosens what rules allowed), new ones follow global. |  | todo |  |
| `internal/runtime` `TestRoutingModeSwitchesWithoutNewRevision` | The routing mode is part of what an apply changes: switching it re-applies the same revision, the global mode renders only baseline rules with the selected node as final … |  | todo |  |
| `internal/runtime` `TestSameRevisionNoopAndMigrationKeepsSelection` | Same revision noop and migration keeps selection |  | todo |  |

## 2. Linux TUN 路由规则守护（tunrules）

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `internal/config` `TestDesktopTUNUsesOwnIPRoute2Namespace` | Desktop tun uses own ip route2 namespace |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/runtime` `TestTUNRulesBrokenIsReported` | : rules that stay missing set the status to broken and send TunRoutingBroken. |  | todo |  |
| `internal/runtime` `TestTUNRulesRestoredAfterDeletion` | deletes the TUN's policy routing the ways seen in the field (everything, as networkd does on a link down; just the goto target; the table's routes) and requires each to … |  | todo |  |
| `internal/runtime` `TestTUNRulesSurviveNetworkdLinkFlap` | reproduces the field report: with systemd-networkd managing a link (ManageForeignRoutingPolicyRules on, its default), taking the link down and up makes networkd drop the … |  | todo |  |
| `internal/tunrules` `TestMissingCountsDuplicates` | Missing counts duplicates |  | todo |  |
| `internal/tunrules` `TestOwnedKeepsEverySingTunRule` | Owned keeps every sing tun rule |  | todo |  |
| `internal/tunrules` `TestOwnedLeavesOtherProgramsRules` | Owned leaves other programs rules |  | todo |  |
| `internal/tunrules` `TestRestoreOrderPutsGotoTargetsFirst` | Restore order puts goto targets first |  | todo |  |
| `internal/tunrules` `TestRouteRestoreOrderPutsGatewayCoveringRoutesLast` | Route restore order puts gateway covering routes last |  | todo |  |
| `internal/tunrules` `TestRuleString` | Rule string |  | todo |  |

## 3. dns-local

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `cmd/ppvpn-core` `TestServeValidatesLocalDNSServers` | Serve validates local dns servers |  | todo |  |
| `internal/config` `TestLocalDNSServers` | Host-supplied physical resolvers become a static ppvpn-local dns-local with every one outside the tunnel, in order; with none left (or none given) dns-local reads the … |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/localdns` `TestCacheFailsFastWithoutServers` | DHCP has not handed out DNS yet: queries fail at once with a clear error (never 127.0.0.1, never a 5 s timeout), and the interface is read again at most once per … |  | todo |  |
| `internal/localdns` `TestCacheFollowsInterfaceChanges` | Cache follows interface changes |  | todo |  |
| `internal/localdns` `TestCacheReadsOnceForConcurrentQueries` | Concurrent queries after an invalidation share one read. |  | todo |  |
| `internal/localdns` `TestCacheRefreshes` | Cache refreshes |  | todo |  |
| `internal/localdns` `TestExchangeAsksServersInOrder` | Exchange asks servers in order |  | todo |  |
| `internal/localdns` `TestExchangeFailureRereads` | Every server failing marks the read servers suspect, so the next query reads the interface again (after RetryInterval). |  | todo |  |
| `internal/localdns` `TestExchangeWithoutServersAnswersServfailAtOnce` | Without servers a hijacked query gets SERVFAIL at once (an error would leave the client waiting for its own timeout), with the cause logged. |  | todo |  |
| `internal/localdns` `TestGlobalServers` | Global servers |  | todo |  |
| `internal/localdns` `TestScopedServers` | Scoped servers |  | todo |  |
| `internal/localdns` `TestUsableLeavesOutTunnelLoopbackAndForeignLinkLocal` | Usable leaves out tunnel loopback and foreign link local |  | todo |  |

## 4. dnstransport guard 与 TUN 远端 DNS

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `internal/config` `TestGuardedDNSTagIsTheRemoteServer` | The DNS transport guard is keyed on the remote server's tag. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/dnstransport` `TestGuardBudgetEndsBeforeSingBoxTimeout` | The overall budget must end before sing-box's own DNS timeout, or SERVFAIL can never be sent. |  | todo |  |
| `internal/dnstransport` `TestGuardDoesNotRetryAnswers` | A DNS answer, even NXDOMAIN or SERVFAIL, is the server's answer: no retry. |  | todo |  |
| `internal/dnstransport` `TestGuardFailsWhenAllUpstreamsFail` | When every upstream fails the query fails with SERVFAIL, so the client is answered instead of waiting out its timeout; no other resolver is tried. |  | todo |  |
| `internal/dnstransport` `TestGuardFallsBackInOrder` | Each attempt goes to the next upstream in order: dns-remote, then the fallbacks. |  | todo |  |
| `internal/dnstransport` `TestGuardGivesUpWithinBudget` | When every attempt fails the guard gives up after maxAttempts within the overall budget and answers SERVFAIL; a caller that gave up gets the error. |  | todo |  |
| `internal/dnstransport` `TestGuardPrefersTheLastAnsweringUpstream` | After a fallback answers, queries start from it for preferFor, so a blocked dns-remote is not hit first every time; then dns-remote is tried first again. |  | todo |  |
| `internal/dnstransport` `TestGuardResetsThePoolAfterIdle` | After idleReset without a success, and with nothing in flight, the pool is reset before the next query; not after a recent success or while another query is in flight. |  | todo |  |
| `internal/dnstransport` `TestGuardRetriesAStaleConnectionOnTheSameUpstream` | A pooled connection the peer already closed fails with EOF at once: the guard resets that upstream's pool and retries it once before falling back, and the answer does … |  | todo |  |
| `internal/dnstransport` `TestGuardRetriesAStaleConnectionOnlyOnce` | Only one same-upstream retry per query: a second stale failure falls back, and every upstream is still tried (four attempts). |  | todo |  |
| `internal/dnstransport` `TestGuardRetriesAfterAHungAttempt` | A query swallowed by a half-open connection costs one attempt timeout, not the whole DNS timeout: the next attempt answers. |  | todo |  |
| `internal/dnstransport` `TestGuardRetriesEOF` | EOF (a closed pooled connection) takes the same retry path. |  | todo |  |
| `internal/dnstransport` `TestGuardWarnsAboutAMissingFallbackOnce` | A fallback missing from the manager is warned about once, and looked up again on later queries instead of being lost for good. |  | todo |  |
| `internal/dnstransport` `TestStaleConnectionErrors` | Stale connection errors |  | todo |  |
| `internal/dnstransport` `TestStaleConnectionWindowsErrnos` | A Windows reset or abort, as net wraps it, is a stale connection. |  | todo |  |
| `internal/dnstransport` `TestUnguardedExchangeIsLoggedOnlyAtDebugLevel` | Unguarded exchange is logged only at debug level |  | todo |  |
| `internal/dnstransport` `TestWrappingWithoutLogger` | Without a logger only the guarded server is wrapped. |  | todo |  |
| `internal/runtime` `TestTUNRemoteDNSAllFailAnswersServfail` | When every remote server fails, a hijacked query is answered SERVFAIL over TCP and UDP, instead of no answer (sing-box drops a query whose exchange errors and the client … |  | todo |  |
| `internal/runtime` `TestTUNRemoteDNSAllTimeOutAnswersServfail` | When every remote server times out (silently dropped), SERVFAIL still arrives within the guard budget: the budget ends before sing-box's own DNS timeout cancels the … |  | todo |  |
| `internal/runtime` `TestTUNRemoteDNSFallsBackThroughTheNode` | When dns-remote fails through the node, the guard falls back to the next DoT server in a real sing-box and gets the answer on attempt 2; the next uncached query starts … |  | todo |  |
| `internal/runtime` `TestTUNRemoteDNSRetriesStalePooledConnections` | A DoT server that closed its idle connections leaves them in sing-box's pool. |  | todo |  |

## 5. IPv6 无出口、domaindest、离线与网卡变化

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `api` `TestNoDefaultInterfaceIsRetryable` | Offline probes fail fast as NO_DEFAULT_INTERFACE, retryable. |  | todo |  |
| `internal/config` `TestDesktopTUNRoutesIPv6AndExcludesIPv6Ingress` | Desktop TUN carries an IPv6 address so IPv6 (and DNS to IPv6 resolvers) is routed into the tunnel instead of around it; every ingress IP, IPv4 or IPv6, stays excluded. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestDesktopTUNWithoutHostIPv6IsIPv4Only` | A host with IPv6 disabled cannot give the TUN an IPv6 address (sing-tun fails the whole start), so the desktop TUN stays IPv4-only there and no IPv6 ingress prefix is … |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestKnownDomainRegexRejectsIPLiterals` | The fake-ip rule must treat an IP literal (what the HTTP sniffer reports for a request to a bare address) as "no domain". |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestNoHostIPv6RouteHandsDirectIPv6ItsDomain` | A host with IPv6 enabled but no IPv6 path of its own keeps the same TUN (IPv6 address and routes, so nothing bypasses it) and only wraps direct. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestNoHostIPv6RouteLeavesIPv4OnlyTUNAlone` | The hand-off needs the TUN's IPv6: a host with IPv6 disabled, and mobile (IPv4-only tunnel), render direct as before. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestTUNProxyTargetsCarryDomain` | Tun proxy targets carry domain |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/domaindest` `TestRestoreFallsBackToTheSharedReverseMapping` | After a kernel switch the new kernel's own reverse mapping is empty; the shared store still gives direct's ipv6_only wrapper (no host IPv6 path) the domain of a global … |  | todo |  |
| `internal/domaindest` `TestRestoreIPv6Only` | Restore i pv6 only |  | todo |  |
| `internal/domaindest` `TestRestore` | Restore |  | todo |  |
| `internal/hostipv6` `TestDarwinDefaultRouteIndexes` | Darwin default route indexes |  | todo |  |
| `internal/hostipv6` `TestLinuxAvailability` | Linux availability |  | todo |  |
| `internal/hostipv6` `TestLinuxRoute` | Linux route |  | todo |  |
| `internal/hostipv6` `TestRouteOnThisHost` | Route on this host |  | todo |  |
| `internal/hostipv6` `TestWindowsAvailability` | Windows availability |  | todo |  |
| `internal/hostipv6` `TestWindowsRoute` | Windows route |  | todo |  |
| `internal/reversemap` `TestCapacityEvictsTheEntryClosestToExpiry` | Capacity evicts the entry closest to expiry |  | todo |  |
| `internal/reversemap` `TestRecordLookupAndExpiry` | Record lookup and expiry |  | todo |  |
| `internal/runtime` `TestApplyProfileProbesHostIPv6ForTUN` | The desktop TUN carries IPv6 only when the host probe allows it; the probe runs on every apply because IPv6 can be toggled between starts. |  | todo |  |
| `internal/runtime` `TestDefaultInterfaceChangeEmitsNetworkChanged` | Every default interface change of the running engine is reported as NetworkChanged: the new interface's name and index, or none. |  | todo |  |
| `internal/runtime` `TestDefaultInterfaceLogLine` | Default interface log line |  | todo |  |
| `internal/runtime` `TestHostIPv6RouteDecidesDirectHandOff` | A host with IPv6 enabled but no IPv6 path keeps the IPv6 TUN and wraps direct; the probe runs on every apply and again at start, and its result is logged. |  | todo |  |
| `internal/runtime` `TestProbesFailFastWithoutDefaultInterface` | With no default interface (offline) both probes fail at once with ErrNoDefaultInterface instead of waiting out their timeout; with one, or when the engine cannot tell, … |  | todo |  |
| `internal/runtime` `TestReprobeDebouncesBursts` | A burst of changes (a Wi-Fi switch) re-arms one probe: each change stops the pending one, and only the last fires. |  | todo |  |
| `internal/runtime` `TestReprobeDefersToApply` | An apply between the change and the probe builds for the new state; the probe then finds nothing to do. |  | todo |  |
| `internal/runtime` `TestReprobeKeepsKernelWhenUnchanged` | Same result: nothing rebuilt, no change logged. |  | todo |  |
| `internal/runtime` `TestReprobeRealTimerFires` | The default scheduler is time.AfterFunc: a change still leads to a probe on its own (with a short delay and a generous deadline). |  | todo |  |
| `internal/runtime` `TestReprobeSkipsWhileOffline` | A link goes down: the path looks lost only because there is no network. |  | todo |  |
| `internal/runtime` `TestReprobeSwitchesWhenIPv6PathAppears` | The host gains an IPv6 path: switch back to the plain build. |  | todo |  |
| `internal/runtime` `TestReprobeSwitchesWhenIPv6PathIsLost` | The host loses its IPv6 path (joins an IPv4-only network): one kernel switch to the hand-off build, no restart, armed ReprobeDelay out. |  | todo |  |
| `internal/runtime` `TestStopCancelsPendingReprobe` | Stop cancels a pending re-probe. |  | todo |  |
| `internal/runtime` `TestTUNApplyWithoutIPv6PathSwitchesKernels` | With the host's IPv6 state unchanged, a TUN apply is a kernel switch: the no-IPv6-path build (direct wrapped, direct-host resolving IPv4 only, see #48) changes … |  | todo |  |
| `internal/runtime` `TestTUNRouteResolvesAndHandsDomainsToNode` | runs the TUN route and DNS configuration on a real sing-box. |  | todo |  |

## 6. failover 与 pin

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `api` `TestPinIngressEndpoint` | Pin ingress endpoint |  | todo |  |
| `internal/failover` `TestActiveTracksSwitchesAndNotifiesObserver` | Active tracks switches and notifies observer |  | todo |  |
| `internal/failover` `TestCheckFallsBackToTheSecondURL` | checkAny passes when a later URL answers although the first does not. |  | todo |  |
| `internal/failover` `TestDialTimeoutMovesToTheNextMember` | A member that hangs costs dialTimeout, then the next member serves the same dial. |  | todo |  |
| `internal/failover` `TestDwellKeepsBackupAfterSwitch` | Within minDwell of a switch the backup keeps leading even once the primary is healthy again; afterwards the primary leads. |  | todo |  |
| `internal/failover` `TestHTTPCheck` | Http check |  | todo |  |
| `internal/failover` `TestHealthLoopIsLazyAndChecksEveryMember` | Health loop is lazy and checks every member |  | todo |  |
| `internal/failover` `TestNetworkFilteringAndAllFailed` | Network filtering and all failed |  | todo |  |
| `internal/failover` `TestPinnedMemberHasNoFallback` | A pinned member carries every connection alone, with no fallback even while it fails; unpinning restores automatic selection. |  | todo |  |
| `internal/failover` `TestPrefersPrimaryAndFailsOverImmediately` | Prefers primary and fails over immediately |  | todo |  |
| `internal/failover` `TestProbeThresholds` | Checks mark a member unhealthy only after UnhealthyAfter consecutive failures, and healthy again only after RecoverAfter consecutive passes; a failed dial marks it … |  | todo |  |
| `internal/failover` `TestRejectsInvalidMembers` | Rejects invalid members |  | todo |  |
| `internal/failover` `TestReturnsToPrimaryAfterRecovery` | Returns to primary after recovery |  | todo |  |
| `internal/runtime` `TestDeadPortRefusesAndStaysReserved` | A dead port refuses connections at once (a dial that hangs instead would make a dead ingress look like a slow one), and on Linux, where released ports get reused, no … |  | todo |  |
| `internal/runtime` `TestLocalProxyOnlyCoreFailsOverToBackupIngress` | runs the real sing-box runtime the way the unprivileged desktop core does (--tun=false --local-proxy=true): a node whose primary ingress is down must still pass the … |  | todo |  |
| `internal/runtime` `TestPinIngressOnARunningCore` | A pinned node sends every connection through its pinned ingress with no fallback, the pin takes effect on the running engine and survives start, status reports it with … |  | todo |  |

## 7. outboundlog

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `internal/outboundlog` `TestLimiterForgetsOldDestinationsWhenFull` | Limiter forgets old destinations when full |  | todo |  |
| `internal/outboundlog` `TestLimiterLogsOncePerWindowWithSuppressedCount` | Limiter logs once per window with suppressed count |  | todo |  |
| `internal/runtime` `TestApplyLogsRealityFingerprintsAtDebug` | At debug level an apply logs fingerprints of each REALITY ingress's parameters, never the values themselves. |  | todo |  |
| `internal/runtime` `TestDirectOutboundFailuresAreLoggedAndLimited` | A failed direct connection logs the same "outbound failed" line as a node (no node_id or endpoint_key), once per destination per DirectLimit; the next line after the … |  | todo |  |
| `internal/runtime` `TestOutboundFailuresAreLoggedAtDebug` | At debug level a failed node connection names the node, the ingress, the protocol, the stage and the error, and never the credentials: a dead port fails at dial; a … |  | todo |  |

## 8. 其他契约

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `api` `TestAPIVersionMismatchRejected` | Api version mismatch rejected |  | todo |  |
| `api` `TestApplyAndListNeverExposeCredentials` | Apply and list never expose credentials |  | todo |  |
| `api` `TestDebugGoroutinesOnlyAtDebugLevel` | GET /v1/debug/goroutines answers only at debug level and only to an authenticated caller. |  | todo |  |
| `api` `TestFoldedErrorIsLoggedWithStageButNotReturned` | Folded error is logged with stage but not returned |  | todo |  |
| `api` `TestGoldenContract` | Golden contract |  | n-a | golden 运行器本身；Rust 跑同一组文件（testdata/golden） |
| `api` `TestLocalProxyAPIsReportDisabledCore` | Local proxy ap is report disabled core |  | todo |  |
| `api` `TestLocalProxyMetadataAndCredentialAreSeparated` | Local proxy metadata and credential are separated |  | todo |  |
| `api` `TestProbeEntrancesMethodAndShape` | Probe entrances method and shape |  | todo |  |
| `api` `TestRoutingModeOnApplyAndStatus` | routing_mode is optional on validate/apply-profile, strictly checked, and reported by get-status; switching it re-applies the same revision. |  | todo |  |
| `api` `TestRuleSetHostsArePinnedAtValidateAndApply` | Rule set hosts are pinned at validate and apply |  | todo |  |
| `api` `TestSetSystemProxyToggleAndStatus` | Set system proxy toggle and status |  | todo |  |
| `api` `TestSystemProxyUnavailableWithoutStateOrInTUNCore` | System proxy unavailable without state or in tun core |  | todo |  |
| `api` `TestUnauthenticatedRejected` | Unauthenticated rejected |  | todo |  |
| `api` `TestUnknownMethodUsesEnvelope` | Unknown method uses envelope |  | todo |  |
| `api` `TestValidationErrorIsStructuredAndRedacted` | Validation error is structured and redacted |  | todo |  |
| `cmd/ppvpn-core` `TestRotateSessionSecret` | Rotate session secret |  | todo |  |
| `cmd/ppvpn-core` `TestServeLogsToFileEvenWhenStartupFails` | Serve logs to file even when startup fails |  | todo |  |
| `cmd/ppvpn-core` `TestServeRejectsTUNStackMissingFromBuild` | Serve rejects tun stack missing from build |  | todo |  |
| `internal/config` `TestEachLocalProxyUserRoutesToItsNode` | Each local proxy user routes to its node |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestGoldenOutboundOptions` | Golden outbound options |  | n-a | sing-box 配置的 golden，Rust 的配置形状不同 |
| `internal/config` `TestIngressTagsStableAcrossReorder` | checks member tags follow endpoint_key, not array position, so a reordered/extended replica set keeps existing tags. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestLocalProxyRejectsInconsistentEndpoints` | Local proxy rejects inconsistent endpoints |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestMain` | renders sing-box's local transport for dns-local whatever the OS the tests run on, so goldens are the same on macOS and Linux; tests of the core's own transport set … |  | n-a | 测试框架入口 |
| `internal/config` `TestMultiIngressProfileGolden` | renders the shared fixture (one node with a primary+backup, one single-ingress node) for desktop TUN plus local proxies, and checks the rendered JSON decodes again … |  | n-a | sing-box 配置的 golden，Rust 的配置形状不同 |
| `internal/config` `TestNoTUNKeepsRoutingUnchanged` | Without TUN (local proxy, system proxy, compatibility mode) none of it is rendered: those inbounds carry domains already. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestNodeOutboundDialsIngressIP` | A node outbound dials the ingress IP when the profile gives one, so reaching a node never needs DNS (in TUN mode the lookup can loop into the core's own tunnel DNS); the … |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestPlatformCapabilitiesStayOutsideProfile` | Platform capabilities stay outside profile |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestPrivateBypassRuleFollowsPerNodeRules` | Private bypass rule follows per node rules |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestProfileToOptionsGolden` | Profile to options golden |  | n-a | sing-box 配置的 golden，Rust 的配置形状不同 |
| `internal/config` `TestRuleMappingAndFixedPriority` | Rule mapping and fixed priority |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestRuleSetsMirrorIntoTUNDNS` | Domain rule sets are mirrored into DNS like domain matchers: direct to dns-local, proxy to dns-remote, reject refused. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestRuleSetsRenderAsLocalBinaryAndRulesReferenceThem` | Rule sets render as local binary and rules reference them |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestShadowsocksEIHPasswordOrder` | SIP022: identity_keys are the server iPSKs, outermost first, and user_key is the user's uPSK; the EIH password is iPSK1:...:iPSKn:uPSK. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestStableTagAcrossEndpointMigration` | Stable tag across endpoint migration |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestTUNCoreRulesComeFirst` | Desktop (auto_route) and mobile (platform-owned tunnel) TUN render the same sniff, DNS hijack and fake-ip rules ahead of everything else. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestTUNDNSServersAndMirroredRules` | Tundns servers and mirrored rules |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestUnavailableRuleSetsAreSkipped` | A rule set without a local copy is dropped from every rule: a rule left without address matchers disappears, a rule with other matchers keeps them. |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/corelog` `TestDebugLinesOnlyAtDebugLevel` | Debug lines only at debug level |  | todo |  |
| `internal/corelog` `TestFileLogIsWrittenImmediately` | File log is written immediately |  | todo |  |
| `internal/corelog` `TestLineFormatRedactionAndChain` | Line format redaction and chain |  | todo |  |
| `internal/privateacl` `TestAccessErrorNamesPathAndRemedy` | Access error names path and remedy |  | todo |  |
| `internal/privateacl` `TestBroaderACLIsNotRepaired` | Broader acl is not repaired |  | todo |  |
| `internal/privateacl` `TestPrivateForTable` | Private for table |  | todo |  |
| `internal/privateacl` `TestRepairsLegacyACLAsOwner` | reproduces a 0.4.0 state directory and file (SYSTEM + Administrators only) created by this user, which owns them and can therefore rewrite their DACL. |  | todo |  |
| `internal/privateacl` `TestRoundTripAsCurrentUser` | writes and reads back private state as the real process user, which is what the standard core does. |  | todo |  |
| `internal/privateacl` `TestTrusteesDependOnProcessUser` | Trustees depend on process user |  | todo |  |
| `internal/privateacl` `TestUnixModes` | Unix modes |  | todo |  |
| `internal/proxyinbound` `TestBasicProxyAuth` | Basic proxy auth |  | todo |  |
| `internal/proxyinbound` `TestHTTPAuthFailureReturns407ThenClosesGracefully` | covers CONNECT and plain requests without auth, with a wrong password and with an unknown user: each reads back a complete 407 challenge followed by a clean EOF, never a … |  | todo |  |
| `internal/proxyinbound` `TestPeekRequestHeadDoesNotConsume` | Peek request head does not consume |  | todo |  |
| `internal/proxyinbound` `TestSOCKS5AuthFailureRepliesThenClosesGracefully` | covers a wrong password and an unknown user: the client reads the complete RFC 1929 failure reply and then a clean EOF, never a reset, even with a CONNECT request … |  | todo |  |
| `internal/proxyinbound` `TestSetUsersReplacesTheUserList` | SetUsers swaps the accepted users atomically: a removed user is refused, an added one accepted, and an empty list is rejected without changing anything. |  | todo |  |
| `internal/proxyinbound` `TestVerifierRequiresKnownUserAndExactSecret` | Verifier requires known user and exact secret |  | todo |  |
| `internal/redact` `TestJSON` | Json |  | todo |  |
| `internal/redact` `TestProxyURL` | Proxy url |  | todo |  |
| `internal/redact` `TestShadowsocksKeys` | Shadowsocks keys |  | todo |  |
| `internal/rulesets` `TestDownloadsRefuseRedirects` | Downloads refuse redirects |  | todo |  |
| `internal/rulesets` `TestFailedUpdateKeepsLastGoodCopy` | A new profile version that cannot be fetched keeps the last good copy. |  | todo |  |
| `internal/rulesets` `TestInspectClassifiesDNSMirroring` | Inspect classifies dns mirroring |  | todo |  |
| `internal/rulesets` `TestPathStaysInsideDir` | Path stays inside dir |  | todo |  |
| `internal/rulesets` `TestPrepareDownloadsVerifiesAndReusesCache` | Prepare downloads verifies and reuses cache |  | todo |  |
| `internal/rulesets` `TestPrepareRejectsDigestMismatchAndForeignHosts` | Prepare rejects digest mismatch and foreign hosts |  | todo |  |
| `internal/rulesets` `TestPrepareRejectsInvalidRuleSet` | Prepare rejects invalid rule set |  | todo |  |
| `internal/rulesets` `TestRecoverySweepsAllSetsAndRebuildsOnce` | When one set recovers, every other set that is not ready is retried at once (not on its own, possibly long, backoff), the downloads run concurrently, and the … |  | todo |  |
| `internal/rulesets` `TestRecoveryTriggersRebuild` | A set that was never downloaded is retried; once it arrives the manager asks for a rebuild so the skipped rules take effect. |  | todo |  |
| `internal/rulesets` `TestRefreshUsesETag` | A ready set is refreshed on its interval with If-None-Match and stays ready on 304. |  | todo |  |
| `internal/runtime` `TestFixtureWithRealityStartsInLocalProxyMode` | starts the shared fixture (a VLESS REALITY primary with a Shadowsocks backup) in the unprivileged desktop mode. |  | todo |  |
| `internal/runtime` `TestFreePortIsFreeForTCPAndUDP` | Free port is free for tcp and udp |  | todo |  |
| `internal/runtime` `TestGoldenRouting` | Golden routing |  | n-a | golden 运行器本身；Rust 跑同一组文件（testdata/golden） |
| `internal/runtime` `TestOpenFlowHonorsAuthorizedClassificationAcrossSelectedSwitch` | Open flow honors authorized classification across selected switch |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `internal/runtime` `TestOpenFlowRejectsDecisionFromOldProfileSnapshot` | Open flow rejects decision from old profile snapshot |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `internal/runtime` `TestRealSingBoxRunsMultipleLocalProxies` | Real sing box runs multiple local proxies |  | todo |  |
| `internal/runtime` `TestRoutedLocalProxyUserFollowsProfileRules` | : the bare prefix on the shared local proxy routes like the system proxy (7891): profile rules first (DIRECT included), then the selected node; select-node and … |  | todo |  |
| `internal/runtime` `TestRuleSetUnavailableThenRecovered` | A rule set that cannot be downloaded never blocks apply or start: its rule is skipped and reported; once the set arrives the core rebuilds with it. |  | todo |  |
| `internal/runtime` `TestRuleSetWithoutPinnedHostIsUnavailable` | Without allowed_rule_set_hosts the profile still applies; the set is reported unpinned and never fetched. |  | todo |  |
| `internal/runtime` `TestSelectNodeChangesOnlyNewFlowSelection` | Select node changes only new flow selection |  | todo |  |
| `internal/runtime` `TestSharedLocalProxyConnectAuthFailureChallenges` | runs the real core: a CONNECT without Proxy-Authorization, with a wrong password or for an unknown user reads back a 407 Basic challenge and then a clean EOF (browsers … |  | todo |  |
| `internal/runtime` `TestSharedLocalProxyRoutesByUsername` | runs two nodes behind one loopback port: the username picks the node for HTTP and SOCKS5, traffic is counted and attributed per node, and bad credentials or removed … |  | todo |  |
| `internal/runtime` `TestStandardCoreSelectNode` | drives select-node on a real non-TUN core, both before start (the selection must survive Start) and while running. |  | todo |  |
| `internal/runtime` `TestSystemProxyFollowsSelectedNodeAndRules` | runs the standard (non-TUN) core with the system proxy toggled at runtime: traffic follows the selected node and the profile's DIRECT rule, counts toward traffic, and … |  | todo |  |
| `internal/runtime` `TestSystemProxyStartFallsBackWhenPortTaken` | System proxy start falls back when port taken |  | todo |  |
| `internal/runtime` `TestSystemProxyUnavailableInTUNCore` | System proxy unavailable in tun core |  | todo |  |
| `internal/runtime` `TestTUNOnlyCoreRejectsLocalProxyAPIs` | covers `serve --tun --local-proxy=false`. |  | todo |  |
| `internal/runtime` `TestTUNRuleSetDomainGoesDirect` | runs a local binary rule set on a real sing-box with the TUN route (fed by a SOCKS inbound carrying the TUN tag): a sniffed domain in a direct rule set connects straight … |  | todo |  |
| `internal/runtime` `TestTelemetryCountsAndRemovesConnections` | The routed conn is the inbound (client) side, as sing-box hands it to the tracker: what the router reads from it is the client's upload, what it writes to it is the … |  | todo |  |
| `internal/runtime` `TestTrackedSniffedConnectionKeepsCachedBytes` | Without a concurrent close the cached bytes come first, then the stream. |  | todo |  |
| `internal/runtime` `TestTrackedSniffedConnectionReadRacesClose` | A sniffed connection reaches the tracker as a bufio.CachedConn holding the sniffed bytes. |  | todo |  |
| `internal/runtime` `TestTrackedSniffedPacketConnectionKeepsCachedPacket` | A sniffed UDP flow keeps its first (cached) packet and destination. |  | todo |  |
| `internal/runtime` `TestTrafficDirection` | : upload is what the client sends toward the remote, download is what the remote returns. |  | todo |  |
| `internal/runtime` `TestTransparentFlowAdapterUsesCompiledDecisionAndNodeOutbound` | Transparent flow adapter uses compiled decision and node outbound |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `internal/runtime` `TestUDPReadNeverSilentlyTruncatesDatagram` | Udp read never silently truncates datagram |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `internal/runtime` `TestUDPWritePreservesOneCallPerDatagram` | Udp write preserves one call per datagram |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `ipc` `TestUnixSocketIsPrivateAndServes` | Unix socket is private and serves |  | todo |  |
| `localproxy` `TestMigratesVersion1StateInPlace` | Migrates version1 state in place |  | todo |  |
| `localproxy` `TestParseUsername` | Parse username |  | todo |  |
| `localproxy` `TestPrefixesAreRandomLowercaseAlphanumerics` | Prefixes are random lowercase alphanumerics |  | todo |  |
| `localproxy` `TestRejectsUnsupportedOrCorruptState` | Rejects unsupported or corrupt state |  | todo |  |
| `localproxy` `TestRejectsWeakStatePermissions` | Rejects weak state permissions |  | todo |  |
| `localproxy` `TestRemovedNodeHasNoEndpoint` | Removed node has no endpoint |  | todo |  |
| `localproxy` `TestRoutedEndpoint` | Routed endpoint |  | todo |  |
| `localproxy` `TestSharedEndpointsAreStableAndPrivate` | Shared endpoints are stable and private |  | todo |  |
| `localproxy` `TestStartupPrefers7890AndFallsBackWhenBusy` | Startup prefers7890 and falls back when busy |  | todo |  |
| `localproxy` `TestStartupReplacesOccupiedPersistedPortOnly` | Startup replaces occupied persisted port only |  | todo |  |
| `localproxy` `TestStartupUsesPreferredPortWhenFree` | Startup uses preferred port when free |  | todo |  |
| `localproxy` `TestSystemProxyPortPrefers7891FallsBackAndPersists` | System proxy port prefers7891 falls back and persists |  | todo |  |
| `localproxy` `TestSystemProxyPortUsesPreferredWhenFree` | System proxy port uses preferred when free |  | todo |  |
| `mobile` `TestBridgeDoesNotExposeNodeCredentials` | Bridge does not expose node credentials |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `mobile` `TestBridgeRejectsUnknownRoutingMode` | Bridge rejects unknown routing mode |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `mobile` `TestFlowIOTimeoutZeroMeansNoDeadline` | Flow io timeout zero means no deadline |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `probe` `TestAvailabilityCancellation` | Availability cancellation |  | todo |  |
| `probe` `TestAvailabilityUsesAuthenticatedNodeProxy` | Availability uses authenticated node proxy |  | todo |  |
| `probe` `TestEntranceAllFailedReportsPrimary` | Entrance all failed reports primary |  | todo |  |
| `probe` `TestEntranceCanceled` | Entrance canceled |  | todo |  |
| `probe` `TestEntranceDNSFailure` | Entrance dns failure |  | todo |  |
| `probe` `TestEntranceFallsBackToBestBackup` | Entrance falls back to best backup |  | todo |  |
| `probe` `TestEntrancePrimaryWinsWhenHealthy` | Entrance primary wins when healthy |  | todo |  |
| `probe` `TestEntranceResolvesDomainWithoutIP` | Entrance resolves domain without ip |  | todo |  |
| `probe` `TestEntranceTimeout` | Entrance timeout |  | todo |  |
| `probe` `TestEntranceUsesLiteralIP` | Entrance uses literal ip |  | todo |  |
| `probe` `TestParseMethod` | Parse method |  | todo |  |
| `probe` `TestPingLoopback` | exercises the real unprivileged ICMP implementation. |  | todo |  |
| `probe` `TestPingTimeoutAndCancel` | Ping timeout and cancel |  | todo |  |
| `profile` `TestEntryIPOptional` | Entry ip optional |  | todo |  |
| `profile` `TestFixtureProfileParsesAndValidates` | Fixture profile parses and validates |  | todo |  |
| `profile` `TestIngressFailoverShapes` | Ingress failover shapes |  | todo |  |
| `profile` `TestIngressLabelIsOptionalDisplayOnly` | Ingress label is optional display only |  | todo |  |
| `profile` `TestIsPrivateIP` | Is private ip |  | todo |  |
| `profile` `TestMaxIngressesAccepted` | Max ingresses accepted |  | todo |  |
| `profile` `TestParseIgnoresUnknownFields` | Parse ignores unknown fields |  | todo |  |
| `profile` `TestParseRejectsSingBoxConfig` | Parse rejects sing box config |  | todo |  |
| `profile` `TestProtocolFieldsFailClosed` | Protocol fields fail closed |  | todo |  |
| `profile` `TestRealityServerNameIsBorrowed` | Reality server name is borrowed |  | todo |  |
| `profile` `TestReplicaOrdinalPresenceRequired` | Replica ordinal presence required |  | todo |  |
| `profile` `TestReservedEntryIPsRejected` | Reserved entry i ps rejected |  | todo |  |
| `profile` `TestRoutingActionUnionIsStrict` | Routing action union is strict |  | todo |  |
| `profile` `TestRoutingValidationIDNAPortsCIDRAndStrictJSON` | Routing validation idna ports cidr and strict json |  | todo |  |
| `profile` `TestRoutingValidationRejectsDuplicateIDsAndUnknownNodes` | Routing validation rejects duplicate i ds and unknown nodes |  | todo |  |
| `profile` `TestRuleSetHostPinning` | Rule set host pinning |  | todo |  |
| `profile` `TestRuleSetUpdateIntervalClamp` | Rule set update interval clamp |  | todo |  |
| `profile` `TestRuleSetValidation` | Rule set validation |  | todo |  |
| `profile` `TestSchemaIncompatible` | Schema incompatible |  | todo |  |
| `profile` `TestShadowsocksServerKeyRejected` | Shadowsocks server key rejected |  | todo |  |
| `profile` `TestShadowsocksUserKeyRequiredIdentityKeysOptional` | Shadowsocks user key required identity keys optional |  | todo |  |
| `profile` `TestSupportedProtocolsValidate` | Supported protocols validate |  | todo |  |
| `profile` `TestTLSServerNameMustMatchEndpointDomain` | Tls server name must match endpoint domain |  | todo |  |
| `routing` `TestAddressMatchersAreORWhileProtocolAndPortAreAND` | Address matchers are or while protocol and port are and |  | todo |  |
| `routing` `TestCIDRProtocolPortPrivateAndRange` | Cidr protocol port private and range |  | todo |  |
| `routing` `TestClassifierPrivateMatchesSharedRanges` | ip_is_private matches the same ranges as the sing-box rules (profile.PrivatePrefixes), not just Go's RFC 1918/ULA IsPrivate. |  | todo |  |
| `routing` `TestClassifierSkipsRuleSetOnlyRules` | Classifier skips rule set only rules |  | todo |  |
| `routing` `TestFirstMatchDomainIDNAAndLabelBoundary` | First match domain idna and label boundary |  | todo |  |
| `routing` `TestFixedPriorityAndSelectedSnapshot` | Fixed priority and selected snapshot |  | todo |  |
| `version` `TestPublishedCapabilitiesMatchProfileAndHideImplementation` | Published capabilities match profile and hide implementation |  | todo |  |
