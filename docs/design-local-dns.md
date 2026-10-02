# 设计：core 自己维护的本地 DNS（dns-local）

状态：已确认（2026-10-02），已实现，目标 0.5.20。实现与本稿的差异和补充见第 7 节。

## 1. 问题

增强模式（TUN）下，直连域名由 `dns-local` 解析：直接向物理网络的 DNS 服务器发查询，socket 绑在物理网卡上（`auto_detect_interface`）。
现状三个平台各不相同：

| 平台 | 现在的 dns-local | 网络变化后的行为 |
| --- | --- | --- |
| Windows | sing-box `local`：读非隧道网卡的 DNS（`GetAdaptersAddresses`，要求网卡 Up、有网关） | 配置缓存在进程里，**最多 5 秒、而且只在有查询时**才重读；网卡变化时不刷新（`Reset()` 是空函数）。一块可用网卡都没有时退回 `defaultNS`（127.0.0.1:53、[::1]:53），超时 5 秒、重试 2 次。VM 102 上禁用再启用网卡后，直连比走节点晚 6.5 秒恢复，与此吻合（待 debug 日志证实）。 |
| macOS | service 传 `--local-dns-servers`（`macdns.rs` 的 `physical_dns_servers` 在启动时读 `State:/Network/Global/DNS`），core 只用列表里第一个不在隧道网段里的地址，渲染成固定 UDP 服务器 | **启动后不再变化**。换 Wi‑Fi 后还在用旧路由器的局域网地址，直连解析一直失败，直到 core 重启（推断，尚未实测）。 |
| Linux | sing-box `local`：systemd-resolved 管理时按默认网卡（link）查询，网卡变化回调里更新；否则读 `/etc/resolv.conf`，按 mtime 最多 5 秒刷新一次 | 本来就是动态的。 |

0.5.4/0.5.5 的背景（见 core-tun-dns-design）：Darwin 上 `dns-local` 曾退回系统解析器，而系统 DNS 被 Desktop 指向了隧道，形成回环；后来改为由 service 传 `--local-dns-servers` 解决。

## 2. 目标

1. 只读**物理默认网卡**的 DNS，也就是 `auto_detect_interface` 绑定的那块网卡，与直连 socket 一致。一律排除 TUN 网卡（utun、Wintun、tun0），也排除隧道网段里的地址（`tunnelPrefixes`，包括 0.5.7 之前的旧网段）和回环地址。
2. 默认网卡变化时**立即作废**并重读。读不到时快速失败，返回明确的错误，**绝不退回 127.0.0.1:53 或系统解析器**。
3. Windows 和 macOS 用同一套缓存和刷新逻辑，只有"怎么读"按平台实现。
4. 永远不经过系统解析器（getaddrinfo、`net.Resolver`），从根本上排除 0.5.4 那种回环。

## 3. 设计

### 3.1 新的 DNS 传输：`ppvpn-local`

新包 `internal/localdns`，在 sing-box 的 DNS 传输注册表里注册类型 `ppvpn-local`（和现在的 dnstransport 注册方式一样）。`internal/config` 渲染 `dns-local` 时：

- Windows、macOS：一律渲染为 `ppvpn-local`；
- 给了 `--local-dns-servers`（任何平台）：渲染为带静态服务器列表的 `ppvpn-local`（见 3.5）；
- Linux 不给时：保持 sing-box `local` 不变（理由见 3.4）。

### 3.2 平台读取（`Discover`）

接口：`Discover(ctx, defaultInterface) (servers []netip.AddrPort, source string, err error)`。默认网卡取自 sing-box 的 `InterfaceMonitor().DefaultInterface()`（名称和 index），与直连绑定用的是同一个来源。

**Windows：** 调 `GetAdaptersAddresses(AF_UNSPEC, GAA_FLAG_INCLUDE_GATEWAYS)`，只看 `IfIndex` 或 `Ipv6IfIndex` 等于默认网卡 index 的那一个适配器，取 `FirstDnsServerAddress` 链表。去掉 fec0::/10（Windows 默认填的站点本地地址，sing-box 也会排除）、隧道网段和回环地址。**不**要求网关：既然被选为默认网卡，就已经有默认路由。纯系统调用，几毫秒，不需要 cgo。

**macOS（CGO_ENABLED=0）：** 几种方式的评估：

| 方式 | 静态 IP / 手动 DNS | 无 DHCP 的网络 | 企业 VPN 的 split DNS | 结论 |
| --- | --- | --- | --- | --- |
| `scutil`：`show State:/Network/Global/DNS`（Desktop 现在用的） | 有（包含手动配置的） | 有 | 不含（那些是另外的 supplemental 解析器） | **主方案**。它是主服务（primary service）的 DNS；Desktop 自己发布的 `State:/Network/Service/com.peakpassvpn.ppvpn.tun/DNS` 是一个独立的 supplemental 服务，不会出现在这里 |
| `scutil --dns` 里的 scoped 解析器（`if_index : N (en0)`） | 有 | 有 | 单列为带 domain 的项，可以区分 | **一致性校验和后备**：当 Global 和默认网卡对不上时，按 if_index 取 |
| `ipconfig getsummary` / `getpacket enX` | **没有**（只有 DHCP 下发的） | 没有 | 没有 | 不采用 |
| sing-box `dhcp` 传输（with_dhcp） | **没有** | 没有 | 没有 | 不采用：会在网络上发 DHCP 报文；拿不到时 sing-box 退回系统解析器，正是 0.5.4 回环的来源 |

macOS 的具体做法：

- 执行 `/usr/sbin/scutil`（绝对路径，2 秒超时），stdin 传入 `show State:/Network/Global/IPv4`、`show State:/Network/Global/DNS`、`quit`。
- 先核对 `PrimaryInterface` 和默认网卡名称是否一致。一致就取 Global/DNS 的 `ServerAddresses`；不一致（比如网络切换中 configd 还没更新完），就解析 `scutil --dns`，取默认网卡 if_index 对应的 scoped 解析器。仍然没有就按"读不到"处理。
- IPv6 地址可能带 zone（`fe80::1%en0`），要保留 zone。
- `scutil` 只在作废后重读时执行，不在每次查询的路径上。

企业 VPN 的 split DNS 不处理：直连域名一律用物理网络的 DNS，和现在一样。和其他 VPN 同时使用本来就不在支持范围内，在文档里写明。

### 3.3 缓存、作废和查询

状态：`{servers, ifIndex, source, readAt, lastAttempt, err}`。

- **作废：** 用 #54 已有的默认网卡变化回调（`Core.defaultInterfaceChanged`）调 `localdns.Invalidate()`。这一次不防抖：作废几乎不花代价，查询时会按需重读。
- **查询时：**
  - 缓存有效、默认网卡没变、servers 非空：直接用；
  - 否则，如果距上次尝试超过 1 秒，就重读一次（singleflight，并发查询只读一次）；
  - 还是空的：立即返回 `ErrNoLocalDNS`（"no DNS servers on <iface>"）。被劫持的查询会立刻得到 SERVFAIL，应用可以马上重试，而不是像现在这样等 5 秒超时。
- **后台软刷新：** 缓存超过 60 秒以后，下一次查询顺带重读一次，用来覆盖 DHCP 续租换了 DNS 但网卡没有变化的情况。
- **交换：** UDP，经 sing-box 的本地 dialer 发出（绑物理网卡，`auto_detect_interface`），每个服务器 2 秒超时，按顺序尝试，第一个应答即返回；应答带 TC 位时改用 TCP。dnstransport 的 debug 日志照常记录，并新增 `upstream=<实际服务器>` 字段。
- **日志：** 每次读到的结果变化时记一条 info：`msg="local dns" interface=en0 servers=… source=scutil-global|scutil-scoped|adapters|override`；读到空列表时记 warn，写明原因。

### 3.4 Linux

不改：sing-box 的 resolved 路径已经按默认网卡查询，并且在网卡变化回调里更新；`resolv.conf` 路径按 mtime 刷新，有 5 秒的上限，但 NetworkManager 或 dhclient 改写文件后很快就会生效。Linux 桌面不是主要平台。如果以后要统一，在 `internal/localdns` 里补一个 Linux 的 `Discover`（resolved 通过 D-Bus 取 link DNS，没有 resolved 时读 resolv.conf）即可，缓存逻辑不用改。

### 3.5 `--local-dns-servers` 的去留

建议：

1. **0.5.20 保留，语义改为显式覆盖：** 给了就只用它，不再动态读，并在启动时记一条 warn：`msg="static local dns servers; they do not follow network changes"`。同时把"只取第一个"改成全部取用（过滤掉隧道网段），按顺序尝试。
2. **Desktop 在 core ≥ 0.5.20 时不再传它**（macOS service 删掉 `physical_dns_servers` 那段）。Desktop 可以从 `get-version` 得到 core 版本，旧 core 继续传，新 core 不传。
3. 参数本身保留，用于测试和特殊主机，不再是推荐用法。

### 3.6 与 0.5.4/0.5.5 回环修复的关系

当时的回环路径是：dns-local → 系统解析器 → 系统 DNS 指向 TUN → core。新设计在结构上排除了这条路径：

- 永远不调用系统解析器，只把 UDP/TCP 直接发给读到的服务器 IP；
- 读到的地址先过滤隧道网段（新旧都过滤）和回环地址。macOS 上 Global/DNS 本来就不含 Desktop 发布的那个服务，过滤是第二道保险；
- socket 绑物理网卡：即使某个服务器地址的路由指向 TUN，查询也不会进入 TUN；
- 读不到时返回错误，没有任何退回系统解析器的分支——这正是 sing-box Darwin `local` 和 Windows `local` 都存在的退路。

## 4. 验证

### 4.1 单元测试（不依赖平台）

- 解析器：用 `scutil` 输出的样本（DHCP、手动静态 DNS、带 zone 的 IPv6、Desktop 那个 tun 服务同时存在、PrimaryInterface 不一致需要走 scoped）；Windows 的假适配器表（fec0::、隧道地址、多块网卡只取默认网卡那块）。
- 缓存逻辑：假的 `Discover` 加手动触发的网卡变化事件，覆盖：作废后立即重读；空列表时 1 秒内不重复读，并返回 `ErrNoLocalDNS`；绝不出现 127.0.0.1:53；60 秒软刷新；并发查询只读一次（singleflight）。

### 4.2 Linux 端到端（Linux 测试机上的特权容器）

缓存和作废是共通逻辑，要在 Linux 上验证，测试构建需要一个可替换的 `Discover`：在 `internal/localdns` 里加一个只用于测试的"文件来源"，由环境变量 `PPVPN_LOCALDNS_TEST_FILE` 打开，按网卡名读一个 JSON。生产构建里这个变量不起作用，或者用 build tag 隔离。场景：

1. 客户端容器有两块网卡：`eth0` 接网 A，`eth1` 接网 B。网 A 的 DNS 只在网 A 可达，把 `x.lab.test` 答成 A；网 B 的 DNS 同理答成 B。
2. 一开始默认路由走 eth0，直连解析 `x.lab.test` 得到 A。
3. 把默认路由切到 eth1（`ip route replace default dev eth1`），同时改写 test file，模拟换 Wi‑Fi。sing-tun 的网卡监视器触发 `event=changed`，dns-local 作废。
4. 断言：切换后第一次查询就走网 B 的 DNS，得到 B；没有任何查询发往旧 DNS 或 127.0.0.1（用 tcpdump 计数）；恢复时间从 changed 算起小于 1 秒。
5. 再测"新网卡还没有 DNS"：test file 先给空列表，查询应在 1 秒内返回 SERVFAIL（不能等 5 秒超时）；然后补上服务器，应在 1 秒内恢复。

### 4.3 平台实测

- Windows：VM 102，禁用再启用网卡，再加一次在两块网卡之间切换；看 `msg="local dns"` 和直连恢复时间。目标：恢复时间与走节点相当，6.5 秒的差距消失。
- macOS：只有用户自己的 Mac，需要征得用户同意。Wi‑Fi 之间切换一次，再切到有线一次，看 `source=` 和直连解析是否立即跟随。

## 5. 改动范围（实现估计）

- 新包 `internal/localdns`：传输、缓存、Windows 和 Darwin 的 `Discover`、测试用的文件来源；
- `internal/config/tundns.go`：Windows/Darwin 渲染 `ppvpn-local`，`--local-dns-servers` 改为覆盖语义；
- `internal/runtime`：在 `defaultInterfaceChanged` 里调用 `localdns.Invalidate()`；
- `internal/dnstransport`：debug 日志加 `upstream`；
- 文档：security.md 的 DNS 一节，以及 quickstart 的日志说明；
- Desktop（另行安排）：core ≥ 0.5.20 时不再传 `--local-dns-servers`。

## 7. 实现说明（与设计稿的差异和补充）

- **作废的挂载点：** 传输在 `Start` 时自己向 sing-box 的 `InterfaceMonitor` 注册回调（与 sing-box 的 resolved 传输相同），`Close` 时注销。这和 #54 的 `Core.defaultInterfaceChanged` 是同一个事件源，不需要经过 `Core` 转发，也覆盖热切换出来的每个内核。另外，`Reset()` 也会作废缓存；默认网卡的 index 与上次读取时不同，也会触发重读（不依赖回调）。
- **作废不加锁：** 回调只给一个原子计数器加一，不等待正在进行的读取，因此不会阻塞网卡监视器。
- **失败驱动的重读：** 所有服务器都失败时（ctx 被取消不算），把缓存标记为可疑，下一次查询重读（同样每秒最多一次）。这覆盖了"换了网络，但 IP 地址和网卡都没变"的少见情况；另有 60 秒的软刷新兜底。软刷新时如果读取本身出错（例如 scutil 超时），继续使用原来的服务器。
- **macOS 判定"属于默认网卡"：** 优先用 `State:/Network/Global/DNS` 自带的 `__IF_INDEX__`，与默认网卡的 index 比较；没有这个字段时，才比较 `Global/IPv4`（或 IPv6-only 时 `Global/IPv6`）的 `PrimaryInterface`。不属于默认网卡时，改用 `scutil --dns` 里该网卡的 scoped 解析器（带 `domain` 的项是 split DNS，跳过）。实测：在一台同时运行着另一个 VPN 的 Mac 上，主服务是那个 VPN 的 utun，Global/DNS 是它的 DNS（`__IF_INDEX__ : 29`）。按 Desktop 原来 `physical_dns_servers` 的读法会拿到那个 VPN 的 DNS；新实现拒绝了它，并从 scoped 解析器取到了 en0 的 DNS。
- **带 zone 的链路本地地址：** 选择**使用**，前提是它属于默认网卡。zone 是默认网卡的名称或 index 时保留；没有 zone 时补上默认网卡的 index；zone 指向其他网卡的一律丢弃（socket 绑在默认网卡上，到不了它）。Windows 读到的链路本地地址用适配器的 `Ipv6IfIndex` 作为 zone。`--local-dns-servers` 给的地址按原样使用，不做这项过滤。
- **过滤：** 隧道网段（`10.60.159.88/30`、`fde2:ec40:9312:c7fd::/126`，以及旧的 `172.19.0.0/30`、`fdfe:dcba:9876::/126`，由 config 通过 `exclude` 传入）、回环、未指定地址、组播、`fec0::/10`，并去掉重复项；`::ffff:` 映射地址还原成 IPv4。
- **`--local-dns-servers`：** 在所有平台上都渲染为 `ppvpn-local` 的静态列表，按顺序使用全部隧道网段外的地址。启动时记 warn：`msg="static local dns servers; they do not follow network changes"`。如果全部在隧道网段内，记 warn，并改为动态读取（Windows/macOS），或 sing-box `local`（Linux）。
- **日志：** 服务器列表每次变化时记 `msg="local dns servers" source=… interface=… servers=…`（info）。读不到时为 warn，`servers=none`，并附 `error`。`source` 的取值：`adapter`（Windows）、`scutil-global`、`scutil-scoped`（macOS）、`override`。`msg=dns` 的 debug 行新增 `upstream`，记录实际应答的服务器。
- **没有网卡监视器时：** `auto_detect_interface` 关闭时（TUN 构建里不会出现），`Start` 只记 warn，不让内核启动失败；查询会立即返回 `ErrNoInterface`。
- **测试来源（按评审要求改用 build tag）：** 只有带 `localdns_testsource` tag 的构建才含这个来源（`internal/localdns/testsource.go`）。它从 `$PPVPN_LOCALDNS_TEST_FILE` 读取 `{"<网卡名>": ["<服务器>", …]}`，并在 Linux 上也让 config 渲染 `ppvpn-local`。正式构建不编译这个文件，环境变量也就不起作用。实验构建用 `make build-lab-linux`。来源里有标记字符串 `ppvpn-localdns-testsource-enabled`（也作为日志里的 `source`）。检查分两处：
  - CI：用正式 tag 和实验 tag 各构建一次。实验构建必须含这个标记，否则检查本身失效；正式构建必须不含。
  - release.yml：在 Assemble 步骤里检查每个发行文件，既不能含这个标记，`go version -m` 里也不能有这个 tag。
- **4.2 的实现：** 用的是 `test/lab/localdns/run.sh`，加上辅助程序 `test/lab/localdns/ldnslab`。它不用容器，而是建三个独立的网络命名空间（客户端、网 A、网 B），既不依赖镜像，也不碰宿主网络；`CPUS=12-15` 时用 taskset 固定到这几个核上。覆盖的情况：
  - 切换到另一块网卡后的第一次查询；
  - 同一网卡上换网络（换地址、换 DNS）；
  - 没有 DNS 时快速失败（每次 <500 ms），DNS 出现后 1.5 秒内恢复；
  - 127.0.0.1:53 上放了一个陷阱解析器，始终不应被查询；
  - 每次列表变化都有一行 `local dns servers`。
- **失败即 SERVFAIL：** 读不到服务器，或所有服务器都失败时，传输返回 SERVFAIL 应答，而不是返回错误。sing-box 对出错的被劫持查询不回任何应答（UDP）或直接关闭连接（TCP），客户端只能等到自己超时。这和 dnstransport 守卫在 0.5.11 的做法相同。原因记在 debug 日志 `msg="local dns failed"` 里；SERVFAIL 不会被缓存。实验中第一版返回的是错误，客户端因此等满了 3 秒超时，是端到端测试发现的。
- **网卡变化的 1 秒防抖：** sing-tun 在所有平台上都把默认网卡检查推迟 1 秒（`monitor_shared.go` 的 `delayCheckUpdate`）。所以 `event=changed` 比路由实际变化晚约 1 秒，Windows VM 102 上的 +1.2 秒就是这个原因。直连 socket 的网卡绑定受同一个延迟影响，dns-local 跟随同一个事件。
- **端到端结果（Linux 测试机，2026-10-02，netshoot 特权容器，连跑 3 次都通过）：**
  - 三种网络变化在 `changed` 之后的第一次查询就由新 DNS 应答（1–2 ms），旧 DNS 不再收到查询；
  - 没有服务器时 SERVFAIL 在 0–1 ms 内返回；
  - 服务器出现但网卡没有变化时，约 1.04 秒恢复（符合每秒最多读一次的限制）；
  - 127.0.0.1 陷阱始终未被查询。
  - 变异检查：把作废改成空操作后，"同一网卡换网络"和"没有服务器"两组断言失败。
- **依赖：socket 必须绑在物理网卡上。** 传输用 `dns.NewLocalDialer` 加空的 `DialerOptions` 拨号，也就是 sing-box 的默认 dialer。它在 `route.auto_detect_interface` 打开时把 socket 绑到默认网卡（Linux、macOS、Windows 都由 sing-box 的 interface finder 和 bind 实现）。core 的 TUN 构建总是打开 `auto_detect_interface`。`auto_route` 会把其余所有流量引进 TUN，所以一旦没绑上，发往物理网卡 DNS 的查询就会进隧道、被劫持回 core，形成回环。这个依赖由端到端测试守住：
  - 整个测试过程都在 TUN（`tun0`）上抓 53 端口；
  - 断言：发往物理 DNS 的包一个都不出现在 TUN 上；各个 DNS 服务器看到的源地址全部是物理网卡的地址；
  - 发往 TUN 自身 DNS（10.60.159.90）的查询作为正对照，证明抓包确实生效。
  - 变异检查：把 dialer 换成不绑网卡的 `N.SystemDialer` 后，565 个发往物理 DNS 的包进了 `tun0`，每次查询都在 2 秒后变成 SERVFAIL，这条断言失败。也就是说，这个回环真实存在，而测试能发现它。
  - 未来如果改动这个 dialer（例如加 `bind_interface` 或 detour），这条测试必须继续通过。
