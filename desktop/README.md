# PPVPN Desktop

PPVPN 的原生桌面客户端：macOS（SwiftUI）、Windows（WinUI 3）、Linux（GTK 4），
共用 Rust 客户端核心 `crates/ppvpn-client`（UniFFI 绑定）和特权服务 `service/`。
网络引擎目前是 Go 版 `ppvpn-core`（冻结在 0.5.21），以独立进程运行、经 IPC 调用；
切换到进程内的 Rust `ppvpn-core`（仓库根的 `crates/ppvpn-core`）见
[#214](https://github.com/peakpassvpn/ppvpn-core/issues/45)，在 `ppvpn-client` 里是默认关闭的
feature `rust-core`。

本目录里的路径和命令都以 `desktop/` 为根（脚本也是）。

- 构建：见 [BUILD.md](BUILD.md) 和各平台的 `apps/<platform>/README.md`。
- CI：见 [docs/ci.md](docs/ci.md)。安装包暂时还不从本仓库构建；发布流水线
  （GitHub Releases，tag `desktop-vX.Y.Z`）尚未迁入。
- 其他文档：见 [docs/README.md](docs/README.md)。
- 第三方组件：见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 日志位置（测试排障）

特权服务及其启动的增强模式 core 的日志放在卸载、重装服务都不会删除的目录里；每个文件上限 5 MB，写满后滚动为 `<名称>.1.log`、`<名称>.2.log`，每种日志共保留 3 个文件。

| 平台 | 服务 / 增强模式 core | 应用（ppvpn-client、标准模式 core、推送代理） |
|---|---|---|
| macOS | `/Library/Logs/PPVPN/`：`ppvpn-service.log`、`ppvpn-core.log`，以及 launchd 的 `ppvpn-service.out.log` / `ppvpn-service.err.log`（目录 0755、文件 0644，无需 root 即可读取） | `~/Library/Logs/PPVPN/` |
| Linux | `/var/log/ppvpn/`：`ppvpn-service.log`、`ppvpn-core.log`（需 root 读取） | `~/.local/state/ppvpn/logs/` |
| Windows | `%ProgramData%\PPVPN\logs\`：`ppvpn-service.log`、`ppvpn-core.log` | `%LOCALAPPDATA%\PPVPN\logs\` |

服务日志按 info 级别记录每次增强模式连接各阶段耗时（core 启动到就绪、get-version、apply-profile、start、停止），应用日志记录客户端侧各步骤耗时（服务检查、冲突预检、Connect、健康检查的各步），慢连接可据此定位。

## 结构

```
desktop/
├── apps/
│   ├── macos/        # SwiftUI app、AppLogic（可在 Linux 上测试）、推送代理
│   ├── windows/      # WinUI 3 app、NSIS 安装包、推送代理
│   ├── linux/        # GTK 4 app、deb/rpm
│   └── shared/       # PPVPN.App.Core（Windows 与 Linux 共用的视图模型）、PPVPN.Client
├── crates/ppvpn-client/  # 共享客户端核心（账号、Profile、连接状态机、健康检查、UniFFI）
├── service/              # 特权服务：托管增强模式 core、client↔service IPC、系统 DNS
├── vendor/ppvpn-core/    # Go core 的 CURRENT 与 manifest；二进制不入库，由脚本下载
├── assets/icons/         # 品牌与托盘图标
├── scripts/              # fetch-vendored-core.sh、verify-vendored-core.mjs
└── docs/                 # CI、Go 基线、真机 QA 清单
```

与仓库其余部分的关系：

- `crates/ppvpn-client` 是仓库根 Cargo workspace 的成员，按路径依赖 `crates/ppvpn-account`
  和（feature `rust-core` 打开时）`crates/ppvpn-core`；它不在 workspace 的 default-members 里，
  要在它自己的目录下或用 `-p ppvpn-client` 构建。
- `service/` 不在 workspace 里，有自己的 `Cargo.lock`。
- 工具链版本由仓库根的 `rust-toolchain.toml` 固定（1.98.1）。
- 工作流在仓库根的 `.github/workflows/desktop.yml`。

## 许可证

与仓库其余部分相同：GPL-3.0-or-later，见仓库根的 [LICENSE](../LICENSE)。
