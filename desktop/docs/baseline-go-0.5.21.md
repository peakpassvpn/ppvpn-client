# Go core 0.5.21 基线（切换到 Rust 引擎前）

[#214](https://github.com/peakpassvpn/ppvpn-core/issues/45) 的切换门槛要求 Rust 版和这份基线对比。
安装包大小取打包时写进发布元数据（release-meta）的 `length`，不需要另测。

## 安装包大小

桌面 0.2.90（build 4），core 0.5.21。

| 平台 | 文件 | 字节 | MB |
| --- | --- | --- | --- |
| windows-x64 | `PPVPN-0.2.90-windows-x64-setup.exe` | 83,000,109 | 79.2 |
| macos-arm64 | `PPVPN-0.2.90-macos-arm64.dmg` | 24,129,124 | 23.0 |
| macos-x64 | `PPVPN-0.2.90-macos-x64.dmg` | 25,983,510 | 24.8 |
| linux-x64-deb | `ppvpn_0.2.90-4_amd64.deb` | 63,034,876 | 60.1 |
| linux-x64-rpm | `ppvpn-0.2.90-4.x86_64.rpm` | 59,380,349 | 56.6 |

门槛：Rust 版每个平台不超过上表的 120%。

## 内存（RSS）

Go 版的标准 core 是 UI 的子进程，Rust 版的标准实例在 UI 进程内；所以对比口径是：

- **UI 侧** = app 进程 + 标准 core 子进程（Go）对比 app 进程（Rust）；
- **特权侧** = service 进程 + 增强 core 子进程（Go）对比 service 进程（Rust）。

每个场景取稳定后 60 s 内的中位数，三次测量。场景：空闲（已登录、未连接）、增强模式已连接空闲、
增强模式下载 100 Mbit/s 持续 60 s。

| 平台 | 场景 | UI 侧 | 特权侧 |
| --- | --- | --- | --- |
| Windows（虚拟机） | A 未连接空闲 | 263.0 WS / 162.4 Private | 9.3 / 2.6（只有 service） |
| Windows（虚拟机） | B 增强已连接空闲 | 260.2 / 161.7 | 66.7 / 82.1 |
| Windows（虚拟机） | C 增强下载约 91 Mbit/s | 260.0 / 161.8 | 69.3 / 85.1 |
| Linux | 待测 | | |
| macOS | 待测（真机） | | |

门槛：每一侧都不超过 Go 基线的 120%。

### Windows 明细

一台 Windows 11 虚拟机（企业评估版 build 26200，8 GB、4 个逻辑 CPU），桌面 0.2.90（build 4），
core 0.5.21，规则分流，单位 MB（1024²），
`WorkingSet64` / `PrivateMemorySize64`。每轮 60 s 内每 5 s 采一次，取各进程中位数后同侧相加；
三轮取中位数，各轮差异在 ±1.5 MB 以内。app 停在**概览页**（页面不同，ppvpn.exe 约有 ±5 MB 差异，
对比时用同一页）。场景 C 用 `curl --limit-rate 12207k` 从一个直连的公共镜像站下载（direct-host，
流量经增强 core）。

| 进程 | A | B | C |
| --- | --- | --- | --- |
| ppvpn.exe | 228.8 / 101.1 | 226.4 / 100.7 | 226.0 / 100.8 |
| 标准 core | 34.2 / 61.4 | 33.9 / 61.0 | 34.0 / 61.0 |
| ppvpn-service.exe | 9.3 / 2.6 | 10.1 / 3.8 | 10.1 / 3.8 |
| 增强 core | — | 56.6 / 78.3 | 59.2 / 81.3 |
| push-agent | 25.2 / 5.1 | 25.1 / 4.9 | 25.2 / 5.1 |
