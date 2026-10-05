# PPVPN Desktop

PPVPN 的原生桌面客户端：macOS（SwiftUI）、Windows（WinUI 3）、Linux（GTK 4），
共用 Rust 客户端核心 `crates/ppvpn-client`（UniFFI 绑定）和特权服务 `service/`。
网络引擎是 Rust `ppvpn-core`（仓库根的 `crates/ppvpn-core`），都在进程内运行：标准模式在
应用进程里，增强模式（TUN）在特权服务进程里。

本目录里的路径和命令都以 `desktop/` 为根（脚本也是）。

- 构建：见 [BUILD.md](BUILD.md) 和各平台的 `apps/<platform>/README.md`。
- CI：见 [docs/ci.md](docs/ci.md)。安装包暂时还不从本仓库构建；发布流水线
  （GitHub Releases，tag `desktop-vX.Y.Z`）尚未迁入。
- 其他文档：见 [docs/README.md](docs/README.md)。
- 第三方组件：见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

## 日志位置（测试排障）

特权服务及其增强模式引擎的日志放在卸载、重装服务都不会删除的目录里；每个文件上限 5 MB，写满后滚动为 `<名称>.1.log`、`<名称>.2.log`，每种日志共保留 3 个文件。

| 平台 | 服务 / 增强模式引擎 | 应用（ppvpn-client、标准模式引擎、推送代理） |
|---|---|---|
| macOS | `/Library/Logs/PPVPN/`：`ppvpn-service.log`、`ppvpn-core.log`，以及 launchd 的 `ppvpn-service.out.log` / `ppvpn-service.err.log`（目录 0755、文件 0644，无需 root 即可读取） | `~/Library/Logs/PPVPN/` |
| Linux | `/var/log/ppvpn/`：`ppvpn-service.log`、`ppvpn-core.log`（需 root 读取） | `~/.local/state/ppvpn/logs/` |
| Windows | `%ProgramData%\PPVPN\logs\`：`ppvpn-service.log`、`ppvpn-core.log` | `%LOCALAPPDATA%\PPVPN\logs\` |

服务日志按 info 级别记录每次增强模式连接各阶段耗时（创建引擎、apply-profile、start、停止），应用日志记录客户端侧各步骤耗时（服务检查、冲突预检、Connect、健康检查的各步），慢连接可据此定位。

## 结构

```
desktop/
├── apps/
│   ├── macos/        # SwiftUI app、AppLogic（可在 Linux 上测试）、推送代理
│   ├── windows/      # WinUI 3 app、NSIS 安装包、推送代理
│   ├── linux/        # GTK 4 app、deb/rpm
│   └── shared/       # PPVPN.App.Core（Windows 与 Linux 共用的视图模型）、PPVPN.Client
├── crates/ppvpn-client/  # 共享客户端核心（账号、Profile、连接状态机、健康检查、UniFFI）
├── crates/engine-host/   # 进程内引擎上的 Core API v1（客户端与特权服务共用）
├── service/              # 特权服务：进程内运行增强模式引擎、client↔service IPC、系统 DNS
├── assets/icons/         # 品牌与托盘图标
├── scripts/              # build-service.sh、签名、图标与 CI 辅助脚本
└── docs/                 # CI、真机 QA 清单
```

与仓库其余部分的关系：

- `crates/ppvpn-client` 和 `crates/engine-host` 是仓库根 Cargo workspace 的成员，按路径依赖
  `crates/ppvpn-account` 和 `crates/ppvpn-core`；它们不在 workspace 的 default-members 里，
  要在各自目录下或用 `-p` 构建。
- `service/` 不在 workspace 里，有自己的 `Cargo.lock`。
- 工具链版本由仓库根的 `rust-toolchain.toml` 固定（1.98.1）。
- 测试在仓库根的 `.github/workflows/ci.yml`（`desktop-*` job），发行在 `desktop-release.yml`。

## 许可证

与仓库其余部分相同：GPL-3.0-or-later，见仓库根的 [LICENSE](../LICENSE)。
