# 移动端接入

本文说明 Rust 引擎目前能给 iOS 和 Android 宿主提供什么。接口契约见 [宿主接入](host-integration.md)。

## 现状

移动端还不能接入：仓库里没有 `ppvpn-core-ffi`，也没有给移动端的 XCFramework 或 AAR，CI 也不为 iOS、Android 构建。

已有的只是 Rust 库本身的准备：

- `Platform` 有 `Ios` 和 `Android` 两个取值，引擎按它们生成移动端的 TUN 配置（见下文）。
- 公开 API 按"能直接包装成 FFI"设计（[宿主接入](host-integration.md) 第 1 节）。

Go 版的移动端桥接和 flow adapter 没有宿主使用，Rust 版不提供（`docs/rust-parity.md`）。

## 计划中的 FFI

将来的 `ppvpn-core-ffi`（C ABI 或 UniFFI）直接包装现有接口，不另设一套 API。为此，公开 API 遵守这几条：

- 公开类型都是值类型，能用 serde 序列化成 JSON，字段名用 snake_case；
- 不暴露泛型、生命周期参数和 trait 对象，句柄是不透明的、引用计数的 `Engine`；
- 事件按种类订阅，每种一个有界通道，不用回调闭包；
- Profile 以原始 JSON 字节传入，宿主不解析它；用 JSON 反序列化 `ApplyRequest` 时，只有 `profile` 是必需的；
- 异步方法是普通的 `async fn`，由 FFI 层决定用回调还是阻塞包装。

方法、状态、事件和错误码就是 [宿主接入](host-integration.md) 里的那一套，移动端不会有单独的版本。

## 移动端的 TUN

`Role::Tun` 加 `Platform::Ios` 或 `Platform::Android` 时，引擎生成的 TUN 配置和桌面不同：

- 系统 VPN 和隧道由宿主建立（iOS 的 `NEPacketTunnelProvider`、Android 的 `VpnService`）。引擎不设置 `auto_route`、`strict_route` 和路由层的地址排除，也不给 TUN 网卡起名；入口地址在路由规则里照样直连。
- 隧道只有 IPv4，不探测主机的 IPv6，也不记 `host ipv6` 日志行。
- 嗅探、DNS 劫持、隧道自身网段和 fake-ip 段的拒绝、私网直连这些前导规则和桌面一样。

把宿主建好的 TUN 文件描述符交给引擎的接口还没有：`TunConfig` 目前只有 `local_dns_servers` 和 `wintun_dll`。

另外两点与移动宿主有关，接入前要重新评估：

- 网络变化目前只来自 Sail 自己的网卡监视器，引擎不向 Sail 推送网络状态。移动端经 FFI 推送网络状态时，Sail 在"宿主推送"加 `auto_detect_interface` 时两边都会宣告网络变化（[宿主接入](host-integration.md) 第 11 节）。
- 入口探测的 ICMP 不需要特权：iOS 用 `SOCK_DGRAM` ICMP socket，Android 需要 `net.ipv4.ping_group_range` 包含当前组，否则报 `ICMP_UNSUPPORTED`。

## 宿主的职责

这些不随 FFI 改变，接入时由宿主负责：

- 让引擎实例与系统 VPN 的生命周期同生共死，不在普通 UI 进程里长期运行它；
- VPN 权限授权、前台服务、Always-on VPN、进程重建和通知；
- 持久化选中节点、ingress pin 和 `routing_mode`，每次 apply 一起传入；
- Profile 只放在内存里（里面有节点凭据）。若经 App Group 文件或 Binder 在 App 和扩展/服务之间传递，写入要有数据保护、原子替换，读完即删。
