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
| ValidateProfile | `/v1/validate-profile` | `{"profile": <Profile>}` | `{"valid":true}` |
| ApplyProfile | `/v1/apply-profile` | `{"profile": <Profile>}` | `{"applied":true|false}` |
| Start | `/v1/start` | `{}` | `{}` |
| Stop | `/v1/stop` | `{}` | `{}` |
| Reload | `/v1/reload` | `{}` | `{}` |
| GetStatus | `/v1/get-status` | `{}` | Status |
| ListNodes | `/v1/list-nodes` | `{}` | NodeSummary[] |
| SelectNode | `/v1/select-node` | `{"node_id":"stable-id"}` | `{"node_id":"stable-id"}` |
| GetSelectedNode | `/v1/get-selected-node` | `{}` | NodeSummary |
| ProbeEntrances | `/v1/probe-entrances` | `{"timeout_ms":5000,"concurrency":4}` | EntranceResult[] |
| ProbeAvailability | `/v1/probe-availability` | `{"node_id":"stable-id","target":"https://example.com/generate_204","timeout_ms":10000}` | AvailabilityResult |
| GetLocalProxyMetadata | `/v1/get-local-proxy-metadata` | `{}` | LocalProxyMetadata[] |
| GetLocalProxyCredential | `/v1/get-local-proxy-credential` | `{"node_id":"stable-id"}` | LocalProxyCredential |
| GetLocalProxyEndpoints | `/v1/get-local-proxy-endpoints` | `{}` | LocalProxyEndpoint[]（兼容接口） |
| GetSystemProxyEndpoints | `/v1/get-system-proxy-endpoints` | `{}` | 固定返回 `SYSTEM_PROXY_UNAVAILABLE`（API v1 兼容路由） |
| GetTraffic | `/v1/get-traffic` | `{}` | Traffic |
| GetConnections | `/v1/get-connections` | `{}` | Connection[] |
| WatchEvents | `GET /v1/watch-events` | 无 | NDJSON Envelope 流 |

`timeout_ms <= 0` 时入口探测默认 5 秒、可用性探测默认 10 秒；最大均为 120 秒。`concurrency < 1` 时默认为 4。

## DTO

### VersionInfo 与 Status

```json
{
  "core_version": "0.3.0",
  "core_api_version": 1,
  "profile_schema_version": 2,
  "flow_adapter_version": 1,
  "local_proxy_contract_version": 1
}
```

```json
{"state":"running","revision":"cfg-42","selected_node_id":"hk-001","node_count":3}
```

`state` 可为 `stopped`、`configured`、`running`。尚未应用 Profile 时返回 `stopped` 且 `node_count=0`。

### NodeSummary

```json
{"id":"hk-001","name":"香港 01","protocol":"vless","region":"Hong Kong","country_code":"HK","tcp":true,"udp":true}
```

节点列表不含入口地址、TLS 参数或协议凭据。

### EntranceResult 与 AvailabilityResult

```json
{"node_id":"hk-001","connect_ms":82,"success":true,"measured_at":"2026-07-23T12:00:00Z"}
```

```json
{"node_id":"hk-001","total_ms":241,"success":true,"http_status":204,"measured_at":"2026-07-23T12:00:01Z"}
```

入口错误码：`CANCELED`、`TIMEOUT`、`CONNECT_FAILED`。可用性错误码：`TARGET_INVALID`、`CANCELED`、`TIMEOUT`、`PROXY_REQUEST_FAILED`、`HTTP_STATUS`。探测失败通常仍是成功的 API 调用，应检查每项 `success` 和 `error_code`。

### LocalProxyMetadata 与 LocalProxyCredential

一般 UI 状态只能读取不含 secret 的 metadata：

```json
[
  {
    "node_id":"hk-001",
    "listen":"127.0.0.1",
    "port":32145,
    "protocols":["http","socks5"],
    "auth_required":true
  }
]
```

只有用户明确打开原生凭据面板时，宿主才能按 node ID 获取该项 credential：

```json
{"node_id":"hk-001","listen":"127.0.0.1","port":32145,"username":"...","password":"..."}
```

同一端口同时提供认证 HTTP CONNECT 与 SOCKS5。凭据是高敏感设备本地秘密；credential
方法和旧兼容接口返回密码，宿主不得把响应传给 WebView、渲染进程、崩溃报告或日志。
旧的 `GetLocalProxyEndpoints` 为 Core API v1 兼容保留，会一次返回所有 credential；新宿主不得调用。

### 已移除的系统代理兼容接口

Core 不再创建无认证 loopback HTTP/SOCKS5 监听器。Core API v1 暂时保留
`/v1/get-system-proxy-endpoints` 路由，并始终返回 `SYSTEM_PROXY_UNAVAILABLE`；后续 API
主版本可以删除该路由。

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

事件类型：`CoreStarted`、`CoreStopped`、`ProfileApplied`、`NodeEndpointChanged`、`NodeSelected`、`ReloadFailed`、`EntranceProbed`、`AvailabilityProbed`。`message` 只包含第一方安全摘要，如 `success` 或探测错误码，不含上游错误原文。

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
| `PROTOCOL_UNSUPPORTED` / `TRANSPORT_UNSUPPORTED` | Profile v2 不支持该功能 |
| `SHADOWSOCKS_METHOD_UNSUPPORTED` / `SHADOWSOCKS_KEY_INVALID` | 修正 SS 2022 方法或密钥长度 |
| `REALITY_REQUIRED` / `REALITY_SHORT_ID_INVALID` | 修正 REALITY 配置 |
| `TLS_REQUIRED` / `TLS_SERVER_NAME_MISMATCH` | 修正 TLS 与连接域名 |
| `CAPABILITIES_INVALID` | 至少启用 TCP 或 UDP |
| `DEFAULT_NODE_NOT_FOUND` / `SELECTION_MODE_UNSUPPORTED` | 修正默认选择 |
| `NODE_NOT_FOUND` | 刷新节点列表；节点可能已被新 Profile 移除 |
| `SYSTEM_PROXY_UNAVAILABLE` | 无认证系统代理能力已移除；调用方不得重试或降级 |
| `PROFILE_NOT_APPLIED` | 先应用有效 Profile，再执行需要运行配置的方法 |
| `STREAM_UNSUPPORTED` | 当前 HTTP writer 无法刷新事件流 |
| `CORE_OPERATION_FAILED` | 安全折叠后的内部失败；读取状态并按产品策略重试/上报 |

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
