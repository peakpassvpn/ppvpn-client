# Desktop 平台接入

## Sidecar 启动

macOS 使用私有 Unix Socket，Windows 使用 owner-only Named Pipe。Desktop 创建权限受限的状态目录和 stdin 存活管道，并启动：

```text
ppvpn-core serve
  --socket <private socket or pipe>
  --session-secret-file <private exchange file>
  --state-dir <private state directory>
  --platform macos|windows
  --local-proxy=true
  --system-proxy=true
  --system-proxy-listen=127.0.0.1
  --exit-on-stdin-close
```

不要从后端 Profile、用户偏好或 WebView 传入 listen、port 或认证配置。Desktop 只决定是否启用本地平台能力。

## 系统代理调用顺序

1. 读取本次进程的 session secret，建立 Core API v1 客户端。
2. `POST /v1/apply-profile`。
3. `POST /v1/start`。
4. `POST /v1/get-system-proxy-endpoints`。
5. 将 `http` 设置为系统 HTTP 与 HTTPS proxy，将 `socks5` 设置为系统 SOCKS proxy。mixed 入口通常是同一 `127.0.0.1:port`，不设置用户名或密码。
6. 节点切换只调用 `POST /v1/select-node`；新连接立即进入新 selected node，端口不变。
7. 监听 `SystemProxyEndpointReady`。若端口因启动冲突迁移，用事件的新端点原子更新系统设置。
8. 正常退出时先清除系统代理，再调用 `POST /v1/stop`。sidecar 崩溃或失联时，Desktop 的恢复逻辑也必须清除指向失效端口的系统设置。

`GetStatus.system_proxy.available` 是事实来源。不要把端口长期缓存到 Desktop 配置；`system-proxy.json` 由 core 独占。

## 两类本地代理

- `GetSystemProxyEndpoints`：单一免认证 mixed 入口，绑定明确 loopback，跟随 selected，供操作系统代理使用。
- `GetLocalProxyEndpoints`：每个稳定 node ID 一个独立 mixed 入口，强制随机用户名/密码，固定走对应节点，供指纹浏览器等隔离场景使用。

Desktop 不能把 per-node 凭据模型用于系统代理，也不能把免认证系统入口当作长期第三方隔离代理。

## TUN

`--tun --tun-stack mixed` 会让 core 构建 sing-box TUN inbound，但 core 不包含提权 helper、驱动安装、系统设置或 UI。macOS/Windows 产品只有在 Desktop 提供已签名 privileged helper/service、平台权限和故障回滚后才能启用；当前默认路径应使用应用主代理入口。TUN 能力只由本地宿主决定，永不进入后端 Profile。
