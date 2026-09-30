# 五分钟快速开始

本指南用于本地验证和桌面端接入。需要 Go 1.25 或 `go.mod` 指定的兼容工具链。

## 1. 构建与自检

```sh
go test ./...
go build -trimpath -o build/ppvpn-core ./cmd/ppvpn-core
./build/ppvpn-core version
```

预期版本响应：

```json
{"core_version":"0.5.6","core_api_version":1,"profile_schema_version":1,"flow_adapter_version":1,"local_proxy_contract_version":1}
```

## 2. 准备 Profile

参考 [Backend Profile](backend-profile.md) 生成 `profile.json`，将所有演示地址和凭据换成真实服务值，然后先做离线校验：

```sh
./build/ppvpn-core validate profile.json
./build/ppvpn-core probe-entrance --method tcp --timeout 5s --concurrency 4 profile.json
./build/ppvpn-core probe-entrance --method icmp profile.json
```

`validate` 成功输出 `profile valid`。入口探测只验证公网字面量 IP 的 TCP 可达性，不等价于节点协议可用。

需要排查配置转换时可使用：

```sh
./build/ppvpn-core render --platform macos profile.json
```

输出已经过脱敏，但仍只应在本机受控环境查看；该命令不是后端或产品客户端的配置生成接口。

## 3. 启动桌面核心

创建一个仅当前用户可访问的应用状态目录。以下路径仅为 macOS/Linux 开发示例：

```sh
APP_STATE=/private/tmp/ppvpn-core-demo
mkdir -m 700 "$APP_STATE"

./build/ppvpn-core serve \
  --socket "$APP_STATE/core.sock" \
  --session-secret-file "$APP_STATE/session.secret" \
  --state-dir "$APP_STATE/state" \
  --platform macos
```

`serve` 默认启用共享认证本地代理：所有节点共用一个 loopback 端口（优先 7890），用户名
`<prefix>-<node_id>` 选择节点，密码为设备 secret。`--local-proxy=false` 可关闭它；核心不提供
无认证的系统代理兼容入口。桌面端的推荐组合是：登录期间常驻一个非特权核心
`--tun=false --local-proxy=true`（共享本地代理端口，`probe-availability` 通过它工作），
增强模式另起一个特权核心 `--tun --local-proxy=false`（本地代理相关 API 返回
`LOCAL_PROXY_DISABLED`）。TUN 核心会把所有入口 IP 加入 `route_exclude_address`，
因此非特权核心到入口的连接和探测不会进入隧道。
方式。

生产环境不要使用共享临时目录。macOS 应使用 App Container/Application Support 私有目录；Windows 应使用带当前用户 ACL 的 LocalAppData 目录和 Named Pipe 路径。

`serve` 把第一方诊断日志写到 stderr（每行一次写入并刷盘），或用 `--log-file <path>` 追加到文件；
启动时记录版本、平台、`tun`/`local_proxy` 参数和状态目录，生命周期请求（apply/start/stop/reload）的成功与所有失败原因都会记录。

`--local-dns-servers <list>`（仅与 `--tun` 一起使用）：逗号分隔的物理网络 DNS 服务器（IP、IP:port 或
`[IPv6%zone]:port`，默认端口 53），应在宿主把系统 DNS 指向隧道之前读取。核心取其中第一个不在隧道地址段
（`172.19.0.0/30`、`fdfe:dcba:9876::/126`）内的地址，作为 `dns-local` 的 UDP 上游（绑定物理网卡），用来
解析路由为直连的域名。非法项会让 `serve` 启动失败。不传或全部被过滤时，`dns-local` 使用 sing-box 的 local
解析器：macOS 在有 TUN 时查询 DHCP 下发的服务器，否则退回系统解析器，而桌面端已把它指向隧道，会形成回环，
因此 macOS 宿主应当传入。列表在启动时固定，运行中切换网络需要重连。

`--log-level info|debug`（默认 `info`，由宿主 service 传入）：

- `info`：另外每次 apply 与 start 各记一行分段耗时（毫秒），用于定位慢启动：
  - `msg="apply timing"`：`validate_ms`、`rule_sets_ms`（规则集校验/下载）、`local_proxy_ms`、
    `host_ipv6_ms`（主机 IPv6 探测）、`build_ms`、`routing_ms`，运行中 apply 还有
    `engine_create_ms`/`engine_start_ms`；
  - `msg="start timing"`：`system_proxy_ms`（启用时）、`engine_create_ms`（sing-box 解析与构造）、
    `engine_start_ms`（sing-box 启动：出站、DNS、路由与规则集、入站，含打开 TUN 与安装 `auto_route`
    路由）、`total_ms`。sing-box 内部各组件不再细分：其计时只在上游日志里，而上游日志保持关闭。
- `debug`：再为每条被路由的连接记一行 `msg=connection`：`inbound`、`network`、`destination`、
  `route_domain`（路由规则匹配用的域名：嗅探所得或 DNS 反查；HTTP 嗅探可能留下地址本身）、`protocol`、
  `rule`、`outbound`（实际节点）、`target`（交给节点的目标）与 `target_kind`（`domain`/`ip`）。
  被 reject 或 hijack-dns 的连接不经过此处。
  另外每次发往上游 DNS 服务器的查询记一行 `msg=dns`：`name`、`type`、`server`（`dns-local`/`dns-remote`）、
  `rcode` 与 `answers`，或 `error`，以及 `ms`。命中 DNS 缓存的查询不会发往上游，因此不记录。**debug 日志包含用户访问的域名，只能在排查时临时开启，
  不得常开或默认开启。**
`serve` 每次启动覆盖生成新的会话密钥，正常退出时删除密钥文件。产品桌面端还应启用 `--exit-on-stdin-close`，并保持传入核心的 stdin 写端存活，使父 App 崩溃后核心自动退出。

## 4. 调用 API

在另一个终端：

```sh
APP_STATE=/private/tmp/ppvpn-core-demo
SECRET=$(cat "$APP_STATE/session.secret")

curl --unix-socket "$APP_STATE/core.sock" \
  -H "Authorization: Bearer $SECRET" \
  -H 'X-Core-API-Version: 1' \
  -H 'X-Request-ID: quickstart-1' \
  -H 'Content-Type: application/json' \
  --data '{}' \
  http://localhost/v1/get-version
```

应用 Profile 时，不要用 shell 拼接含密钥的命令。产品代码应直接在内存中编码请求并写入 IPC。仅本机开发可用下列 `jq` 示例：

```sh
jq -n --slurpfile profile profile.json '{profile:$profile[0]}' > /private/tmp/apply-request.json

curl --unix-socket "$APP_STATE/core.sock" \
  -H "Authorization: Bearer $SECRET" \
  -H 'X-Core-API-Version: 1' \
  -H 'Content-Type: application/json' \
  --data-binary @/private/tmp/apply-request.json \
  http://localhost/v1/apply-profile
```

随后调用 `/v1/start`，并用 `/v1/get-local-proxy-metadata` 读取不含 secret 的每节点
HTTP/SOCKS5 端点（所有节点同一端口）；只有原生凭据面板按需调用 `/v1/get-local-proxy-credential`。Windows
产品由特权 service 以 TUN 模式运行 core，macOS 产品使用 XCFramework 和原生 Network
Extension；两者都不写系统 HTTP/SOCKS 设置。节点切换只调用 `/v1/select-node`，退出时
调用 `/v1/stop`。完整顺序和 DTO 见 [Core API v1](core-api.md)。

## 5. 常见问题

- `SCHEMA_UNSUPPORTED`：核心和后端的 Profile Schema 不兼容，先停止应用配置。
- `ENTRY_IP_NOT_PUBLIC`：入口 `endpoint.ip` 不是可拨号公网单播 IP；不要填域名或文档地址（不知道 IP 时可省略该字段）。
- `TLS_SERVER_NAME_MISMATCH`：AnyTLS 的 TLS SNI 必须等于 `endpoint.domain`（REALITY 的 SNI 是借用站点，不受此限）。
- `CORE_OPERATION_FAILED`：上游错误已安全折叠。核心日志（默认 stderr，或 `--log-file`）中有一行 `level=error msg=CORE_OPERATION_FAILED`，包含 `path`、`request_id`、`stage`、`error` 和 `chain`（错误链类型，OS 错误附带数值，如 `syscall.Errno(5)`）。
- TUN 启动失败并提示 `gVisor is not included`：核心未带 `with_gvisor` 构建，而 `mixed`/`gvisor` 栈需要它；使用 Makefile 的桌面目标构建（`serve` 会在启动时直接拒绝这种组合）。
- 本地代理端口变更：核心启动前发现持久端口（或 7890）已占用时改用空闲端口并持久化；`start` 后调用 `GetLocalProxyMetadata` 刷新。
- TUN/Network Extension 启动失败：保持未连接并由原生宿主通知用户；不要静默回退到系统代理。
