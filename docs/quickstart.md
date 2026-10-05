# 五分钟快速开始

本指南用于本地验证和宿主接入。引擎是 Rust 库 `ppvpn-core`（`crates/ppvpn-core`，内嵌 Sail），没有单独发行的核心进程：Desktop 和 CLI 在自己的进程里链接它。接口契约见 [宿主接入](host-integration.md)，测试见 [测试分层](testing.md)。

本机试用有三条路：

- 在自己的 Rust 代码里直接调用 `Engine`（第 3 节）；
- `ppvpn` CLI 的 dev 构建，读本地 Profile 文件运行标准实例（第 4 节）；
- 测试宿主 `ppvpn-core-lab`，用 Core API v1 的形状对外提供引擎（第 5 节）。它只给 lab 和 CI 用，不是产品。

工具链：不低于 `Cargo.toml` 里的 `rust-version`。

## 1. 构建与自检

```sh
cargo build --locked -p ppvpn-core-lab
./target/debug/ppvpn-core-lab version
```

`version` 输出 `Engine::version()`（`core_version` 是 crate 版本，`sail_version`、`sail_commit` 是链接的 Sail）：

```json
{"core_version":"X.Y.Z","sail_version":"…","sail_commit":"…","profile_schema_version":1,"local_proxy_contract_version":1}
```

测试入口是 `make test-unit`、`make test-integration` 和 `make test-system`（Linux，需要 sudo），见 [测试分层](testing.md)。

## 2. 准备 Profile

参考 [Backend Profile](backend-profile.md) 生成 `profile.json`，把所有演示地址和凭据换成真实服务的值。

校验不需要实例，也不联网：在 Rust 里是 `Engine::validate(&ApplyRequest)`，经 `ppvpn-core-lab` 是 `/v1/validate-profile`（第 5 节）。失败时返回稳定的错误码和字段路径，例如 `ENTRY_IP_NOT_PUBLIC`、field=`nodes[0].ingresses[0].endpoint.ip`。

Profile 只放在内存里：引擎的 `state_dir` 不保存 Profile 原文，宿主也不必落盘。

## 3. 在 Rust 宿主里使用

```rust
use ppvpn_core::{
    ApplyRequest, Engine, EngineConfig, LocalProxyConfig, LogConfig, LogLevel, LogSink, Platform,
    Role,
};

async fn run(state_dir: &str, log_path: &str, profile: Vec<u8>) -> Result<(), ppvpn_core::Error> {
    let config = EngineConfig::new(Role::Standard, Platform::Linux, state_dir)
        .with_local_proxy(LocalProxyConfig::new())
        .with_log(LogConfig::new(
            LogLevel::Info,
            LogSink::File { path: log_path.into() },
        ));
    let engine = Engine::new(config).await?;
    let request = ApplyRequest::new(profile)
        .with_allowed_rule_set_hosts(vec!["api.example.com".into()]);
    engine.apply(request).await?;
    engine.start().await?;
    let endpoints = engine.local_proxy_metadata()?; // 不含密码
    // …
    engine.shutdown().await?;
    Ok(())
}
```

- `Role::Standard` 不需要特权：共享本地代理、探测、流量与连接统计。`Role::Tun` 需要 root（Linux 也可以是 `CAP_NET_ADMIN` 加 `CAP_NET_RAW`）或 Windows 的 SYSTEM，一个进程里最多一个。
- `state_dir` 是私有目录，存规则集缓存和本地代理状态（prefix、密码、端口），被实例独占加锁。
- `allowed_rule_set_hosts` 为空时一个规则集也不下载，只用本地已有、sha256 相符的缓存。宿主传拉取 Profile 的 API 主机。
- 选中节点、ingress pin 和 `routing_mode` 由宿主持久化，每次 apply 一起传入（`with_selected_node_id`、`with_pins`、`with_routing_mode`）。
- 宿主如果装了自己的 tracing 订阅者，要加上 `ppvpn_core::tracing_layer()`，否则日志行进不了实例的 sink。

完整的生命周期、状态、事件和错误码见 [宿主接入](host-integration.md)。

## 4. 用 CLI 运行标准实例

`ppvpn` CLI 在自己的 daemon 里运行一个标准实例：只有共享本地代理，不建 TUN，不改系统网络设置。dev 构建可以不登录、直接读本地 Profile：

```sh
PPVPN_BUILD_PROFILE=dev cargo build --locked -p ppvpn-cli
export PPVPN_API_BASE=https://api.example.com    # 规则集只从这个主机下载
export PPVPN_PROFILE_FILE="$PWD/profile.json"    # 必须是绝对路径；release 构建忽略它
./target/debug/ppvpn start --foreground
```

在另一个终端用 `ppvpn status`、`ppvpn nodes`、`ppvpn proxy` 查看状态和本地代理端点，`ppvpn proxy credential [node-id]` 读凭据。CLI 把核心日志写到运行目录的 `core.log`（info 级别）。命令、退出码和文件位置见 [CLI](cli.md)。

## 5. 用 ppvpn-core-lab 调用 Core API v1

`ppvpn-core-lab serve` 的参数和日志格式与原来的 `ppvpn-core serve` 相同，lab 和 netns CI 的脚本直接用它。创建一个仅当前用户可访问的目录（下面的路径只是 macOS/Linux 开发示例）：

```sh
APP_STATE="${TMPDIR:-/tmp}/ppvpn-core-demo"
mkdir -m 700 "$APP_STATE"

./target/debug/ppvpn-core-lab serve \
  --socket "$APP_STATE/core.sock" \
  --session-secret-file "$APP_STATE/session.secret" \
  --state-dir "$APP_STATE/state"
```

- 默认是标准实例，带共享本地代理（`--local-proxy=false` 关闭）。所有节点共用一个 loopback 端口（优先 7890），用户名 `<prefix>-<node_id>` 选择节点，裸 `<prefix>` 按规则路由。
- `--tun` 改为 TUN 实例，需要特权；这时本地代理相关的接口返回 `LOCAL_PROXY_DISABLED`，建议同时传 `--local-proxy=false`。
- `--local-dns-servers <list>` 只能和 `--tun` 一起用：逗号分隔的物理网络 DNS 服务器，作为 dns-local 的静态覆盖，不跟随网络变化。不传时 dns-local 自己读默认网卡的 DNS（Windows 读适配器，macOS 读 scutil，Linux 读 resolv.conf 或 systemd-resolved），网卡变化后重读。
- `--log-file <path>` 追加到文件，默认写 stderr；`--log-level info|debug`，默认 `info`（见第 6 节）。
- `--exit-on-stdin-close`：父进程持有的 stdin 关闭时退出。
- `--platform` 和 `--tun-stack` 只为兼容旧脚本而接受，不起作用：平台取自构建目标。
- 只有 Unix socket；Windows 的 Named Pipe 还没有实现。

`serve` 每次启动生成新的会话密钥，退出时删除密钥文件，并在 10 秒内关闭实例。在另一个终端：

```sh
APP_STATE="${TMPDIR:-/tmp}/ppvpn-core-demo"
SECRET=$(cat "$APP_STATE/session.secret")

curl --unix-socket "$APP_STATE/core.sock" \
  -H "Authorization: Bearer $SECRET" \
  -H 'X-Core-API-Version: 1' \
  -H 'X-Request-ID: quickstart-1' \
  -H 'Content-Type: application/json' \
  --data '{}' \
  http://localhost/v1/get-version
```

应用 Profile 时，不要用 shell 拼接含密钥的命令。仅本机开发可用下列 `jq` 示例：

```sh
jq -n --slurpfile profile profile.json \
  '{profile:$profile[0], allowed_rule_set_hosts:["api.example.com"]}' > "$APP_STATE/apply-request.json"

curl --unix-socket "$APP_STATE/core.sock" \
  -H "Authorization: Bearer $SECRET" \
  -H 'X-Core-API-Version: 1' \
  -H 'Content-Type: application/json' \
  --data-binary @"$APP_STATE/apply-request.json" \
  http://localhost/v1/apply-profile
```

同样的请求体发到 `/v1/validate-profile` 只做校验。随后调用 `/v1/start`，用 `/v1/get-status` 看状态，用 `/v1/get-local-proxy-metadata` 读不含 secret 的端点，`/v1/get-local-proxy-credential` 读凭据（`kind` 为 `node` 或 `routed`）。节点切换用 `/v1/select-node`，结束用 `/v1/stop`。`ppvpn-core-lab` 只实现 lab 用到的那部分 Core API v1，DTO 见 [Core API v1](core-api.md)。

## 6. 日志

每一行的格式是 `<RFC 3339 UTC 时间，纳秒> level=<error|warn|info|debug> msg=<消息> key=value …`。引擎的行末尾带 `source=core`（`ppvpn-core` 自己的行）或 `source=sail`（Sail 的行，原文整个放在 `msg` 里）。任何级别都不记录凭据。

**info（默认）**：日志里不出现连接的目的地址或域名。Sail 在这个级别只记 warn 及以上，因为它的 info 会给每条连接写一行目的地。常见的 info 行：

- `msg="default interface"`：每次 start 和默认网卡每次变化各一行。`event`（`start`/`changed`）、`name`、`index`、`addresses`；`name=none` 表示没有可用网卡（离线）。没有 `mtu`：Sail 的网络快照不带它。
- `msg="host ipv6"`（TUN 实例，桌面平台）：每次 apply 和 start 探测主机 IPv6，记一行 `host_ipv6_enabled`、`host_ipv6_route`（是否有全局单播 IPv6 地址加 IPv6 默认路由）、`policy`。`policy` 取值：`tun_ipv6`；`tun_ipv6_direct_ipv4`，表示没有 IPv6 出口，直连的 IPv6 目标改按域名走 IPv4；`tun_ipv4_only`，表示主机关闭了 IPv6。路由读不出来时附 `error`，按有出口处理。
  - 默认网卡最后一次变化 2 秒后再探测一次（离线时跳过）。出口的有无变了，就生成新配置让 Sail 原地 reload，记一行 `msg="host ipv6 changed"`：`previous_policy`、`policy`、`rebuilt=true`；失败时是 error 级的同一行，`rebuilt=false` 并附 `error`。详见 [security.md](security.md#ipv6)。
- `msg="local dns servers"`（TUN 实例）：dns-local 读到的服务器和上次不同时记一行。`interface`、`source`（`adapter`：Windows；`scutil-global`/`scutil-scoped`：macOS；`resolv.conf`/`resolved`：Linux；`override`：`--local-dns-servers` 或 `TunConfig.local_dns_servers`）、`servers`；读不到时 `servers=none` 并附 `error`。详见 [安全模型](security.md#增强模式tun的-dns-与防泄漏) 与 [设计](design-local-dns.md)。
- `msg="kernel switched"`：运行中的 apply、规则集重建或 IPv6 重探让 Sail 原地 reload 时记一行。`gen` 和 `previous` 是引擎对配置代数的计数；`closed_connections`、`kept_connections` 目前恒为 0（`docs/rust-parity.md` 的 A 类项）。原地 reload 不关监听，已有连接留在原处。
- `msg="full restart" reasons=…`：Sail 不能原地接受的变化（例如 `tun options changed`）改为停止再启动，所有连接断开。
- `msg="rule set"`：规则集状态变化。`id`、`state`（`ready`/`stale`/`unavailable`）、`error`、`failures`、`next_retry_at`。
- `msg="apply timing"`：每次 apply 一行，用于定位慢启动。`outcome`（`ok`/`failed`）、`tun`、`rule_sets_ready`/`rule_sets_stale`/`rule_sets_unavailable`，然后各分段的毫秒数，没跑到的分段不写：`validate_ms`、`rule_sets_ms`（规则集校验/下载）、`wait_ms`（等另一个生命周期调用释放操作锁）、`host_ipv6_ms`（主机 IPv6 探测）、`build_ms`（翻译），停止时 `check_ms`（配置交给 Sail 校验），运行中 `kernel_switch_ms`（reload 与恢复选择/固定）或 `full_restart_ms`，最后 `total_ms`。内容未变的 apply 不记。
- `msg="start timing"`：每次 start 一行。`outcome`、`tun`、`local_proxy_ms`（检查监听端口）、`host_ipv6_ms`、`build_ms`、`engine_start_ms`（Sail 解析、构造与启动：出站、DNS、路由与规则集、入站，含打开 TUN 与安装路由，不再细分）、`total_ms`。已在运行时 start 不记。
- `msg="log lines dropped" dropped=<行数>`（warn）：日志接收端阻塞时丢了行，恢复后补这一行。

`ppvpn-core-lab` 自己另记 `serve starting`（`core_version`、`sail_commit`、`os`、`arch`、`log_level`、`platform`、`tun`、`tun_stack`、`local_proxy`、`state_dir`、`socket`）、`serve ready`、`serve stopping`，以及生命周期请求（apply/start/stop/set-system-proxy/pin-ingress）的 `request ok` 和所有请求的 `request rejected`（`path`、`request_id`、`code`，有字段时带 `field`）。

**debug**：Sail 也记 debug。引擎另外记下面几种含目的地的行。**debug 日志包含用户访问的域名，只能在排查时临时开启，不得常开或默认开启。**

- `msg=connection`：每条被路由的连接一行。`id`、`inbound`、`network`、`destination`、`route_domain`（路由规则匹配用的域名）、`protocol`、`rule`（Profile 规则的 id，没有命中规则时为 `final`）、`outbound`（规则选中的出站）、`target`（交给出站的目标）、`target_kind`（`domain`/`ip`）、`action`、`error`。
- `msg="outbound failed"`：每次拨号失败一行。`stage`、`outbound`、`destination`、`error`；经节点时另有 `node_id`、`endpoint_key`、`count`、`more_to_try`（多入口节点的每个入口失败各一行，只有这条连接最后一次失败为 false）。直连出站同一目标 10 秒内只记第一次，窗口过后的下一行附 `suppressed`，即期间省略的次数。
- `msg=dns`：发往上游 DNS 服务器的每次查询一行，命中缓存的不记。`name`、`type`、`server`（`dns-local` 或 `dns-remote`）、`upstream`（实际应答的服务器）、`attempt`（`dns-remote` 的第几次尝试）、`rcode` 与 `answers`，或 `error`，以及 `ms`。
- `msg="ingress tls"`：每次 apply 为每个带 TLS 的入口记一行，用于和服务端核对参数而不暴露原文：`node_id`、`endpoint_key`、`protocol`、`server_name`、`flow`；REALITY 入口另有 `public_key_sha256`、`short_id_sha256`（按收到的字符串原样取 SHA-256 的前 10 个十六进制字符）、两者的长度、`public_key_encoding` 和 `fingerprint`；其他 TLS 入口另有 `insecure`。

`dns-remote`（TUN 实例）经所选节点用 DoT 查询，按顺序试 `1.1.1.1`、`8.8.8.8`、`9.9.9.9`：每次尝试最多 3 秒，总计不超过 8 秒，三个都失败时回 SERVFAIL；某个上游应答后，之后 10 分钟内的查询先问它。

## 7. 常见问题

- `SCHEMA_UNSUPPORTED`：核心和后端的 Profile Schema 不兼容，先停止应用配置。
- `ENTRY_IP_NOT_PUBLIC`：入口 `endpoint.ip` 不是可拨号的公网单播 IP；不要填域名或文档地址（不知道 IP 时可省略该字段）。
- `TLS_SERVER_NAME_MISMATCH`：AnyTLS 的 TLS SNI 必须等于 `endpoint.domain`（REALITY 的 SNI 是借用站点，不受此限）。
- `PROFILE_NOT_APPLIED`：`start`、`select_node` 之前要先 apply。
- `STATE_DIR_IN_USE`：另一个实例正在使用同一个 `state_dir`。
- `PERMISSION_DENIED`：TUN 实例的权限不足。
- `TUN_NAME_TAKEN`：TUN 网卡名被占用；Linux（`ppvpn0`）和 Windows（`PPVPN`）上多半是另一个 ppvpn-core 正在运行。
- 规则集状态带 `error=RULE_SET_HOST_NOT_PINNED`：没有传 `allowed_rule_set_hosts`，引擎不下载，只能用已有的缓存。列表不为空、而 Profile 的规则集 URL 不在这些主机上时，apply 直接被拒。
- `CORE_OPERATION_FAILED`：内部错误，原因只写进日志。`ppvpn-core-lab` 的日志里有一行 `level=error msg=CORE_OPERATION_FAILED`，带 `path`、`request_id` 和 `error`。
- 本地代理端口：启动时持久化的端口（或 7890）被占用，就换一个空闲端口并持久化，发出 `LocalProxyEndpointChanged`；之后重新读 metadata。监听打不开时实例照常运行，状态为 `Degraded{LocalProxyUnavailable}`，并按退避重试。
- TUN 启动失败：保持未连接并由宿主通知用户；不要静默回退到系统代理。
