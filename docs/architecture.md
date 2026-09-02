# 架构与生命周期

## 设计原则

核心的公开契约只有两层：后端到核心的 Profile Schema，以及桌面/移动宿主到核心的第一方控制接口。sing-box 是固定版本的内部执行引擎，不是产品协议。升级 sing-box 时不要求后端、桌面端或移动端同步理解其配置格式。

```mermaid
flowchart LR
  Backend["ppvpn-backend"] -->|"Profile v2"| Profile["profile: 解析、迁移、校验"]
  Desktop["桌面 App"] -->|"Core API v1 + 会话密钥"| API["api / ipc"]
  Mobile["Network Extension / VpnService"] -->|"mobile.Bridge JSON DTO"| Runtime["internal/runtime"]
  API --> Runtime
  Profile --> Builder["internal/config"]
  Runtime --> Builder
  Builder -->|"option.Options（仅内部）"| SingBox["sing-box 1.13.12"]
  Runtime --> Probe["入口与可用性探测"]
  Runtime --> Telemetry["第一方流量、连接、事件"]
  Runtime --> LocalProxy["每节点认证 HTTP/SOCKS5 代理"]
```

## 包边界

| 包 | 职责 | 是否公开稳定契约 |
| --- | --- | --- |
| `profile` | Profile DTO、严格解析、版本迁移、语义校验 | 是 |
| `api` | Core API v1 HTTP 契约、认证、统一错误封装 | 是 |
| `ipc` | Unix Domain Socket / Windows Named Pipe | 是 |
| `mobile` | gomobile 可绑定的 JSON DTO 桥 | 是 |
| `probe` | 入口 TCP 和端到端可用性探测 | 是 |
| `localproxy` | 设备本地节点代理端点与状态 | 是，但凭据只限受信宿主 |
| `internal/config` | Profile 到 sing-box `option.Options` 的直接构建 | 否 |
| `internal/runtime` | 引擎生命周期、热更新、遥测和事件 | 否 |
| `internal/redact` | 诊断输出脱敏 | 否 |

## 生命周期状态机

```mermaid
stateDiagram-v2
  [*] --> stopped
  stopped --> configured: ApplyProfile 成功
  configured --> running: Start
  running --> configured: Stop
  configured --> configured: ApplyProfile / Reload
  running --> running: ApplyProfile / Reload（替换成功）
```

- 未应用 Profile 时，状态是 `stopped`；此时调用 `Start` 失败。
- `ApplyProfile` 会先复制、校验并构建候选配置。revision 与当前值相同则返回 `applied=false`，不触发重载。
- 运行中应用新 revision 时，核心启动替换实例。存在固定本地代理端口时会先停止旧实例以释放端口；若候选启动失败，则用旧构建结果恢复运行。
- `SelectNode` 原子更新 `selected` selector，不重建 runtime。已有 TCP/UDP flow 保持原节点，新 flow 使用新节点。
- `Reload` 重建当前 Profile，但对外保留原 revision。
- `Stop` 和重复 `Start`/`Stop` 是幂等的。

## Profile 与平台能力分离

Profile 描述“连接到哪些服务以及使用什么协议”；`PlatformCapabilities` 描述“本设备允许核心做什么”。TUN、本地监听地址、日志级别和平台名称只能由宿主提供，后端不得下发。

桌面 `serve` 默认启用每节点认证代理。每节点代理为每个稳定
node ID 提供一个同时支持 HTTP/SOCKS5 的认证端点。Windows 宿主以特权 TUN 接管流量，
macOS 宿主使用原生 Network Extension；Profile 不包含系统级隧道配置。

## TUN 实际边界

`PlatformCapabilities.tun.enabled=true` 确实进入配置构建：macOS/Windows 会生成带 `auto_route`、`strict_route` 的 sing-box TUN inbound。桌面 CLI 提供 `--tun` 与 `--tun-stack`，但 core 不获取管理员/root 权限，不安装或打开平台驱动，不设置系统代理，也不承载平台 UI。因此普通权限 sidecar 不能被视为已经具备可交付 TUN。

Windows Desktop 必须由已签名 privileged service 提供权限和平台资源，再以 `--tun`
启动 core；失败时保持未连接并通知用户，不静默回退到系统代理。macOS Desktop 使用
`NETransparentProxyProvider` 内的 XCFramework flow adapter。iOS `NEPacketTunnelProvider`
与 Android `VpnService` 仍由宿主创建系统 VPN/TUN；移动 TUN 文件描述符桥接是后续接入点。

## 探测语义

- 入口探测直接 TCP 连接 `endpoint.ip:endpoint.port`，不查询 DNS，也不做协议握手；结果字段是 `connect_ms`。
- 可用性探测通过指定节点的认证本地 HTTP 代理发起完整 HTTP 请求；结果字段是 `total_ms`。它包含代理握手、节点连接和目标响应时间，不能当作入口延迟。
- 两类探测都有明确的超时、取消和稳定错误码，且会产生结构化事件。

## 遥测与事件

流量和活动连接由第一方路由跟踪器统计，不依赖 Clash API。公开连接只包含稳定 `node_id`，内部 outbound tag 不会序列化。事件是尽力投递：订阅者缓冲区满时丢弃新事件，调用方应在重连后用 `GetStatus` 重新同步当前状态，而不能把事件流当作持久化日志。
