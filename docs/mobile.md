# 移动端接入

移动端通过 gomobile 绑定 [`mobile.Bridge`](../mobile/bridge.go)。所有复杂输入输出都是 JSON 字符串，公开类型不包含 sing-box 值。宿主必须让 Bridge 与系统 VPN 生命周期同生共死，不能在普通 UI 进程里长期运行核心。

## 公共方法

| Bridge 方法 | 输入 | JSON 输出/行为 |
| --- | --- | --- |
| `NewBridge(platformJSON, statePath)` | PlatformCapabilities、私有状态文件路径 | 创建实例 |
| `Version()` | 无 | VersionInfo |
| `ValidateProfile(profileJSON)` | Profile | `{"valid":true}` |
| `ApplyProfile(profileJSON)` | Profile | `{"applied":boolean}` |
| `Start()` / `Stop()` | 无 | 生命周期控制 |
| `Status()` | 无 | Status |
| `ListNodes()` | 无 | 安全节点摘要数组 |
| `SelectNode(nodeID)` | 稳定节点 ID | 更新选择 |
| `ProbeEntrances(timeoutMS, concurrency)` | 超时、并发 | EntranceResult[] |
| `ProbeAvailability(nodeID, target, timeoutMS)` | 节点、HTTP(S) URL、超时 | AvailabilityResult |
| `LocalProxyEndpoints()` | 无 | 本地代理端点及凭据 |
| `SystemProxyEndpoints()` | 无 | 当前系统 HTTP/SOCKS5 代理端点 |
| `Traffic()` / `Connections()` | 无 | 第一方遥测 DTO |
| `WatchEvents(handler)` | 实现 `OnEvent(string)` 的回调 | EventWatcher |

返回错误只包含 Profile 稳定校验码或通用 `core operation failed`，不会透传 sing-box 原始错误。宿主不要依赖错误字符串做精细业务判断；需要稳定的桌面式错误封装时，应在平台 IPC 层统一映射。

PlatformCapabilities 示例：

```json
{
  "platform": "android",
  "tun": {"enabled": false},
  "system_proxy": {"enabled": false},
  "local_proxy": {"enabled": true, "listen": "127.0.0.1"},
  "log_level": "info"
}
```

若 `local_proxy.enabled=true`，`statePath` 必须是文件路径（例如 `<filesDir>/ppvpn/local-proxies.json`），不是目录。宿主需提前确保父目录只对本 App 可见。

## iOS

### 集成位置

1. 用 `make build-mobile-ios` 生成 `build/PPVPNCore.xcframework`。
2. 将 XCFramework 链接到 Packet Tunnel Extension target，而不是只链接主 App。
3. 在 `NEPacketTunnelProvider` 中创建并持有 Bridge。
4. `startTunnel` 中依次 ApplyProfile、配置宿主拥有的 `NEPacketTunnelNetworkSettings`、Start。
5. `stopTunnel` 中关闭 EventWatcher、Stop 并释放 Bridge。
6. 主 App 通过 `NETunnelProviderSession.sendProviderMessage` 发送第一方 DTO；不要发送 sing-box JSON。

示意流程（Swift 方法名以生成的 Objective-C/Swift 绑定为准）：

```swift
// PacketTunnelProvider 内；错误处理按应用规范展开
let platform = #"{"platform":"ios","tun":{"enabled":false},"system_proxy":{"enabled":false},"local_proxy":{"enabled":true,"listen":"127.0.0.1"},"log_level":"info"}"#
bridge = try MobileNewBridge(platform, appGroupStateFile.path)
_ = try bridge.applyProfile(profileJSON)
try bridge.start()
```

Profile 凭据只留在 Extension 内存。若通过 App Group 文件交换 Profile，写入必须使用数据保护和原子替换，Extension 读完即删除。主 App 不得把 Profile 放入 `UserDefaults`、URL scheme 或通知 payload。

## Android

### 集成位置

1. 用 `make build-mobile-android` 生成 `build/ppvpn-core.aar`。
2. 将 AAR 加入 app module，minSdk 至少 23。
3. 在前台 `VpnService` 中创建并持有 Bridge。
4. Service 获取 Profile 后 ApplyProfile；宿主用 `VpnService.Builder` 建立系统 TUN，再调用 Start。
5. `onDestroy` 中关闭 EventWatcher 并调用 Stop。
6. Activity 与 Service 通过 Binder 传递第一方 JSON DTO；对大 Profile 使用 App 私有文件加短期 token，避免 Binder 大事务。

Kotlin 示意（具体生成包名以 AAR 为准）：

```kotlin
val platform = """{
  "platform":"android",
  "tun":{"enabled":false},
  "system_proxy":{"enabled":false},
  "local_proxy":{"enabled":true,"listen":"127.0.0.1"},
  "log_level":"info"
}""".trimIndent()

bridge = Mobile.newBridge(platform, File(filesDir, "ppvpn/local-proxies.json").path)
bridge.applyProfile(profileJson)
bridge.start()
```

Service 必须在平台要求的时间内进入前台，并由宿主处理 VPN 权限授权、网络切换、Always-on VPN、进程重建和通知。核心不会代替平台创建 TUN，也不会持久化当前 Profile。

虽然 `tun.enabled` 会进入 sing-box 配置构建，当前 gomobile bridge 尚不能接收 Extension/`VpnService` 所拥有的 TUN 文件描述符，因此 XCFramework/AAR 目前不是可直接运行的移动数据包隧道。文件描述符桥接是后续接入点；移动端当前应保持示例中的 `tun.enabled=false`，不能伪装为已支持。

## 事件与并发

`WatchEvents` 会在 Go 管理的后台 goroutine 上调用 `OnEvent`。回调必须快速返回，并把 UI 工作投递到主线程；不要在回调内同步调用耗时的 Bridge 方法。停止 Extension/Service 时必须调用 `EventWatcher.Close()`，然后调用 `Bridge.Stop()`。

Bridge 内部串行化生命周期变更；状态读取和事件投递可并发。宿主仍应在自己的 Service/Extension 层建立单一 owner，避免多个 UI 请求竞态地切换节点或应用 Profile。

## 构建要求

```sh
make bootstrap-mobile
make build-mobile-ios
make build-mobile-android
```

Android 使用 NDK `29.0.14206865` 和 API 23；AAR 包含 `armeabi-v7a`、`arm64-v8a`、`x86`、`x86_64`。`bootstrap-mobile` 固定 gomobile 版本，CI 不应改用 `@latest`。iOS 产物包含 device arm64 和 simulator arm64/x86_64。
