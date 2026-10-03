# Rust 版与 Go 版的行为对照

Go core 冻结在 v0.5.21（#45）。Rust 版 `ppvpn-core`（`crates/ppvpn-core`）硬切换前，下表每一行都要有结论：

- **todo**：还没有对应的 Rust 用例；
- **done**：`Rust 用例` 一列写明对应的测试（crate 路径和名称），行为与 Go 一致；
- **n-a**：不适用，`备注` 写明原因（例如只属于 sing-box 配置、或者没有宿主使用的接口）。

行为不一致、但决定就这样改的，也算 done，必须在 `备注` 里写明差异和决定出处；涉及 golden 的偏离统一列在下面"Rust 有意偏离 Go golden 的行为"一节。没有结论的行不能硬切换。

配套的语言无关基准在 [`testdata/golden/`](../testdata/golden/README.md)：Core API 契约（`contract/`）和路由判定（`routing/`）。Rust 跑同一组文件，即可覆盖其中的行为；对应的 Go 运行器（`TestGoldenContract`、`TestGoldenRouting`）在下表标为 n-a。

分组沿用 #45 评审意见里的八组。"行为摘要"取自 Go 测试的注释，没有注释的取测试名；细节以 Go 测试为准。

新增 Go 测试（只允许测试和文档）时，在对应分组里加一行。

## Rust 有意偏离 Go golden 的行为

以下几项 Go 0.5.21 的行为已按现状记录在 `testdata/golden/contract`。#45 的待定项 D1–D3 已经决定（2026-10-03）：Rust 版修正这些行为，Go 的 golden 不改。Rust 跑这几个步骤时，按下表的"Rust 预期"判定，不按 golden。其余步骤仍按 golden 判定。

| # | golden 步骤 | Go 0.5.21（golden） | Rust 预期 |
| --- | --- | --- | --- |
| D1 | `lifecycle.json` `start_without_profile` | `CORE_OPERATION_FAILED`（retryable=false） | `PROFILE_NOT_APPLIED`（retryable=false），不发事件 |
| D2 | `selection.json` `select_unknown` | `CORE_OPERATION_FAILED` | `NODE_NOT_FOUND`（field=`node_id`，retryable=false），与 `pin-ingress` 一致；选中的节点不变 |
| D2 | `selection.json` `select_before_profile` | `CORE_OPERATION_FAILED` | `PROFILE_NOT_APPLIED`（retryable=false） |
| D3 | `apply_dedupe.json` `apply_unknown_default_node_keeps_selection` | 接受（`applied=true`，`ProfileApplied`）：Go 在校验前先用当前选中的节点覆盖了 `default_node_id` | apply 先校验原始 Profile：`default_node_id` 不存在就报 `DEFAULT_NODE_NOT_FOUND`（field=`selection.default_node_id`），与 `validate-profile` 一致，已应用的 Profile 不变，发 `ReloadFailed`。校验通过后，选中的节点取宿主随 apply 传入的 `selected_node_id`（它仍在新 Profile 里时）；宿主不传，就用新 Profile 的 `default_node_id`（host-integration 4.1） |

D3 的连带影响：同一文件里后面的 `status_r3` 和 `status_still_r3`，在 Rust 下 `revision` 仍是 `2026-09-29T00:00:00Z#2`，因为 r3 被拒绝了；这两步也按此判定。

这几步（D1、D2、D3）由 `tests/golden_contract.rs` 的 `scenarios_match_the_go_golden` 按"Rust 预期"判定（`scenario_departure`）。

不在 golden 里的偏离：

- **TunRoutingRestored**（Core 组 2026-10-03 定）：Go 0.5.20 只在 `TunRoutingBroken` 之后才发；Rust 每次自愈补回都发，也就是从 `Degraded{TunRoutingRestoring}` 退出时发（host-integration 第 5、6 节）。在 Rust 的状态机里，Broken 属于 `Fatal`，之后不会再补回，所以照 Go 的做法这个事件就永远发不出来。宿主对它的处理应当是幂等的。

D3 改变的只是失效的 `default_node_id`。选择由宿主持久化，引擎不留隐藏状态（2026-10-03 决定）：宿主每次 apply 都传入它保存的 `selected_node_id`，所以 `TestSameRevisionNoopAndMigrationKeepsSelection` 的"保持选择"在宿主传入选择时成立；宿主不传，就回到 `default_node_id`，重建实例和不重建的结果一样。

Profile 本身的解码错误也有一项偏离（#45 待定项 D5，2026-10-03 决定）。Go 的 IPC 层把这类错误折叠成 `CORE_OPERATION_FAILED`；库形态直接报 Profile 的问题：

| # | golden 步骤 | Go 0.5.21（golden） | Rust 预期 |
| --- | --- | --- | --- |
| D5 | `validation.json` `profile_missing`（空 Profile） | `CORE_OPERATION_FAILED` | `PROFILE_REQUIRED`（retryable=false） |
| D5 | （golden 没有对应步骤）Profile 不是合法 JSON，或者字段类型不对（例如 `port` 大于 65535） | `CORE_OPERATION_FAILED` | `PROFILE_MALFORMED`（retryable=false） |

`validation.json` 的 `request_invalid_unknown_field`（请求体里有未知字段）只在 IPC 下存在，库里没有对应的情形，标为 n-a。

lab 用例里也有一项偏离（#45 待定项 D4，2026-10-03 决定：Rust 版遵守 `capabilities.udp`；Backend 已确认生产上所有入口下发的都是 `udp=true`，所以不影响现有用户）：

| # | lab 用例 | Go 0.5.21（baseline） | Rust 预期 |
| --- | --- | --- | --- |
| D4 | `test/lab/engine/cases/udp.sh` `udp.3` | UDP 被规则路由到 `capabilities.udp=false` 的节点（lab 的 `us`，AnyTLS），仍经该节点发出（出口 `.13`）：Go 在路由时不检查这个字段 | 立即拒绝：不经该节点，也不改走别的节点或直连；记一行 debug 日志说明原因（节点或入口 `udp=false`）。多入口节点做故障转移时，`udp=false` 的入口不承接 UDP。`udp.3` 在 Rust 下应判为没有应答 |

## Rust 新增的行为

下面这些是 Go 0.5.21 没有的新能力。它们不改变 Go 已有的行为，所以不算偏离；Rust 版要另写用例覆盖，并在这里登记结论。

| # | 行为 | 来源 | Rust 用例 | 状态 |
| --- | --- | --- | --- | --- |
| N1 | 越过 `expires_at` 时进入 `Degraded{ProfileExpired}`，由定时器触发；转发照常，apply 一份未过期的 Profile 后清除 | #86（Core 定） | | todo |
| N2 | macOS 和 Windows 上的 TUN 路由完整性：被删时检测并上报，能自愈就自愈（`TunRouting*`，`Degraded`/`Fatal`）；Linux 沿用 Go 0.5.20 的规则守护 | #86（Desktop B） | | todo（G5 实机验收） |

## 硬切换前的阻塞项

下面几项在 Rust 版里还没有完整对应 Go 的行为，或者还没在 lab 里验证过。全部解决之前不能硬切换。

| # | 行为 | Go 0.5.21 | Rust 现状 | 还缺什么 |
| --- | --- | --- | --- | --- |
| X1 | 主机有 IPv6 但没有自己的 IPv6 出口时，直连双栈域名仍然能通（0.5.17 修复，验收清单"功能与行为"） | `handOffDirectIPv6`：`direct` 换成 `domaindest(ipv6_only)` 包装 `direct-host`，后者解析时只取 IPv4；TUN 本身不变 | 翻译层已实现（`translate::tests::without_a_host_ipv6_path_direct_hands_global_ipv6_its_domain`），改用 sail 现有能力：TUN inbound 加一条匹配 `2000::/3` 的 route-options 规则，设 `override_destination: "proxy_and_direct"`（后面规则的值优先，sail 有同样形状的路由测试）；`direct` 带 `domain_resolver {dns-local, ipv4_only}`。不需要 Sail 改动 | ① 探测已接入 Engine（`engine::tun`：每次 apply 和 start 各探一次并写日志，结果填到 `Tun.no_host_ipv6_route`；`reprobe_host_ipv6` 在结果变化时 reload 并发 `KernelSwitched`）；网络变化时由 `engine::network` 触发，最后一次变化 2 s 后探测（同 Go 的防抖），离线期间跳过（过渡实现的来源见上）；② kernel 切换或 reload 后，反向映射是否还在（Go 用共享的存储，见 `TestRestoreFallsBackToTheSharedReverseMapping`；sail 的 reload 会清空 DNS 缓存，反向映射是否随之清空待确认）；③ lab 用例：IPv6 无出口的主机上，直连双栈域名能通 |

## 测试宿主的约定

- 所有 lab 和性能检查都通过 `ppvpn-core-lab`（Rust 的测试宿主）驱动 Rust 版。它要提供和 `ppvpn-core serve` 相同的命令行、日志格式，以及 lab 实际用到的那部分 Core API v1（#45）。
- `ppvpn-core-lab` 必须能信任测试时现场生成的 CA：性能检查的 AnyTLS 假节点（`tools/perf`）就是这样。Go 版通过 `SSL_CERT_FILE` 实现（只在 Linux 上有效）；Rust 版要支持 `SSL_CERT_FILE`，例如 rustls-native-certs，或者提供等价的命令行参数来注入 CA 文件，否则 `tools/perf/measure.py` 测不了 AnyTLS。

## netns CI（G3、G7）

`.github/workflows/netns.yml` 在每个 PR 和 main 上运行，环境是 GitHub 的 ubuntu-latest runner，测的是冻结的 Go core。每项测试都在 runner 上的独立网络命名空间里运行，经过 `test/netns/run.sh`：它负责超时，并在宿主命名空间里比较测试前后的状态。比较项是 ip rule（v4、v6）、全部路由表（去掉剩余生存期）、网卡名、nftables（不含计数器）、`/etc/resolv.conf`、systemd-resolved 的各网卡 DNS；任何差异都判失败，这一项覆盖 G7 的宿主残留。脚本与引擎无关：Rust 版 ppvpn-core 接入时，只把被测的二进制换成 `ppvpn-core-lab`，脚本和断言不变。

| CI 步骤 | 脚本 | 覆盖的行为 | 相关 Go 测试 | Rust 接入 |
| --- | --- | --- | --- | --- |
| tun：残留检查自检 | `run.sh` 加一个伪造的测试 | 宿主命名空间被改动时，run.sh 必须判失败 | — | 不变 |
| tun：规则补回 | `run.sh` + `runtime.test -test.run TestTUNRulesRestoredAfterDeletion` | 真实 TUN 下，三种删法删掉的策略路由都被补回，宿主不受影响 | 第 2 组 `TestTUNRulesRestoredAfterDeletion` | 已有：`run.sh --libtest ppvpn_core.test tunrules::linux_tests::`（与 Go 并行） |
| tun：规则损坏上报 | 同上，`PPVPN_TEST_TUN_RULES_NO_RESTORE=1` | 补不回来时，状态为 broken，并发出 TunRoutingBroken | 第 2 组 `TestTUNRulesBrokenIsReported` | 同上（同一步，进程内关掉补回） |
| network-change：dns-local 跟随网络变化 | `run.sh --host test/lab/localdns/run.sh` | 切到另一块网卡、同一网卡换网络（新地址和新 DNS）、读不到 DNS 时在 500 ms 内回 SERVFAIL、DNS 出现后 1.5 s 内恢复、127.0.0.1 陷阱始终没被查询、发往物理 DNS 的查询不进 TUN、每次变化都有 `local dns servers` 日志 | 第 3 组 `TestCacheFollowsInterfaceChanges`、`TestCacheFailsFastWithoutServers`、`TestExchangeWithoutServersAnswersServfailAtOnce`（单元层面）；对应 #45 dns-local 用例 D1、E1–E3、E5、E6（检查项带用例编号） | `network-change-rust`：`CORE_ENGINE=rust`，严格模式；服务器从命名空间的 resolv.conf 读，网卡变化看 `NetworkChanged` 事件。**未通过**（continue-on-error），见下文 |
| network-change：断网恢复，模式 0–4 | `run.sh --host test/lab/localdns/updown.sh` | up 后 2.5 s 内恢复；断网期间不切换内核（#69）；整轮切换次数：IPv6 不再回来时为 1，其余为 0；在断网状态下启动；网卡 up 后有连续的 netlink 事件 | 第 5 组 `TestReprobeSkipsWhileOffline`、`TestReprobeSwitchesWhenIPv6PathIsLost`（单元层面）；对应 dns-local 用例 E4 | `network-change-rust`，内核切换看 `KernelSwitched` 事件。**未通过**（continue-on-error），见下文 |

**Go 0.5.21 的已知滞后（Rust 必须修好）**：前端的网卡监视器报告默认网卡变化后，dns-local 和直连拨号用的是**内核自己的**监视器，网络事件连续不断时可能晚几秒才跟上。原因是 sing-tun 每收到一个 netlink 事件，就把 1 秒的检查重新计时；各个盒子的节奏不同，某一个就可能一直被推迟（#45；和 Desktop 在 Linux 实机上恢复慢 5.2 秒（#69）是同一个根源）。CI 里 network-change 对 Go 用 `SWITCH_GRACE_MS=6000`：在 6 秒内跟上才算通过，日志里记下实际滞后和第一次查询的结果。前端监视器自己也会被同样推迟（CI 上见过 5146 ms 才报告变化），所以有宽限时，等待"变化被报告"的上限是 10 秒（`CHANGE_REPORT_MS` 可改），日志里记下实际耗时。Rust 版的 ppvpn-core 必须在 `SWITCH_GRACE_MS=0`（默认）下通过：变化在 2 秒内被报告，变化后的第一次查询就用新网络。做法是全程只用一个监视器（同一个事件源同时用于日志、DNS 和拨号），并且防抖要有上限，不能被持续的事件无限推迟。

**不在 CI 里的**（继续在共享测试主机上用 hostq 跑，见 `test/lab/engine`）：
- 真实节点、弱网（netem）、长时间运行和内存（G4、G6）；
- Docker 多节点 lab 的场景（G2：B1–B7、t3/t4/t56/t9），在移植到 runner 之前都在这里；
- systemd-networkd 管理的链路抖动（`TestTUNRulesSurviveNetworkdLinkFlap`），runner 上没有 networkd 管理的链路。

**Rust 的 network-change（2026-10-03，在共享的 Linux 测试机上实跑，与 CI 的 `network-change-rust` 相同）**：apply 和 start 已通过；宿主无残留。其余未通过，缺的是 Engine 的这几块，补齐后去掉 continue-on-error：
- Tun 实例还不打开 TUN（`tun interface: none`）：E1、E2、E3、E5 的查询和 updown 各模式的恢复都失败；D1 的抓包断言要求 TUN 存在，不会空过。
- dns-local 还不是 core 自己的监听（#119）。在那之前翻译用的是 Sail 的 `local`，它走系统解析器：resolv.conf 为空时 glibc 退回 127.0.0.1，陷阱被查询（E6 失败）。
- `NetworkChanged` 还没有事件源（Sail 的网络事件未接入）：三次"2 秒内报告变化"都失败。
- `Engine::logs()` 还是空实现：没有 `local dns servers` 日志行。
- updown 模式 2 需要 host IPv6 重探后切换一次内核（`KernelSwitched`）；其余模式切换次数为 0 是空过。

D2（关掉 socket 绑定的变异构建必须让 D1 失败）需要一个只给 lab 用的开关来构建不绑定的 core，Engine 里还没有，暂缺。D3（入口只给域名）需要节点，在 `test/lab/engine` 的 t4 里跑（`lab.sh up ... <ppvpn-core-lab>`，引擎 `rust`），同样等 TUN。

## 过渡实现

Core 组 2026-10-03 决定：网卡变化以 sail 的监视器为唯一来源，Engine 不自己监视网卡，也不调用 `network_changed`。所有依赖网络变化的逻辑都由 sail 的网络事件驱动（`Event::Network`：InterfaceChanged、Moved、Offline、Restored，加上 `instance.network()` 快照），包括：NetworkChanged 事件；`Degraded{NoDefaultInterface}` 的进入和退出；探测在离线时立即返回 `NO_DEFAULT_INTERFACE`；主机 IPv6 出口的重新探测（`hostipv6::route`，在 Restored、InterfaceChanged、Moved 时触发）；离线期间不做重新探测（#69）。

`Runtime::network()` / `network_changes()`（`runtime/sail.rs`）现在直接用 `sail::embed` 的 `instance.network()` 和 `instance.events(Kinds::NETWORK)`。订阅在 Runtime 创建时建立，跨越每次启动和停止都有效；落后时收到 `Lagged`，就按快照补一次变化（reason=`lagged`）。不再通过 `manager()`，也没有轮询，过渡已经结束。sail 的事件映射到 Engine：`InterfaceChanged`、`Moved`、`Restored` 映射为 `NetworkChanged`，`Offline` 映射为 `Degraded{NoDefaultInterface}`（`NetworkChange.change`）。

Engine 侧（`engine/network.rs`）：watcher 订阅 `network_changes()`，每次变化转成 `on_network`（NetworkChanged、`Degraded{NoDefaultInterface}`、探测的离线状态）；TUN 实例在最后一次变化 2 s 后重新探测主机 IPv6 出口；start 时读一次 `network()` 快照，只设离线状态，不报变化；`default interface` 日志行同 Go 的格式，但没有 `mtu`（sail 的快照不带）。

落后时的处理：sail 的事件是有界广播，落后时会收到 `Lagged`，Runtime 按快照补一条 `reason=lagged` 的变化。Engine 还保留按 generation 跳号补报一步的逻辑：跳号、并且这条变化的 `old` 和上次看到的网络不同时，先按 `old` 补报一步（`engine::network::tests::a_missed_step_is_replayed_from_the_changes_old`）。离线判断只看 sail 的 offline 标志（`NetworkChange.change` 为 `offline` 时快照的 offline 为真）。

强杀后的残留清扫（host-integration 第 3 节，切换前必须关掉的缺口）：`Engine::new` 把 sail 的 run_dir 设在 `state_dir/run`，并在清扫时调用 `sail::embed::sweep`。

| 平台 | 状态 | 依据 |
| --- | --- | --- |
| Linux | done | Sail 在 run_dir 的台账记下改动，强杀后由下一次 `new` 的 `sweep` 撤销：ip rule、没有设备的 throw 路由、nft 表、fw4 drop-in；另有 tunrules 按我们的优先级段和表清扫 |
| macOS | done（不靠台账） | 强杀后 utun 和经它的路由随进程消失，由内核回收；Sail 接受 run_dir 但不写台账；Sail 的常驻 CI 每次都验证 |
| Windows | todo（切换前缺口） | Sail 在 Windows 上还没有台账和 sweep；强杀后 Wintun 适配器及其路由、DNS 会不会残留还没测 |

运行中开关系统代理监听，用的是 sail `Instance` 上的 `add_inbound` / `remove_inbound`（`runtime/sail.rs`）；sail 的 reload 不会新增或删除监听，所以不能用 reload 做。`remove_inbound` 停止监听，并由 sail 断开这个 inbound 接进来的全部连接，其他 inbound 的连接不动（`engine::proxy_tests::system_proxy_listener_toggles`、`runtime::sail_tests` 都断言了这一点）。已知缺口（Sail）：多路复用入站上，sail 断开其中的各条流，但暂时不断开承载它们的连接；系统代理监听是 mixed，没有多路复用，不受影响。

## Lab 用例（`test/lab/engine/cases`）

UDP、DNS 劫持和反向映射需要 TUN，在 routing golden（`testdata/golden/routing`）里没有覆盖。它们写成 lab 的用例，在有特权容器的 Linux 主机上跑：`lab.sh case <组> sing|rust`。Go 0.5.21 的输出存为 `cases/<组>.baseline.txt`。Rust 版跑同一个脚本，结论按 id 记在这里。

| id | 行为 | Go 0.5.21 | Rust | 备注 |
| --- | --- | --- | --- | --- |
| `dns-hijack.1`–`.4` | 发往任意 IPv4、IPv6 地址，以及隧道自身 DNS 地址（`10.60.159.90`、`fde2:…::2`）的 53 端口查询都被劫持 | PASS | todo | |
| `dns-hijack.5`–`.6` | 代理路由的域名由 dns-remote 解析，直连路由的域名由 dns-local 解析 | PASS | todo | |
| `dns-hijack.7`–`.8` | 发往服务器 853 端口的 DoT 不被劫持，按普通连接路由 | PASS | todo | |
| `udp.1`–`.2` | UDP 经选中节点；direct 规则下的 UDP 直连 | PASS | todo | |
| `udp.3` | UDP 被规则路由到 `capabilities.udp=false` 的节点（AnyTLS），仍经该节点发出 | PASS | todo | Rust 有意偏离（D4）：拒绝，见上文 |
| `reverse-map.1`–`.4` | 不带 Host 的连接按 DNS 应答的域名交给节点；内核热切换、改选节点后仍然有效 | PASS | todo | 对应 `TestKernelSwitchKeepsReverseMapping` |

未覆盖：QUIC 嗅探（lab 镜像里没有 QUIC 客户端）。

## 1. 热切换和排空

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `internal/runtime` `TestApplyClosesConnectionsANewRuleRejects` | A new reject rule closes the connections it now matches. |  | todo |  |
| `internal/runtime` `TestApplyClosesConnectionsOfRemovedNodes` | Taking a node away closes its connections (and its local proxy user's) on the switch; a connection on a node that stays keeps running. |  | todo |  |
| `internal/runtime` `TestApplyDoesNotDeadlockWithStatusAndAWriter` | An apply in progress must not wedge the core's lock: Status reads the kernel while holding it (activeIngress), and the apply's prepare (pins) takes it. |  | todo |  |
| `internal/runtime` `TestApplyKeepsRunningConnections` | An apply while a download runs (new rules, same nodes) switches kernels without touching the download: it completes in full after the switch, new connections use the new … |  | todo |  |
| `internal/runtime` `TestApplyKernelStartFailureLeavesTheOldKernel` | A kernel that fails to start is discarded: ApplyProfile fails, the old kernel and its connections are untouched, and the old profile stays. |  | todo |  |
| `internal/runtime` `TestApplyReachesConnectionsOfOlderKernels` | A switch applies the new profile to every replaced kernel still draining, not only the one it replaces: a download started two applies earlier is counted as kept, and … |  | todo |  |
| `internal/runtime` `TestAtomicApplyRollback` | Atomic apply rollback | `ppvpn-core` `engine::lifecycle_tests::a_failed_apply_keeps_what_runs_and_says_why` | done | 在 FakeRuntime 上：校验失败、翻译失败、运行时拒绝 reload 时，生效的 Profile 和状态都不变，并发 `ReloadFailed`（带 `code`）；真实 sail 上的回滚由 sail 的 reload 保证（失败不改变任何东西） |
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
| `internal/runtime` `TestRoutingModeSwitchesWithoutNewRevision` | The routing mode is part of what an apply changes: switching it re-applies the same revision, the global mode renders only baseline rules with the selected node as final … | `tests/golden_contract.rs` `scenarios_match_the_go_golden`（`apply_dedupe`）、`ppvpn-core` `translate::tests::global_keeps_baseline_rules_and_proxies_the_rest` | done | |
| `internal/runtime` `TestSameRevisionNoopAndMigrationKeepsSelection` | Same revision noop and migration keeps selection | `ppvpn-core` `engine::lifecycle_tests::apply_dedupes_on_the_live_values`、`tests/golden_contract.rs` `scenarios_match_the_go_golden`（`apply_dedupe`） | done | 去重键是 (revision, routing_mode, selected_node_id, pins)，与当前生效的值比较（host-integration 4.1）。选择由宿主随 apply 传入：传入的节点仍在新 Profile 里就沿用，否则用 `default_node_id` 并返回 `selection_reset` |

## 2. Linux TUN 路由规则守护（tunrules）

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `internal/config` `TestDesktopTUNUsesOwnIPRoute2Namespace` | Desktop tun uses own ip route2 namespace |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/runtime` `TestTUNRulesBrokenIsReported` | : rules that stay missing set the status to broken and send TunRoutingBroken. | `ppvpn-core` `tunrules::linux_tests::tun_rules_broken_is_reported` | done | netns CI：tun 作业（`run.sh --libtest`），在进程内关掉补回，不用环境变量；守护层面断言 `Broken{missing}`，事件和 `Fatal` 由 Engine 接入后的测试覆盖。另验证手动补回后报 `Restored` |
| `internal/runtime` `TestTUNRulesRestoredAfterDeletion` | deletes the TUN's policy routing the ways seen in the field (everything, as networkd does on a link down; just the goto target; the table's routes) and requires each to … | `ppvpn-core` `tunrules::linux_tests::tun_rules_restored_after_deletion` | done | netns CI：tun 作业（`run.sh --libtest`），独立 netns 中真实的 sail TUN；守护层面断言 `Restored`，`Status::tun_routing` 由 Engine 接入后的测试覆盖 |
| `internal/runtime` `TestTUNRulesSurviveNetworkdLinkFlap` | reproduces the field report: with systemd-networkd managing a link (ManageForeignRoutingPolicyRules on, its default), taking the link down and up makes networkd drop the … |  | todo | 不在 CI：runner 没有 systemd-networkd 管理的链路；在容器里跑 |
| `internal/tunrules` `TestMissingCountsDuplicates` | Missing counts duplicates | `ppvpn-core` `tunrules::tests::missing_counts_duplicates` | done |  |
| `internal/tunrules` `TestOwnedKeepsEverySingTunRule` | Owned keeps every sing tun rule | `ppvpn-core` `tunrules::tests::owned_keeps_every_sing_tun_rule` | done | 另有 `owned_keeps_every_sail_auto_route_rule`：sail auto_route 为桌面 TUN 装的全部规则 |
| `internal/tunrules` `TestOwnedLeavesOtherProgramsRules` | Owned leaves other programs rules | `ppvpn-core` `tunrules::tests::owned_leaves_other_programs_rules` | done |  |
| `internal/tunrules` `TestRestoreOrderPutsGotoTargetsFirst` | Restore order puts goto targets first | `ppvpn-core` `tunrules::tests::restore_order_puts_goto_targets_first` | done |  |
| `internal/tunrules` `TestRouteRestoreOrderPutsGatewayCoveringRoutesLast` | Route restore order puts gateway covering routes last | `ppvpn-core` `tunrules::tests::route_restore_order_puts_gateway_covering_routes_last` | done | sail 的路由没有网关，保留这个顺序是为了有网关的路由 |
| `internal/tunrules` `TestRuleString` | Rule string | `ppvpn-core` `tunrules::tests::rule_string` | done | 字符串与 Go 逐字一致（宿主在 `missing` 里看到的就是它） |

## 3. dns-local

dns-local 用自研实现（`crate::localdns` 加上进程内监听），不用 Sail 的 `local`（2026-10-03 决定）。Sail `local` 的缺口作为 Sail 的通用改进继续推进，**不阻塞切换**：忽略 macOS 手动 DNS、丢弃链路本地 DNS、每个服务器没有独立超时、不处理 TC、不过滤回环、没有默认网卡时不立即失败，以及 split DNS 和日志行。翻译层把 dns-local 渲染成指向监听的 tcp 服务器（`translate::tests::dns_local_listener_is_asked_over_tcp`）。下表的 Rust 用例指 `crate::localdns`。

过渡期已知限制（macOS）：TUN 不写网卡名，由 Sail 分配。Sail 改为由内核分配编号并报告实际名字之前，它固定用 `utun233`，这个编号被别的程序占用时启动失败。排除隧道网段由 dns-local 自己的隧道地址过滤保证。

已知行为（macOS，Sail `98a5cbf5`，见 Sail 的 routing 文档）：auto_route 要装的路由如果已经存在（例如另一个 VPN 装的），Sail 会替换它并打一行警告；停止时**不恢复**被替换的路由。

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `cmd/ppvpn-core` `TestServeValidatesLocalDNSServers` | Serve validates local dns servers |  | todo |  |
| `internal/config` `TestLocalDNSServers` | Host-supplied physical resolvers become a static ppvpn-local dns-local with every one outside the tunnel, in order; with none left (or none given) dns-local reads the … |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/localdns` `TestCacheFailsFastWithoutServers` | DHCP has not handed out DNS yet: queries fail at once with a clear error (never 127.0.0.1, never a 5 s timeout), and the interface is read again at most once per … | `ppvpn-core` `localdns::cache::tests::fails_fast_without_servers` | done | #45 B3、B7；端到端仍由 netns CI network-change 覆盖 |
| `internal/localdns` `TestCacheFollowsInterfaceChanges` | Cache follows interface changes | `ppvpn-core` `localdns::cache::tests::follows_interface_changes` | done | #45 B1、B2、B7；端到端仍由 netns CI network-change 覆盖 |
| `internal/localdns` `TestCacheReadsOnceForConcurrentQueries` | Concurrent queries after an invalidation share one read. | `ppvpn-core` `localdns::cache::tests::concurrent_queries_share_one_read` | done | #45 B6 |
| `internal/localdns` `TestCacheRefreshes` | Cache refreshes | `ppvpn-core` `localdns::cache::tests::refreshes` | done | #45 B4、B5 |
| `internal/localdns` `TestExchangeAsksServersInOrder` | Exchange asks servers in order | `ppvpn-core` `localdns::tests::next_server_after_a_silent_one`、`ppvpn-core` `localdns::tests::answer_names_its_upstream` | done | #45 C2、C5；另有 C3 `truncated_answer_retries_over_tcp`、C4 `reply_with_another_id_is_dropped`、C6 `hosts_file_names_are_answered_locally` |
| `internal/localdns` `TestExchangeFailureRereads` | Every server failing marks the read servers suspect, so the next query reads the interface again (after RetryInterval). | `ppvpn-core` `localdns::tests::next_server_after_a_silent_one` | done | #45 B4（全部失败后回 SERVFAIL，下次查询重读） |
| `internal/localdns` `TestExchangeWithoutServersAnswersServfailAtOnce` | Without servers a hijacked query gets SERVFAIL at once (an error would leave the client waiting for its own timeout), with the cause logged. | `ppvpn-core` `localdns::tests::without_servers_servfail_at_once` | done | #45 C1 |
| `internal/localdns` `TestGlobalServers` | Global servers | `ppvpn-core` `localdns::scutil::tests::global_servers_of_the_default_interface`、`ppvpn-core` `localdns::scutil::tests::another_vpns_dns_is_not_the_default_interfaces` | done | #45 A4–A6；fixture 取自 Go 测试（`src/localdns/testdata`） |
| `internal/localdns` `TestScopedServers` | Scoped servers | `ppvpn-core` `localdns::scutil::tests::scoped_servers_of_an_interface` | done | #45 A7 |
| `internal/localdns` `TestUsableLeavesOutTunnelLoopbackAndForeignLinkLocal` | Usable leaves out tunnel loopback and foreign link local | `ppvpn-core` `localdns::servers::tests::usable_leaves_out_tunnel_loopback_and_foreign_link_local` | done | #45 A1–A3；A8（Windows 适配器）见 `adapters::tests` |

## 4. dnstransport guard 与 TUN 远端 DNS

Rust 版的 dns-remote 是 sail 的 `sequential` server，参数和 Go 的 guard 相同：依次问 1.1.1.1、8.8.8.8、9.9.9.9（DoT，经 selected 节点）；每个上游 3 s（`attempt_timeout`），最后一个用剩下的预算；总预算 8 s（`budget`），用完回 SERVFAIL；后备上游答过之后，10 分钟内从它开始问（`prefer_for`）；保持的连接静默超过一半时间时，换新连接重试一次。`dns.timeout` 是 10 s，对应 sing-box 的 `C.DNSTimeout`，Go 的 guard 就运行在它之下，sail 也要求 budget 小于它。所以 SERVFAIL 的时序与 Go 相同，B3（`test/lab/engine/repro/b3-sequential.sh`）和 `t4-host.sh` 4.5 的"8 s 内 SERVFAIL"不受影响。

已知差异：Go 的 `idleReset`（30 s 没有成功且没有进行中的查询时重置连接池）在 sail 里没有对应项。sail 在网络变化（`network_moved`）时重置 DoT/DoH 等长连接；保持的连接超时，也会在同一次查询里换新连接。`TestGuardResetsThePoolAfterIdle` 一行按这个结论处理。

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
| `api` `TestNoDefaultInterfaceIsRetryable` | Offline probes fail fast as NO_DEFAULT_INTERFACE, retryable. | `ppvpn-core` `probe::tests::no_default_interface_is_retryable` | done | Engine 层见 `engine::probes_tests::entrance_probes_on_the_applied_profile` |
| `internal/config` `TestDesktopTUNRoutesIPv6AndExcludesIPv6Ingress` | Desktop TUN carries an IPv6 address so IPv6 (and DNS to IPv6 resolvers) is routed into the tunnel instead of around it; every ingress IP, IPv4 or IPv6, stays excluded. | `ppvpn-core` `translate::tests::desktop_tun_routes_ipv6_and_keeps_ingresses_out`；Engine 层 `ppvpn-core` `engine::tun::tests::a_tun_instance_runs_a_tun_inbound_a_standard_one_does_not` | done | Engine 层断言 Tun 实例交给运行时的配置带 IPv6 地址和 auto_route，Standard 实例没有 TUN |
| `internal/config` `TestDesktopTUNWithoutHostIPv6IsIPv4Only` | A host with IPv6 disabled cannot give the TUN an IPv6 address (sing-tun fails the whole start), so the desktop TUN stays IPv4-only there and no IPv6 ingress prefix is … | `ppvpn-core` `translate::tests::desktop_tun_routes_ipv6_and_keeps_ingresses_out`；Engine 层 `ppvpn-core` `engine::tun::tests::the_probe_decides_the_tun_ipv6_and_the_hand_off` | done | 主机 IPv6 由注入的探测给出：关闭时 TUN 只有 IPv4 地址，IPv6 入口前缀不排除 |
| `internal/config` `TestKnownDomainRegexRejectsIPLiterals` | The fake-ip rule must treat an IP literal (what the HTTP sniffer reports for a request to a bare address) as "no domain". |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/config` `TestNoHostIPv6RouteHandsDirectIPv6ItsDomain` | A host with IPv6 enabled but no IPv6 path of its own keeps the same TUN (IPv6 address and routes, so nothing bypasses it) and only wraps direct. | `ppvpn-core` `translate::tests::without_a_host_ipv6_path_direct_hands_global_ipv6_its_domain`；Engine 层 `ppvpn-core` `engine::tun::tests::the_probe_decides_the_tun_ipv6_and_the_hand_off` | done | 用 sail 的 `override_destination: "proxy_and_direct"` 代替 domaindest，见阻塞项 X1；TUN 本身不变 |
| `internal/config` `TestNoHostIPv6RouteLeavesIPv4OnlyTUNAlone` | The hand-off needs the TUN's IPv6: a host with IPv6 disabled, and mobile (IPv4-only tunnel), render direct as before. | `ppvpn-core` `translate::tests::without_a_host_ipv6_path_direct_hands_global_ipv6_its_domain` | done | 同一用例的后半段：IPv6 关闭和移动平台都不做 hand-off |
| `internal/config` `TestTUNProxyTargetsCarryDomain` | Tun proxy targets carry domain |  | todo | 断言的是生成的 sing-box 配置：Rust 用例应断言等价的产品行为，不是配置形状 |
| `internal/domaindest` `TestRestoreFallsBackToTheSharedReverseMapping` | After a kernel switch the new kernel's own reverse mapping is empty; the shared store still gives direct's ipv6_only wrapper (no host IPv6 path) the domain of a global … |  | todo |  |
| `internal/domaindest` `TestRestoreIPv6Only` | Restore i pv6 only |  | todo |  |
| `internal/domaindest` `TestRestore` | Restore |  | todo |  |
| `internal/hostipv6` `TestDarwinDefaultRouteIndexes` | Darwin default route indexes | `ppvpn-core` `hostipv6::darwin::tests::default_routes_are_up_zero_routes`；另有 `parses_a_route_dump`、`route_needs_the_default_routes_interface_up_with_a_global_address` | done | 解析是纯函数，各平台的测试在所有 CI 平台上都跑；读取系统状态的部分按平台编译 |
| `internal/hostipv6` `TestLinuxAvailability` | Linux availability | `ppvpn-core` `hostipv6::linux::tests::availability` | done | 解析是纯函数，各平台的测试在所有 CI 平台上都跑；读取系统状态的部分按平台编译 |
| `internal/hostipv6` `TestLinuxRoute` | Linux route | `ppvpn-core` `hostipv6::linux::tests::route_needs_a_global_address_and_a_default_route_on_one_interface` | done | 解析是纯函数，各平台的测试在所有 CI 平台上都跑；读取系统状态的部分按平台编译 |
| `internal/hostipv6` `TestRouteOnThisHost` | Route on this host | `ppvpn-core` `hostipv6::tests::route_on_this_host` | done | 解析是纯函数，各平台的测试在所有 CI 平台上都跑；读取系统状态的部分按平台编译 |
| `internal/hostipv6` `TestWindowsAvailability` | Windows availability | `ppvpn-core` `hostipv6::windows::tests::availability` | done | 解析是纯函数，各平台的测试在所有 CI 平台上都跑；读取系统状态的部分按平台编译 |
| `internal/hostipv6` `TestWindowsRoute` | Windows route | `ppvpn-core` `hostipv6::windows::tests::route_needs_an_up_adapter_with_a_global_address_and_a_gateway` | done | 解析是纯函数，各平台的测试在所有 CI 平台上都跑；读取系统状态的部分按平台编译 |
| `internal/reversemap` `TestCapacityEvictsTheEntryClosestToExpiry` | Capacity evicts the entry closest to expiry |  | todo |  |
| `internal/reversemap` `TestRecordLookupAndExpiry` | Record lookup and expiry |  | todo |  |
| `internal/runtime` `TestApplyProfileProbesHostIPv6ForTUN` | The desktop TUN carries IPv6 only when the host probe allows it; the probe runs on every apply because IPv6 can be toggled between starts. | `ppvpn-core` `engine::tun::tests::a_tun_instance_runs_a_tun_inbound_a_standard_one_does_not`、`ppvpn-core` `engine::tun::tests::the_probe_decides_the_tun_ipv6_and_the_hand_off` | done | 探测函数可注入（`Inner::set_host_ipv6_probe`）；apply 和 start 各探一次，Standard 实例和移动平台不探测 |
| `internal/runtime` `TestDefaultInterfaceChangeEmitsNetworkChanged` | Every default interface change of the running engine is reported as NetworkChanged: the new interface's name and index, or none. | `ppvpn-core` `engine::network::tests::every_change_of_the_running_engine_is_network_changed` | done | 来源是 sail 的网络 watch（过渡实现，见上）；离线进入、恢复退出 `Degraded{NoDefaultInterface}` |
| `internal/runtime` `TestDefaultInterfaceLogLine` | Default interface log line |  | todo |  |
| `internal/runtime` `TestHostIPv6RouteDecidesDirectHandOff` | A host with IPv6 enabled but no IPv6 path keeps the IPv6 TUN and wraps direct; the probe runs on every apply and again at start, and its result is logged. | `ppvpn-core` `engine::tun::tests::the_probe_decides_the_tun_ipv6_and_the_hand_off` | done | 读不到出口（`Err`）时照旧使用 IPv6、不做 hand-off；日志一行（`host ipv6`，含结果、policy 和 Err 的原因），日志内容未断言 |
| `internal/runtime` `TestProbesFailFastWithoutDefaultInterface` | With no default interface (offline) both probes fail at once with ErrNoDefaultInterface instead of waiting out their timeout; with one, or when the engine cannot tell, … | `ppvpn-core` `probe::entrance::tests::entrance_offline_fails_fast_and_probes_nothing`、`ppvpn-core` `probe::availability::tests::availability_offline_fails_fast_and_dials_nothing`、`ppvpn-core` `engine::probes_tests::entrance_probes_on_the_applied_profile`、`ppvpn-core` `engine::probes_tests::availability_probes_through_the_node` | done | 探测层：默认网卡状态由参数注入（Unknown/Present 照常探测，Absent 立即失败且不拨号）；Engine 层由 `on_network` 提供（来源是 sail 的网卡事件，接上之前为 Unknown） |
| `internal/runtime` `TestReprobeDebouncesBursts` | A burst of changes (a Wi-Fi switch) re-arms one probe: each change stops the pending one, and only the last fires. | `ppvpn-core` `engine::network::tests::a_burst_of_changes_reprobes_once_after_the_delay` | done | REPROBE_DELAY 2 s，同 Go |
| `internal/runtime` `TestReprobeDefersToApply` | An apply between the change and the probe builds for the new state; the probe then finds nothing to do. |  | todo |  |
| `internal/runtime` `TestReprobeKeepsKernelWhenUnchanged` | Same result: nothing rebuilt, no change logged. | `ppvpn-core` `engine::tun::tests::an_unchanged_ipv6_path_does_nothing` | done | 与 Go 一样，比较的是运行中构建的 hand-off；主机关掉 IPv6 留给下一次 apply 或 start |
| `internal/runtime` `TestReprobeRealTimerFires` | The default scheduler is time.AfterFunc: a change still leads to a probe on its own (with a short delay and a generous deadline). |  | todo |  |
| `internal/runtime` `TestReprobeSkipsWhileOffline` | A link goes down: the path looks lost only because there is no network. | `ppvpn-core` `engine::tun::tests::offline_or_stopped_does_not_probe` | done | 离线状态取自 `on_network`（来源是 sail 的网卡事件，E1b，未接线）；端到端：netns CI network-change，updown 模式 0–4 |
| `internal/runtime` `TestReprobeSwitchesWhenIPv6PathAppears` | The host gains an IPv6 path: switch back to the plain build. | `ppvpn-core` `engine::tun::tests::a_changed_ipv6_path_switches_kernels` | done | reload 后发 `KernelSwitched`；连接数要等排空（第 1 组）接上，目前为 0 |
| `internal/runtime` `TestReprobeSwitchesWhenIPv6PathIsLost` | The host loses its IPv6 path (joins an IPv4-only network): one kernel switch to the hand-off build, no restart, armed ReprobeDelay out. | `ppvpn-core` `engine::tun::tests::a_changed_ipv6_path_switches_kernels` | done | reload 后发 `KernelSwitched`；Go 的 ReprobeDelay 防抖随网卡事件接线实现。端到端：netns CI updown 模式 2（在线时切换一次） |
| `internal/runtime` `TestStopCancelsPendingReprobe` | Stop cancels a pending re-probe. | `ppvpn-core` `engine::network::tests::stop_cancels_a_pending_reprobe` | done |  |
| `internal/runtime` `TestTUNApplyWithoutIPv6PathSwitchesKernels` | With the host's IPv6 state unchanged, a TUN apply is a kernel switch: the no-IPv6-path build (direct wrapped, direct-host resolving IPv4 only, see #48) changes … |  | todo |  |
| `internal/runtime` `TestTUNRouteResolvesAndHandsDomainsToNode` | runs the TUN route and DNS configuration on a real sing-box. |  | todo |  |

## 6. failover 与 pin

| Go 测试 | 行为摘要 | Rust 用例 | 状态 | 备注 |
| --- | --- | --- | --- | --- |
| `api` `TestPinIngressEndpoint` | Pin ingress endpoint | `tests/golden_contract.rs` `scenarios_match_the_go_golden`（`pin_ingress`）、`ppvpn-core` `engine::selection_tests::pin_ingress_validates_then_pins_and_unpins` | done | `endpoint_key` 为 `None` 时恢复自动（选中节点 selector 里的 `<tag>-auto`） |
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
| `api` `TestLocalProxyAPIsReportDisabledCore` | Local proxy ap is report disabled core | `ppvpn-core` `engine::proxy_tests::instances_without_a_local_proxy_refuse_its_calls` | done | TUN 实例和没有本地代理的 Standard 实例；`get-local-proxy-endpoints` 在库里不提供 |
| `api` `TestLocalProxyMetadataAndCredentialAreSeparated` | Local proxy metadata and credential are separated | `ppvpn-core` `engine::proxy_tests::metadata_and_credentials_are_separate_and_follow_the_profile` | done | 有意不同：routed 凭据在 apply 之前就能读（host-integration 4.6）；`kind` 的请求校验（REQUEST_INVALID）属于 IPC，库里是两个方法 |
| `api` `TestProbeEntrancesMethodAndShape` | Probe entrances method and shape | `ppvpn-core` `probe::entrance::tests::entrance_node_filter_and_shape`、`ppvpn-core` `probe::entrance::tests::parse_method` | done | method 是枚举，未知方法在解码时被拒；映射成 `PROBE_METHOD_UNSUPPORTED` 是将来 FFI 解码层的事 |
| `api` `TestRoutingModeOnApplyAndStatus` | routing_mode is optional on validate/apply-profile, strictly checked, and reported by get-status; switching it re-applies the same revision. | `tests/golden_contract.rs` `scenarios_match_the_go_golden`（`apply_dedupe`） | done | routing_mode 在库里是类型化参数；`ROUTING_MODE_INVALID` 来自 `RoutingMode::parse` |
| `api` `TestRuleSetHostsArePinnedAtValidateAndApply` | Rule set hosts are pinned at validate and apply |  | todo |  |
| `api` `TestSetSystemProxyToggleAndStatus` | Set system proxy toggle and status | `ppvpn-core` `engine::proxy_tests::system_proxy_listener_toggles` | done | 运行中开关走 reload（Go 原地增删监听），见 host-integration 4.6 |
| `api` `TestSystemProxyUnavailableWithoutStateOrInTUNCore` | System proxy unavailable without state or in tun core | `ppvpn-core` `engine::proxy_tests::instances_without_a_local_proxy_refuse_its_calls` | done | 库里没有"无状态路径"的情形：`system_proxy=false` 的实例和 TUN 实例返回 SYSTEM_PROXY_UNAVAILABLE |
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
| `internal/corelog` `TestDebugLinesOnlyAtDebugLevel` | Debug lines only at debug level | `ppvpn-core` `engine::logs::tests::levels_and_instances_decide_who_gets_a_line` | done | 级别按实例：同一进程里 info 实例收不到 debug 行 |
| `internal/corelog` `TestFileLogIsWrittenImmediately` | File log is written immediately | `ppvpn-core` `engine::logs::tests::a_file_gets_both_appended` | done | 写盘在引擎自己的线程里，队列空了就 flush，测试等到行出现为止 |
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
| `internal/rulesets` `TestDownloadsRefuseRedirects` | Downloads refuse redirects | `ppvpn-core` `rulesets::tests::downloads_refuse_redirects` | done |  |
| `internal/rulesets` `TestFailedUpdateKeepsLastGoodCopy` | A new profile version that cannot be fetched keeps the last good copy. | `ppvpn-core` `rulesets::tests::failed_update_keeps_last_good_copy` | done |  |
| `internal/rulesets` `TestInspectClassifiesDNSMirroring` | Inspect classifies dns mirroring | `ppvpn-core` `rulesets::tests::inspect_classifies_dns_mirroring` | done | 有意偏离：Rust 只接受 sail 能读的 .srs（版本到 5；AdGuard、`network_interface_address`、`default_interface_address` 判为 `RULE_SET_INVALID`），Go 1.13 读到版本 4 且接受这些条目。sail 读不了的集合不能交给内核（`rulesets::srs::tests`） |
| `internal/rulesets` `TestPathStaysInsideDir` | Path stays inside dir | `ppvpn-core` `rulesets::tests::path_stays_inside_dir` | done |  |
| `internal/rulesets` `TestPrepareDownloadsVerifiesAndReusesCache` | Prepare downloads verifies and reuses cache | `ppvpn-core` `rulesets::tests::prepare_downloads_verifies_and_reuses_cache` | done |  |
| `internal/rulesets` `TestPrepareRejectsDigestMismatchAndForeignHosts` | Prepare rejects digest mismatch and foreign hosts | `ppvpn-core` `rulesets::tests::prepare_rejects_digest_mismatch_and_foreign_hosts` | done |  |
| `internal/rulesets` `TestPrepareRejectsInvalidRuleSet` | Prepare rejects invalid rule set | `ppvpn-core` `rulesets::tests::prepare_rejects_invalid_rule_set` | done |  |
| `internal/rulesets` `TestRecoverySweepsAllSetsAndRebuildsOnce` | When one set recovers, every other set that is not ready is retried at once (not on its own, possibly long, backoff), the downloads run concurrently, and the … | `ppvpn-core` `rulesets::tests::recovery_sweeps_all_sets_and_rebuilds_once` | done |  |
| `internal/rulesets` `TestRecoveryTriggersRebuild` | A set that was never downloaded is retried; once it arrives the manager asks for a rebuild so the skipped rules take effect. | `ppvpn-core` `rulesets::tests::recovery_triggers_rebuild` | done |  |
| `internal/rulesets` `TestRefreshUsesETag` | A ready set is refreshed on its interval with If-None-Match and stays ready on 304. | `ppvpn-core` `rulesets::tests::refresh_uses_etag` | done |  |
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
| `internal/runtime` `TestStandardCoreSelectNode` | drives select-node on a real non-TUN core, both before start (the selection must survive Start) and while running. | `ppvpn-core` `engine::selection_tests::select_node_before_and_after_a_profile` | done | 在 FakeRuntime 上：start 前选择的节点作为 `selected` 的默认值，运行中调用 `select("selected", 节点 tag)`；真实 sail 的 selector 由 `runtime::sail` 的测试覆盖 |
| `internal/runtime` `TestSystemProxyFollowsSelectedNodeAndRules` | runs the standard (non-TUN) core with the system proxy toggled at runtime: traffic follows the selected node and the profile's DIRECT rule, counts toward traffic, and … |  | todo |  |
| `internal/runtime` `TestSystemProxyStartFallsBackWhenPortTaken` | System proxy start falls back when port taken | `ppvpn-core` `engine::proxy_tests::system_proxy_start_falls_back_when_port_taken` | done |  |
| `internal/runtime` `TestSystemProxyUnavailableInTUNCore` | System proxy unavailable in tun core | `ppvpn-core` `engine::proxy_tests::instances_without_a_local_proxy_refuse_its_calls` | done |  |
| `internal/runtime` `TestTUNOnlyCoreRejectsLocalProxyAPIs` | covers `serve --tun --local-proxy=false`. | `ppvpn-core` `engine::proxy_tests::instances_without_a_local_proxy_refuse_its_calls`、`ppvpn-core` `engine::probes_tests::a_tun_instance_does_not_probe_availability` | done | TUN 实例的 selected_ingress 由 `engine::selection_tests` 覆盖 |
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
| `localproxy` `TestMigratesVersion1StateInPlace` | Migrates version1 state in place | `ppvpn-core` `localproxy::tests::migrates_version_1_state_in_place` | done |  |
| `localproxy` `TestParseUsername` | Parse username | `ppvpn-core` `localproxy::tests::parse_username_splits_prefix_and_node` | done |  |
| `localproxy` `TestPrefixesAreRandomLowercaseAlphanumerics` | Prefixes are random lowercase alphanumerics | `ppvpn-core` `localproxy::tests::prefixes_are_random_lowercase_alphanumerics` | done |  |
| `localproxy` `TestRejectsUnsupportedOrCorruptState` | Rejects unsupported or corrupt state | `ppvpn-core` `localproxy::tests::unsupported_or_corrupt_state_is_rebuilt` | done | 有意偏离：Go 拒绝这类文件（serve 起不来，要手工删文件）；Rust 重建 prefix、密码和端口并记一行 warn，读文件本身的 I/O 错误仍然报错（`unreadable_state_fails`）。决定出处：本 PR（Rust 移植约定"损坏时重建"） |
| `localproxy` `TestRejectsWeakStatePermissions` | Rejects weak state permissions | `ppvpn-core` `localproxy::tests::weak_state_permissions_renew_the_secret` | done | 有意偏离：Go 拒绝；Rust 保留端口、重新生成 prefix 和密码（旧密码已不算秘密），按 0600 写回。只在 Unix 上检查；Windows 的 ACL 由宿主的私有目录保证，Rust 不设置 ACL（Go 用 privateacl） |
| `localproxy` `TestRemovedNodeHasNoEndpoint` | Removed node has no endpoint | `ppvpn-core` `localproxy::tests::removed_node_has_no_endpoint` | done | 未知节点报 `NODE_NOT_FOUND`（field=`node_id`） |
| `localproxy` `TestRoutedEndpoint` | Routed endpoint | `ppvpn-core` `localproxy::tests::routed_user_is_the_bare_prefix` | done | 有意偏离：Go 没有节点 endpoint 时没有 routed 用户（`PROFILE_NOT_APPLIED`）；Rust 在 `new` 之后、apply 之前就能读 routed 凭据，metadata 只含 routed 一项（host-integration.md 第 3 节） |
| `localproxy` `TestSharedEndpointsAreStableAndPrivate` | Shared endpoints are stable and private | `ppvpn-core` `localproxy::tests::shared_endpoints_are_stable_and_private` | done | 另断言状态文件不含节点 id（第 4.6 节：`state_dir` 不存 Profile） |
| `localproxy` `TestStartupPrefers7890AndFallsBackWhenBusy` | Startup prefers7890 and falls back when busy | `ppvpn-core` `localproxy::tests::startup_prefers_7890_and_falls_back_when_busy` | done |  |
| `localproxy` `TestStartupReplacesOccupiedPersistedPortOnly` | Startup replaces occupied persisted port only | `ppvpn-core` `localproxy::tests::startup_replaces_occupied_persisted_port_only` | done | Go 的 `Ensure`/`ReconcileForStartup` 对应 Rust 的内存状态/`reconcile_port`；端口变化时返回 `LocalProxyEndpointChanged` |
| `localproxy` `TestStartupUsesPreferredPortWhenFree` | Startup uses preferred port when free | `ppvpn-core` `localproxy::tests::startup_uses_preferred_port_when_free` | done |  |
| `localproxy` `TestSystemProxyPortPrefers7891FallsBackAndPersists` | System proxy port prefers7891 falls back and persists | `ppvpn-core` `localproxy::tests::system_proxy_port_prefers_7891_falls_back_and_persists` | done |  |
| `localproxy` `TestSystemProxyPortUsesPreferredWhenFree` | System proxy port uses preferred when free | `ppvpn-core` `localproxy::tests::system_proxy_port_uses_preferred_when_free` | done |  |
| `mobile` `TestBridgeDoesNotExposeNodeCredentials` | Bridge does not expose node credentials |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `mobile` `TestBridgeRejectsUnknownRoutingMode` | Bridge rejects unknown routing mode |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `mobile` `TestFlowIOTimeoutZeroMeansNoDeadline` | Flow io timeout zero means no deadline |  | n-a | flow adapter / mobile bridge：目前没有宿主使用（#45、#71），Rust 不提供 |
| `probe` `TestAvailabilityCancellation` | Availability cancellation | `ppvpn-core` `probe::availability::tests::availability_cancellation` | done |  |
| `probe` `TestAvailabilityUsesAuthenticatedNodeProxy` | Availability uses authenticated node proxy | `ppvpn-core` `probe::availability::tests::availability_goes_through_the_node_outbound` | done | Rust 不经本地代理用户，直接经节点 outbound（`Runtime::dial_tcp`）发 GET，与本地代理用户的去向相同；另有 `availability_status_and_redirects`、`availability_failures` |
| `probe` `TestEntranceAllFailedReportsPrimary` | Entrance all failed reports primary | `ppvpn-core` `probe::entrance::tests::entrance_all_failed_reports_primary` | done |  |
| `probe` `TestEntranceCanceled` | Entrance canceled | `ppvpn-core` `probe::entrance::tests::entrance_canceled` | done | 另有 `entrance_cancel_ends_probes_in_flight` |
| `probe` `TestEntranceDNSFailure` | Entrance dns failure | `ppvpn-core` `probe::entrance::tests::entrance_dns_failure` | done |  |
| `probe` `TestEntranceFallsBackToBestBackup` | Entrance falls back to best backup | `ppvpn-core` `probe::entrance::tests::entrance_falls_back_to_best_backup` | done |  |
| `probe` `TestEntrancePrimaryWinsWhenHealthy` | Entrance primary wins when healthy | `ppvpn-core` `probe::entrance::tests::entrance_primary_wins_when_healthy` | done |  |
| `probe` `TestEntranceResolvesDomainWithoutIP` | Entrance resolves domain without ip | `ppvpn-core` `probe::entrance::tests::entrance_resolves_domain_without_ip` | done |  |
| `probe` `TestEntranceTimeout` | Entrance timeout | `ppvpn-core` `probe::entrance::tests::entrance_timeout` | done |  |
| `probe` `TestEntranceUsesLiteralIP` | Entrance uses literal ip | `ppvpn-core` `probe::entrance::tests::entrance_uses_literal_ip` | done |  |
| `probe` `TestParseMethod` | Parse method | `ppvpn-core` `probe::entrance::tests::parse_method` | done | 空字符串同 Go 读作 tcp（serde alias） |
| `probe` `TestPingLoopback` | exercises the real unprivileged ICMP implementation. | `ppvpn-core` `probe::icmp::tests::ping_loopback` | done | 不允许无特权 ICMP 的主机上跳过，同 Go |
| `probe` `TestPingTimeoutAndCancel` | Ping timeout and cancel | `ppvpn-core` `probe::icmp::tests::ping_timeout_and_cancel` | done | 取消即丢弃 future |
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
