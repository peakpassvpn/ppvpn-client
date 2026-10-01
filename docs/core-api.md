# Core API v1

Core API 是桌面宿主与核心之间唯一稳定控制协议。它使用本机 HTTP 语义，但不监听 TCP：macOS/Linux 使用权限为 `0600` 的 Unix Domain Socket，Windows 使用仅当前用户可访问的 Named Pipe。

## 认证、版本和限制

每个请求都应携带：

```http
Authorization: Bearer <session-secret>
X-Core-API-Version: 1
X-Request-ID: <optional-client-id>
```

- 会话密钥由 `serve` 每次启动随机生成并写入 `--session-secret-file`，至少 32 字符；不得放在命令行、日志或持久设置中。
- API 版本头若缺省按当前 v1 处理；产品客户端应始终发送，以便在不兼容时得到 `CORE_API_UNSUPPORTED`。
- `X-Request-ID` 可选且最长 128 字符；缺省时核心生成 24 位十六进制 ID。
- 请求体上限 4 MiB，严格拒绝未知字段和一个 JSON 值之后的尾随内容。
- 除事件流外均为 `POST`，请求体至少发送 `{}`。

## 通用封装

成功：

```json
{"request_id":"desktop-42","ok":true,"data":{},"error":null}
```

失败：

```json
{
  "request_id": "desktop-42",
  "ok": false,
  "error": {
    "code": "PROFILE_EXPIRED",
    "message": "profile has expired",
    "field": "expires_at",
    "retryable": false
  }
}
```

调用方必须以 `ok` 和 `error.code` 分支，不要解析英文 `message`。认证失败使用 HTTP 401，未知路径使用 HTTP 404，其余当前业务/校验错误使用 HTTP 400；不要只凭 HTTP 状态判断具体业务原因。

## 调用顺序

典型启动流程：

1. `GetVersion` 检查 Core API 和 Profile Schema 兼容性。
2. `ValidateProfile` 可用于预检；`ApplyProfile` 本身也会完整校验。
3. `ApplyProfile` 成功后读取 `applied`；同 revision 返回 false。
4. `Start`，然后用 `GetStatus` 确认 `state=running`。
5. 建立 `WatchEvents`；断线重连后重新调用 `GetStatus`。
6. 退出时调用 `Stop`，关闭 IPC；父进程还应关闭传给核心的 stdin 存活管道。

## 方法

| 方法 | 路径 | 请求 `data`/请求体 | 成功响应 `data` |
| --- | --- | --- | --- |
| GetVersion | `/v1/get-version` | `{}` | VersionInfo |
| ValidateProfile | `/v1/validate-profile` | `{"profile": <Profile>, "allowed_rule_set_hosts": ["api.example.com"], "routing_mode": "rules"}` | `{"valid":true}` |
| ApplyProfile | `/v1/apply-profile` | `{"profile": <Profile>, "allowed_rule_set_hosts": ["api.example.com"], "routing_mode": "rules"}` | `{"applied":true|false}` |
| Start | `/v1/start` | `{}` | `{}` |
| Stop | `/v1/stop` | `{}` | `{}` |
| Reload | `/v1/reload` | `{}` | `{}` |
| GetStatus | `/v1/get-status` | `{}` | Status |
| ListNodes | `/v1/list-nodes` | `{}` | NodeSummary[] |
| SelectNode | `/v1/select-node` | `{"node_id":"stable-id"}` | `{"node_id":"stable-id"}` |
| DebugGoroutines（`GET`，0.5.10 起） | `/v1/debug/goroutines` | 无 | 纯文本 goroutine 栈（pprof `debug=2`）；只在 `serve --log-level debug` 时存在，否则返回 `API_NOT_FOUND`。用于 Windows 等无法发 SIGQUIT 的平台定位卡住的位置；同样需要鉴权，宿主默认不应放行 |
| PinIngress | `/v1/pin-ingress` | `{"node_id":"stable-id","endpoint_key":"9002"}`（`null` 为自动） | `{"node_id":"stable-id","endpoint_key":"9002"}` |
| GetSelectedNode | `/v1/get-selected-node` | `{}` | NodeSummary |
| ProbeEntrances | `/v1/probe-entrances` | `{"method":"tcp","timeout_ms":5000,"concurrency":4,"node_ids":["stable-id"]}` | EntranceResult[] |
| ProbeAvailability | `/v1/probe-availability` | `{"node_id":"stable-id","target":"https://example.com/generate_204","timeout_ms":10000}` | AvailabilityResult |
| GetLocalProxyMetadata | `/v1/get-local-proxy-metadata` | `{}` | LocalProxyMetadata[] |
| GetLocalProxyCredential | `/v1/get-local-proxy-credential` | `{"node_id":"stable-id"}`，或 `{"kind":"routed"}`（0.5.12 起） | LocalProxyCredential |
| GetLocalProxyEndpoints | `/v1/get-local-proxy-endpoints` | `{}` | LocalProxyEndpoint[]（兼容接口） |
| SetSystemProxy | `/v1/set-system-proxy` | `{"enabled":true}` | SystemProxyStatus |
| GetSystemProxyEndpoints | `/v1/get-system-proxy-endpoints` | `{}` | SystemProxyStatus |
| GetTraffic | `/v1/get-traffic` | `{}` | Traffic |
| GetConnections | `/v1/get-connections` | `{}` | Connection[] |
| WatchEvents | `GET /v1/watch-events` | 无 | NDJSON Envelope 流 |

`timeout_ms <= 0` 时入口探测默认 5 秒（每个入口）、可用性探测默认 10 秒；最大均为 120 秒。`concurrency < 1` 时默认为 4（同时探测的入口数）。入口探测的 `method` 为 `tcp`（缺省）或 `icmp`，其他值返回 `PROBE_METHOD_UNSUPPORTED`；`node_ids` 缺省时探测全部节点。

`allowed_rule_set_hosts`（可选，字符串数组）是宿主获取 Profile 所用 API 的 authority：`host` 或
`host:port`，不带 scheme、路径或 userinfo，例如 API base `https://api.example.com/api/v1` 对应
`"api.example.com"`（比较时忽略大小写，`:443` 等同于省略端口；IPv6 写作 `[2001:db8::1]`）。
Profile 中每个 `routing.rule_sets[].url` 的主机都必须在其中，否则 `validate-profile` / `apply-profile`
返回 `RULE_SET_HOST_NOT_ALLOWED`；数组中有无法解析的值返回 `RULE_SET_HOSTS_INVALID`。省略该字段时
Profile 照常应用，但规则集一律不下载（状态为 `RULE_SET_HOST_NOT_PINNED`；已缓存且 sha256 匹配的副本
仍会使用）。同 revision 且同 `routing_mode` 的 `apply-profile` 仍直接返回 `applied=false`，不重新下载。

`routing_mode`（可选，核心 0.5.6 起）：`"rules"`（缺省）或 `"global"`，其他值返回 `ROUTING_MODE_INVALID`
（`field` 为 `routing_mode`）。请求体严格拒绝未知字段，所以不要向 0.5.6 之前的核心发送该字段。

- `rules`：应用全部 Profile 规则和 Profile 的 `final`。
- `global`：只保留 `baseline: true` 的规则（保持原顺序），其余 Profile 规则丢弃，`final` 固定为代理 selected
  节点；只有被保留规则引用的规则集才会准备、下载和刷新，`get-status` 的 `rule_sets` 也只列这些。核心自身的
  规则（TUN 的嗅探、DNS 劫持、fake-ip 拒绝、入口直连）不受影响；流分类（`classifyFlow`）与 sing-box 路由使用
  同一份规则。
- 去重键是 `(revision, routing_mode)`：只改 `routing_mode` 的 `apply-profile` 会重新应用同一个 Profile，宿主切换
  模式时直接调用即可，无需重连。运行中的应用会替换引擎：监听端口不变，但已建立的连接会断开。
- 规则集刷新触发的重建和 `reload` 沿用当前模式；`get-status` 的 `routing_mode` 返回当前生效的模式（尚未应用
  Profile 时省略）。

`apply-profile` 在构建配置前准备规则集：`<state_dir>/rule-sets/<id>.srs` 已存在且 sha256 匹配时立即使用；
否则直连下载，总计最多等待 10 秒，超时或失败时按降级规则构建（见 backend-profile.md），不会因规则集而失败。
宿主的 `apply-profile` 调用超时应大于 10 秒。

需要本地代理的方法（`get-local-proxy-metadata`、`get-local-proxy-credential`、`get-local-proxy-endpoints`、`probe-availability`）在 `serve --local-proxy=false` 启动的核心上返回 `LOCAL_PROXY_DISABLED`；`probe-availability` 在核心未 `start` 时返回 `CORE_NOT_RUNNING`（可重试）。

## DTO

### VersionInfo 与 Status

```json
{
  "core_version": "0.5.12",
  "core_api_version": 1,
  "profile_schema_version": 1,
  "flow_adapter_version": 1,
  "local_proxy_contract_version": 1
}
```

```json
{"state":"running","revision":"cfg-42","selected_node_id":"hk-001","node_count":3,"routing_mode":"rules",
 "selected_ingress":{"endpoint_key":"9002","previous_endpoint_key":"9001","role":"backup","switched_at":"2026-07-23T12:00:00Z"},
 "system_proxy":{"available":true,"enabled":false,"listening":false},
 "rule_sets":[{"id":"cn-ip","state":"ready","updated_at":"2026-07-23T12:00:00Z"},
              {"id":"cn-site","state":"unavailable","error":"RULE_SET_DOWNLOAD_FAILED"}]}
```

`state` 可为 `stopped`、`configured`、`running`。尚未应用 Profile 时返回 `stopped` 且 `node_count=0`。

`selected_ingress` 是 selected 节点实际使用的入口副本，标准核心与 TUN 核心都会返回；核心未运行或无法
确定时省略该字段：

- `endpoint_key` / `role`：当前副本及其角色（`primary`/`backup`），取自 Profile；副本有 `label` 时一并返回。多入口节点指承载该节点
  最近一个新连接的副本；该节点尚无流量时为 primary。单入口节点始终是唯一的入口。
- `previous_endpoint_key` / `switched_at`：最近一次切换前的副本和切换时间（RFC 3339，UTC）；从未切换时省略。
  切换包括故障转移到 backup，以及 primary 恢复后回到 primary。重启核心或应用新 revision 后重新计算。

每次切换还会发出 `NodeIngressSwitched` 事件（任意节点，不限 selected）：

```json
{"type":"NodeIngressSwitched","at":"2026-07-23T12:00:00Z","node_id":"hk-001","endpoint_key":"9002","previous_endpoint_key":"9001"}
```

### 入口固定与各入口健康（核心 0.5.7 起）

`pin-ingress` 把节点固定到一个入口（`endpoint_key`），`endpoint_key: null` 恢复自动故障转移。它直接作用于运行中的引擎，
不重建引擎、不改变 revision；核心未运行时先记下，`start` 时生效。固定后该节点只用这一个入口：入口不健康时拨号直接失败，
不回退到其他入口；健康检查照常进行，用来报告它是否可用。固定在同一个核心进程内跨 `apply-profile` 保留，不写入磁盘
（由宿主持久化并在连接后重新下发）；新 Profile 中已没有该节点或该 `endpoint_key` 时自动清除，并发出
`NodeIngressPinCleared` 事件。错误码：`PROFILE_NOT_APPLIED`、`NODE_NOT_FOUND`、`INGRESS_NOT_FOUND`（`endpoint_key`
不属于该节点，或为空字符串）。单入口节点也接受固定，不改变任何行为。

`get-status` 的 `nodes` 按 Profile 顺序列出每个节点：

```json
"nodes":[{"node_id":"hk-001","pinned_endpoint_key":null,
          "ingresses":[{"endpoint_key":"9001","role":"primary","healthy":false,"last_check_at":"2026-07-23T12:00:00Z","consecutive_failures":2,"active":false},
                       {"endpoint_key":"9002","role":"backup","label":"线路 2","healthy":true,"last_check_at":"2026-07-23T12:00:00Z","consecutive_failures":0,"active":true}]}]
```

- `pinned_endpoint_key`：固定的入口；自动模式为 `null`。
- `healthy`、`last_check_at`、`consecutive_failures` 来自经过入口的健康检查（见 backend-profile.md）；单入口节点和核心未运行时
  省略 `healthy` 与 `last_check_at`，节点空闲期间不检查，所以 `last_check_at` 可能较旧或缺省。“当前入口不可用”应以 `healthy`
  为准：入口探测（`probe-entrances`）只测 TCP/ICMP 可达性，测不出“能连上但不转发”。
- `active`：承载该节点最近一个新连接的入口。

### RuleSetStatus

`rule_sets` 按 Profile 顺序列出已应用 Profile 的每个规则集；Profile 未声明规则集时省略。

- `state`：`ready`（本地副本的 sha256 等于 Profile 中的值，正在使用）、`stale`（Profile 的新版本未能获取，
  正在使用更早的已校验副本）、`unavailable`（没有任何副本，引用它的规则被跳过或去掉该规则集）。
- `updated_at`：最近一次确认副本（下载成功、304 或启动时读到匹配的缓存文件）的时间，RFC 3339 UTC；未知时省略。
- `error`：仅在非 `ready` 时出现，取值：`RULE_SET_HOST_NOT_PINNED`（未提供/未包含该主机的
  `allowed_rule_set_hosts`）、`RULE_SET_DOWNLOAD_FAILED`（连接/TLS/超时）、`RULE_SET_HTTP_STATUS`
  （非 200/304，含重定向）、`RULE_SET_TOO_LARGE`（超过 32 MiB）、`RULE_SET_SHA256_MISMATCH`、
  `RULE_SET_INVALID`（不是可读的二进制规则集）、`RULE_SET_STORAGE_FAILED`（写文件失败）、
  `RULE_SET_STORAGE_UNAVAILABLE`（核心没有状态目录）。
- `failures`（0.5.8 起）：非 `ready` 时连续下载失败的次数；为 0 时省略。
- `next_retry_at`（0.5.8 起）：非 `ready` 时下次重试的时间（RFC 3339 UTC）；`RULE_SET_HOST_NOT_PINNED` 与
  `RULE_SET_STORAGE_UNAVAILABLE` 在下次 `apply-profile` 之前不会变化，此时省略。

核心按 `update_interval_seconds` 刷新 `ready` 的规则集（带 `If-None-Match`）；`stale`/`unavailable` 的以
5 秒起指数退避重试（5、10、20 秒……最长 15 分钟，且不超过更新间隔）。同时到期的规则集并发下载（最多 4 个）。
某个非 `ready` 的规则集恢复时，核心认为网络已经恢复，立即重试其余所有非 `ready` 的规则集，而不是各自等待退避。
有规则集从 `unavailable` 变为可用（或反之）时，核心在这一轮到期的下载全部结束后只重建一次配置（等同
`reload`，运行中的实例会被替换，**已建立的连接会断开**，监听端口不变）；已在使用的规则集文件内容更新时由
sing-box 就地重新加载，不重启实例，也不断开连接。状态每次变化都会发出 `RuleSetChanged` 事件，并在核心日志记一行
`msg="rule set"`（`id`、`state`、`error`、`failures`、`next_retry_at`）；`apply timing` 一行附带
`rule_sets_ready`/`rule_sets_stale`/`rule_sets_unavailable` 与 `rebuild`（规则集或 `reload` 触发的重建为 `true`）：

```json
{"type":"RuleSetChanged","at":"2026-07-23T12:00:00Z","rule_set_id":"cn-ip","message":"unavailable","code":"RULE_SET_DOWNLOAD_FAILED"}
```

### NodeSummary

```json
{"id":"3f2c…-128","name":"香港-203.0.113.10","entry_key":"cn-optimized","entry_label":"CN Optimized",
 "protocol":"vless","region":"香港","tcp":true,"udp":true,
 "ingresses":[{"endpoint_key":"9001","replica_ordinal":0,"role":"primary","protocol":"vless"},
              {"endpoint_key":"9002","replica_ordinal":1,"role":"backup","protocol":"shadowsocks"}]}
```

`entry_key` / `entry_label` 原样透传自 Profile（`entry_label` 缺省时省略）。入口的可选展示名
`label` 同样原样透传（缺省时省略），也出现在入口探测结果的 `ingresses[]` 和 `selected_ingress` 中；它只用于展示，
副本以 `endpoint_key` 标识。`ingresses` 按故障转移顺序列出；`protocol` 是第一个（primary）入口的协议。节点列表不含入口地址、TLS 参数或协议凭据。

### EntranceResult 与 AvailabilityResult

```json
{"node_id":"3f2c…-128","method":"icmp","success":true,"latency_ms":95,"endpoint_key":"9002","ingress_role":"backup",
 "ingresses":[
   {"endpoint_key":"9001","replica_ordinal":0,"role":"primary","success":false,"latency_ms":0,"error_code":"ICMP_TIMEOUT"},
   {"endpoint_key":"9002","replica_ordinal":1,"role":"backup","success":true,"latency_ms":95}
 ],
 "measured_at":"2026-07-23T12:00:00Z"}
```

每个入口都被单独测量（`ingresses` 与 Profile 中的入口同序）。节点级 `success`/`latency_ms`/`error_code`/`endpoint_key`/`ingress_role` 描述同一个副本：primary 成功时取 primary；否则取最快的成功 backup；全部失败时报告 primary 的失败。成功时 `latency_ms` 至少为 1（四舍五入到毫秒），失败时为 0。

```json
{"node_id":"hk-001","total_ms":241,"success":true,"http_status":204,"measured_at":"2026-07-23T12:00:01Z"}
```

入口错误码：通用 `CANCELED`、`DNS_FAILED`（入口无 IP 且域名解析失败）；`tcp` 为 `TIMEOUT`、`CONNECT_FAILED`；`icmp` 为 `ICMP_TIMEOUT`、`ICMP_UNREACHABLE`、`ICMP_UNSUPPORTED`（系统不允许非特权 ICMP）、`ICMP_FAILED`。可用性错误码：`TARGET_INVALID`、`CANCELED`、`TIMEOUT`、`PROXY_REQUEST_FAILED`、`HTTP_STATUS`。探测失败通常仍是成功的 API 调用，应检查每项 `success` 和 `error_code`。

### LocalProxyMetadata 与 LocalProxyCredential

所有节点共用**一个** loopback 端口（`127.0.0.1`，同一端口同时提供认证 HTTP 代理/CONNECT
与 SOCKS5），由代理用户名决定路由。用户名有两类：

- 按节点（`kind: "node"`）：用户名 `<prefix>-<node_id>`。`prefix` 是每台设备随机生成的 5 位小写字母数字（如
  `u8f2k`），不含 `-`，因此 `node_id` 本身可以含 `-`（按第一个 `-` 切分）。
- 按规则（`kind: "routed"`，0.5.12 起）：用户名就是裸 `<prefix>`（不含 `-`），不占用任何 node_id。
- 密码是每台设备一个随机 secret，所有节点共用。桌面端以设备码登录、没有账户密码，
  设计稿中“密码同账户”即指这个设备 secret。
- prefix、密码和最终端口在首次使用时生成并写入私有状态文件（0600），重启和 Profile
  更新后保持不变。
- 端口优先使用上次持久化的端口，其次 7890；启动时二者都被占用则改用任意空闲端口并持久化，
  下次优先尝试它。metadata/credential 返回的 `port` 始终是实际监听端口。
- 缺少认证、未知用户名（包括已从 Profile 删除的节点）或错误密码一律拒绝：HTTP（含
  `CONNECT`）返回 `407 Proxy Authentication Required`、`Proxy-Authenticate: Basic realm="ppvpn"`、
  `Content-Length: 0` 与 `Connection: close`，随后正常关闭连接（FIN，不发 RST），浏览器据此弹出
  认证；SOCKS5 返回 RFC 1929 认证失败（同样随后正常关闭，不发 RST）；不支持无认证方法和 SOCKS4。secret 使用常量时间比较。
- 按节点的用户名，流量固定走该节点（多入口节点走其故障转移组），不受 selected 节点或 Profile
  规则影响；流量统计、连接归属和 `probe-availability` 与之前一致。
- 裸 `<prefix>` 的流量与系统代理（7891）走同一条路：先匹配 Profile 规则（含 DIRECT 分流），其余走
  selected 节点；遵循当前 `routing_mode`（`global` 时只保留 baseline 规则）；`select-node`、`apply-profile`
  对新连接立即生效；流量计入 `get-traffic` 与 `get-connections`。与系统代理一样，非 TUN 模式下核心不嗅探，
  SOCKS5 以 IP 为目标的连接没有域名，**不会命中域名规则**；要让域名规则生效，客户端应把域名交给代理：
  用 HTTP 代理/CONNECT，或 SOCKS5 远端解析（如 curl `--socks5-hostname`、`ALL_PROXY=socks5h://…`）。

一般 UI 状态只能读取不含 secret 的 metadata。每个节点一项（按 `node_id` 排序），0.5.12 起在**最后**追加一项
`kind: "routed"`（`node_id` 为空）；`listen`/`port` 对所有项相同：

```json
[
  {"kind":"node","node_id":"hk-001","listen":"127.0.0.1","port":7890,"protocols":["http","socks5"],"auth_required":true},
  {"kind":"node","node_id":"jp-002","listen":"127.0.0.1","port":7890,"protocols":["http","socks5"],"auth_required":true},
  {"kind":"routed","node_id":"","listen":"127.0.0.1","port":7890,"protocols":["http","socks5"],"auth_required":true}
]
```

`kind` 从 0.5.12 起出现；宿主遇到不认识的 `kind` 必须跳过该项，不要假设每一项都是节点。

只有用户明确打开原生凭据面板时，宿主才能获取 credential。按节点用 `{"node_id":"hk-001"}`：

```json
{"kind":"node","node_id":"hk-001","listen":"127.0.0.1","port":7890,"username":"u8f2k-hk-001","password":"..."}
```

按规则用 `{"kind":"routed"}`（0.5.12 起），同一端口、同一密码：

```json
{"kind":"routed","node_id":"","listen":"127.0.0.1","port":7890,"username":"u8f2k","password":"..."}
```

`kind` 可选，取值 `"node"`（缺省）或 `"routed"`，其他值返回 `REQUEST_INVALID`（`field` 为 `kind`）；
`kind` 为 `"routed"` 时不得带 `node_id`（否则 `REQUEST_INVALID`）；尚未应用 Profile 时返回 `PROFILE_NOT_APPLIED`。
空 `node_id` 不是 routed 的隐式写法，仍返回 `NODE_NOT_FOUND`。请求体严格拒绝未知字段，所以不要向 0.5.12
之前的核心发送 `kind`（会得到 `REQUEST_INVALID`）。移动端对应 `LocalProxyRoutedCredential()`。

字段与之前相同，宿主可以继续把每个节点当作独立的 `{host, port, username, password}` 使用。
端口只可能在核心未运行时应用 Profile（即启动前的端口协调）时改变；宿主应在 `start`
之后（以及每次重新启动核心后）重新读取 metadata，而不是缓存旧端口。

凭据是高敏感设备本地秘密；credential 方法和旧兼容接口返回密码，宿主不得把响应传给
WebView、渲染进程、崩溃报告或日志。旧的 `GetLocalProxyEndpoints` 为 Core API v1 兼容保留，
会一次返回所有按节点的 credential（不含 `kind`，也不含 routed 项）；新宿主不得调用。

### SystemProxyStatus（可选的系统代理监听器）

供宿主的「兼容模式」使用：操作系统代理设置无法携带凭据，所以这是一个**无认证**的回环
HTTP/SOCKS5 监听器（同一端口）。它默认关闭，由宿主在运行时开关：

```json
{"available":true,"enabled":true,"listening":true,"listen":"127.0.0.1","port":7891,"protocols":["http","socks5"]}
```

- `POST /v1/set-system-proxy {"enabled":true|false}`：幂等；缺少 `enabled` 返回 `REQUEST_INVALID`。
  核心运行时只增删这一个监听器，不重启核心、不中断其他连接；核心未运行时开启，在下次 `start` 后监听。
  关闭立即停止监听。开关状态不持久化，核心每次启动都是关闭的。
- `get-status` 总是带 `system_proxy`；`/v1/get-system-proxy-endpoints` 返回同样的内容。`enabled=false`
  时省略 `listen`、`port`、`protocols`；`listening` 仅在运行中的实例实际接受连接时为 true。
- 只绑定 `127.0.0.1`。端口优先使用上次持久化的端口，其次 7891，都被占用则用任意空闲端口，
  写入本地代理状态文件（`system_proxy_port`），且不与共享本地代理端口相同。
- 路由：先匹配 Profile 规则（含 DIRECT 分流），其余流量走 selected 节点；`select-node` 对新连接立即生效；
  流量计入 `get-traffic` 与 `get-connections`。
- 特权 TUN 核心不提供该监听器（`available=false`，开启返回 `SYSTEM_PROXY_UNAVAILABLE`）。

安全边界：开启期间，本机任何进程都可以不经认证使用该端口访问网络。宿主只应在用户选择兼容模式
并且处于已连接状态时开启，断开、切换到增强模式、退出登录或退出时立即关闭。每个节点的共享本地代理
仍然要求凭据，不受影响。

### Traffic 与 Connection

```json
{"upload_bytes":1234,"download_bytes":5678,"measured_at":"2026-07-23T12:00:00Z"}
```

```json
[
  {
    "id":"f2b8...",
    "node_id":"hk-001",
    "network":"tcp",
    "destination":"example.com:443",
    "upload_bytes":512,
    "download_bytes":2048,
    "started_at":"2026-07-23T11:59:59Z"
  }
]
```

Traffic 是当前运行实例的累计计数；重启或替换实例后归零。Connections 只包含仍活动的连接，关闭后移除。

## 事件流

`GET /v1/watch-events` 返回 `Content-Type: application/x-ndjson`，每行一个独立 Envelope：

```json
{"request_id":"events-1","ok":true,"data":{"type":"NodeSelected","at":"2026-07-23T12:00:00Z","revision":"cfg-42","node_id":"hk-001"}}
```

事件类型：`CoreStarted`、`CoreStopped`、`ProfileApplied`、`NodeEndpointChanged`、`NodeSelected`、`ReloadFailed`、`EntranceProbed`、`AvailabilityProbed`、`NodeIngressSwitched`（附 `endpoint_key`、`previous_endpoint_key`）、`NodeIngressPinned`（附 `endpoint_key`，恢复自动时为空）、`NodeIngressPinCleared`（附 `endpoint_key` 与新 `revision`）、`SystemProxyChanged`（`message` 为 `enabled` 或 `disabled`）、`RuleSetChanged`（附 `rule_set_id`；`message` 为新状态，`code` 为非 ready 时的错误码）。`message` 只包含第一方安全摘要，如 `success` 或探测错误码，不含上游错误原文。

事件不持久化且缓冲区满时可丢弃。因此它适合触发 UI 刷新，不适合作为唯一事实来源或审计日志。

## 稳定错误码

| code | 含义/处理 |
| --- | --- |
| `UNAUTHENTICATED` | 重新完成本次进程的会话密钥握手 |
| `CORE_API_UNSUPPORTED` | 阻止继续调用并提示升级宿主或核心 |
| `API_NOT_FOUND` | 客户端与核心 API 不匹配 |
| `REQUEST_INVALID` | 修正请求 JSON、字段或大小 |
| `PROFILE_REQUIRED` / `FIELD_REQUIRED` | Profile 缺少必要数据 |
| `SCHEMA_UNSUPPORTED` | 后端 Schema 与核心不兼容 |
| `PROFILE_EXPIRED` / `TIME_RANGE_INVALID` | 重新获取 Profile 或修正时间 |
| `NODE_ID_INVALID` / `NODE_ID_DUPLICATE` | 修正后端稳定 ID |
| `ENTRY_IP_NOT_PUBLIC` / `PORT_INVALID` | 修正入口地址 |
| `CREDENTIALS_INVALID` | 凭据联合体、内容或编码不合法 |
| `PROTOCOL_UNSUPPORTED` / `TRANSPORT_UNSUPPORTED` | Profile 不支持该功能 |
| `INGRESS_ROLE_INVALID` / `INGRESS_COUNT_INVALID` | 入口为 1–64 个；`ingresses[0]` 为 primary，其余为 backup |
| `ENTRY_KEY_INVALID` | `entry_key` 缺失或不符合 `[A-Za-z0-9][A-Za-z0-9._-]{0,63}` |
| `ENDPOINT_KEY_INVALID` / `ENDPOINT_KEY_DUPLICATE` | `endpoint_key` 缺失、格式不符或在 Profile 内重复 |
| `INGRESS_LABEL_INVALID` | 可选的 `ingresses[].label` 为空、首尾有空白、含控制字符或超过 32 个字符 |
| `REPLICA_ORDINAL_INVALID` | `replica_ordinal` 为负，或在 Node 内未按数组顺序严格递增（缺失报 `FIELD_REQUIRED`） |
| `EXIT_IP_INVALID` | `exit.ip` 不是合法 IP |
| `SHADOWSOCKS_METHOD_UNSUPPORTED` / `SHADOWSOCKS_KEY_INVALID` | 修正 SS 2022 方法或密钥长度 |
| `SHADOWSOCKS_SERVER_KEY_REMOVED` | Profile 仍带已删除的 `shadowsocks.server_key`；改为 `identity_keys`（服务端 iPSK）+ `user_key`（用户 uPSK），见 backend-profile.md |
| `REALITY_REQUIRED` / `REALITY_PUBLIC_KEY_INVALID` / `REALITY_SHORT_ID_INVALID` | 修正 REALITY 配置 |
| `TLS_REQUIRED` / `TLS_SERVER_NAME_MISMATCH` / `TLS_SERVER_NAME_INVALID` | 修正 TLS 与连接域名（AnyTLS 须相等；REALITY 须为合法域名） |
| `CAPABILITIES_INVALID` | 至少启用 TCP 或 UDP |
| `DEFAULT_NODE_NOT_FOUND` / `SELECTION_MODE_UNSUPPORTED` | 修正默认选择 |
| `RULE_SET_ID_INVALID` / `RULE_SET_ID_DUPLICATE` / `RULE_SET_COUNT_INVALID` | 修正 `routing.rule_sets` 的 id 或数量（最多 32） |
| `RULE_SET_URL_INVALID` / `RULE_SET_SHA256_INVALID` / `RULE_SET_INTERVAL_INVALID` | 规则集 URL 必须是 https；sha256 为 64 位 hex；更新间隔不能为负 |
| `RULE_SET_NOT_FOUND` / `RULE_SET_REF_DUPLICATE` | `match.rule_set_ids` 引用了不存在或重复的规则集 |
| `RULE_SET_HOST_NOT_ALLOWED` | 规则集 URL 的主机不在 `allowed_rule_set_hosts` 中；后端配置错误，不可重试 |
| `RULE_SET_HOSTS_INVALID` | `allowed_rule_set_hosts` 中有无法解析的 authority |
| `NODE_NOT_FOUND` | 刷新节点列表；节点可能已被新 Profile 移除 |
| `SYSTEM_PROXY_UNAVAILABLE` | 该核心不提供系统代理监听器（TUN 核心，或没有私有状态目录）；不可重试 |
| `SYSTEM_PROXY_START_FAILED` | 系统代理监听端口无法打开；可重试 |
| `PROFILE_NOT_APPLIED` | 先应用有效 Profile，再执行需要运行配置的方法 |
| `PROBE_METHOD_UNSUPPORTED` | 入口探测 `method` 只能是 `tcp` 或 `icmp` |
| `LOCAL_PROXY_DISABLED` | 该核心以 `--local-proxy=false` 启动；改用本地代理核心 |
| `CORE_NOT_RUNNING` | 先调用 `/v1/start` |
| `STREAM_UNSUPPORTED` | 当前 HTTP writer 无法刷新事件流 |
| `CORE_OPERATION_FAILED` | 安全折叠后的内部失败；读取状态并按产品策略重试/上报。响应不含原因；原因、阶段（如 `apply/local-proxy-state`、`start > engine-start/tun-open`）和错误链以同一 `request_id` 写入核心日志 |

## Unix Socket 调试示例

```sh
SOCKET=/private/app/core.sock
SECRET=$(cat /private/app/session.secret)

curl --unix-socket "$SOCKET" \
  -H "Authorization: Bearer $SECRET" \
  -H 'X-Core-API-Version: 1' \
  -H 'Content-Type: application/json' \
  --data '{}' \
  http://localhost/v1/get-status
```

该示例仅用于受控开发环境。不要在共享 shell 历史、CI 日志或诊断脚本中打印 `$SECRET`。
