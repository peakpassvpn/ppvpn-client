# 移动端接入

本文说明 Rust 引擎给 iOS 和 Android 宿主提供什么、还缺什么。接口契约见 [宿主接入](host-integration.md) 第 11 节，计划和分工见 #448。

## 结构

```
界面进程（Flutter + Rust 客户端状态） ── Unix socket，Core API v1 ──  隧道进程
                                                                    VpnService / NEPacketTunnelProvider
                                                                      │ 几个 C 函数（JNI / C ABI）
                                                                    Rust 隧道宿主：Engine + engine-host::dispatch
```

- 引擎只在隧道进程里运行，和系统 VPN 同生共死，不在界面进程里长期运行。
- 界面进程和桌面一样说 Core API v1，只是 transport 换成到隧道进程的 Unix socket。
- 原生层只调隧道宿主的几个 C 函数：启动、停止、网络变化、休眠和唤醒，Android 上还有 socket 保护。完整的引擎 FFI 不做。

## 现状

- **已有：** 引擎的移动端接口（[宿主接入](host-integration.md) 第 11 节）：
  - `tun_settings`：宿主照着它建隧道；
  - `TunConfig::with_fd`：宿主建好的 TUN 交给引擎；
  - `SocketProtect`：Android 的 socket 保护；
  - `Engine::set_network_state`：宿主推送网络；
  - mobile profile 和 `max_connections`。
- **隧道宿主（`crates/tunnel-host`）：**
  - C ABI：`include/ppvpn_tunnel.h` 是契约，函数有 settings、start、stop、network_changed、sleep、wake；
  - 隧道 socket 上的 Core API v1，报文格式见 #448；
  - 隧道进程里的健康检查（见下）；
  - iOS 的 XCFramework：`scripts/build-xcframework.sh <输出目录>`，产物是 `PPVPNTunnel.xcframework`（真机和模拟器的静态库，模块名 `PPVPNTunnel`）和 `PPVPNTunnel.ldflags`（扩展要链接的系统库）。只放进网络扩展，不放进 app。
- **还没有：**
  - Android 的 JNI（`libppvpn_tunnel.so`，Kotlin 类 `com.peakpassvpn.ppvpn.tunnel.TunnelHost`）和交叉编译；
  - 隧道宿主的 CI 只有 iOS 构建和 Linux 上的测试，还没有在设备上跑的测试。

## 隧道进程里的健康检查

界面进程在后台会被挂起，而断线多发生在这时（锁屏、切网络），所以检查和恢复放在隧道宿主里，规则同桌面（`ppvpn_engine_host::health`，#438）：

- **一轮检查：** 引擎在运行（含 degraded），有选中的节点，且它的入口 TCP 探测成功。桌面的直连请求、捕获检查和经本地代理的路径检查在手机上做不了：隧道实例没有本地代理，所有流量都进隧道。
- **恢复：** 在进程内停止再启动引擎，TUN 不动，系统的 VPN 图标不闪。每个恢复周期一次；两轮健康或用户的一次连接（`/v1/start`）开始新的周期。
- **放弃：** 同一周期里第二次需要恢复，或引擎 `Fatal`（无法自愈，而 profile 只在界面进程里）：经 `on_exit` 通知原生层结束隧道，免得设备卡在一个不通的隧道后面。
- **离线：** 原生层报告断网时不检查，这时的失败与隧道无关。
- **报告：** 每一步在 `/v1/watch` 上写一帧 `{"kind": "health", "state": "healthy" | "settling" | "rechecking" | "recovering" | "failed", "detail"?}`。

## 移动端的 TUN

`Role::Tun` 加 `Platform::Ios` 或 `Platform::Android` 时，引擎生成的 TUN 配置和桌面不同：

- **隧道由宿主建立：** 引擎不设置 `auto_route`、`strict_route` 和路由层的地址排除，也不给 TUN 网卡起名。入口地址在路由规则里照样直连。
- **只有 IPv4：** 不探测主机的 IPv6，也不记 `host ipv6` 日志行。MTU 是 9000，和 `tun_settings` 一致。
- **与桌面相同的前导规则：** 嗅探、DNS 劫持、隧道自身网段和 fake-ip 段的拒绝、私网直连。
- **DNS：** 宿主给系统的 DNS 地址是隧道网段里的 `10.60.159.90`。查询进隧道后由 DNS 劫持应答；劫持排在拒绝隧道网段的规则之前。
- **入口探测的 ICMP 不需要特权：** iOS 用 `SOCK_DGRAM` ICMP socket；Android 需要 `net.ipv4.ping_group_range` 包含当前组，否则报 `ICMP_UNSUPPORTED`。

## 宿主的职责

这些由隧道宿主和原生层负责：

- 让引擎实例与系统 VPN 的生命周期同生共死；
- VPN 权限授权、前台服务、Always-on VPN、进程重建和通知；
- 持久化选中节点、ingress pin 和 `routing_mode`，每次 apply 一起传入；
- Profile 只放在内存里（里面有节点凭据），经 socket 随 `apply` 传给隧道进程，不落盘。
