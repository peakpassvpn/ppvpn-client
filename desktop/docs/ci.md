# CI

桌面端的工作流是仓库根的 `.github/workflows/desktop.yml`。它只做测试和 Debug 构建；
安装包的打包与发布工作流尚未迁入本仓库（见文末）。

## 何时运行

pull request 和推送到 `main` 时，改动涉及以下路径才运行：

- `desktop/**`
- `crates/**`（客户端 crate 按路径链接的 workspace crate）
- `Cargo.toml`、`Cargo.lock`、`rust-toolchain.toml`
- `.github/workflows/desktop.yml`

## 与 rust.yml 的分工

- `rust.yml` 在每个 pull request 和每次推送到 `main` 时运行，覆盖 workspace 的 default-members
  （`crates/` 下的 `ppvpn-core`、`ppvpn-cli`、`ppvpn-account`）：format、clippy、Linux / Windows（MSVC）/
  macOS 上的测试，以及 `[patch.crates-io]` 和 `Cargo.lock` 与 sail 的一致性检查。它的 job 是 `main`
  的必需检查。
- `desktop.yml` 的 job 都**不是**必需检查：被路径过滤跳过的 pull request 不会因此卡住；
  一旦运行，和其他检查一样要全部通过才合并。
- `desktop/crates/ppvpn-client` 是 workspace 成员但不在 default-members 里，所以由 `desktop.yml`
  而不是 `rust.yml` 检查。`desktop/service` 不在 workspace 里，用自己的 `Cargo.lock`。

工具链版本固定为 1.98.1，与 `rust-toolchain.toml` 保持一致。Rust 缓存只在 `main` 上保存，
pull request 只读取。

## Job

| Job | 运行环境 | 内容 |
| --- | --- | --- |
| `desktop-rust` | ubuntu-latest | 下载并校验 vendored core；`desktop/service` 的 `cargo test` 和 clippy（`-D warnings`）；`bash -n` 检查 `desktop/scripts/*.sh` |
| `desktop-client` | ubuntu-latest | `ppvpn-client` 的测试和 clippy（应用实际链接的形态：经 IPC 驱动 Go core）；用 `uniffi-bindgen-cs` 生成 C# 绑定；`PPVPN.App.Core.Tests` 和 `PPVPN.Linux.Tests`（.NET 8，不需要 GTK 运行时） |
| `desktop-rust-core` | ubuntu-latest | `ppvpn-client` 打开 feature `rust-core`（链接 Rust `ppvpn-core`）的 clippy 和 `rust_core` 测试。只在 Linux 上；Windows 与 macOS 上的引擎构建由 `rust.yml` 覆盖。sail 和 BoringSSL 从源码构建，冷缓存时较慢 |
| `desktop-swift` | ubuntu-latest，容器 `swift:6.4.0-noble` | 构建客户端 crate 的 Linux 版和 Swift 绑定（`apps/macos/scripts/build-swift-linux.sh`），测试 macOS 的 `AppLogic`（不含 AppKit/SwiftUI） |
| `desktop-macos` | macos-15 | `ppvpn-client` 在 macOS 上的测试；`bootstrap.sh` 之后用 `xcodebuild` 构建 Debug app（ad-hoc 签名，不嵌入 core 和 service，Debug 构建不需要） |
| `desktop-windows` | windows-latest | `ppvpn-client` 在 Windows（MSVC）上的测试；生成 C# 绑定；`PPVPN.App.Core.Tests`；app 的 Release 构建（`-warnaserror`）；推送代理（NativeAOT）。不打安装包，不签名 |

`uniffi-bindgen-cs` 的版本（`v0.11.0+v0.31.0`）在工作流的 `UNIFFI_BINDGEN_CS_TAG` 和
`desktop/crates/ppvpn-client/scripts/build-dotnet.sh` 里各写了一次，要一起改。

## Vendored core

Go core 的二进制不入库。`desktop/vendor/ppvpn-core/` 里只有：

- `CURRENT`：当前版本号（`0.5.21`）；
- `<版本>/manifest.json`：来源（仓库 `peakpassvpn/ppvpn-core`、完整 commit、tag、Release 地址）和每个制品的
  路径、大小、SHA-256。制品 key：`windows-x86_64`、`macos-cli-arm64`、`macos-cli-x86_64`、`linux-x86_64`。

`desktop/scripts/fetch-vendored-core.sh [ARTIFACT...]`：

1. 检查 manifest 的仓库是 `peakpassvpn/ppvpn-core`、tag 是 `v<CURRENT>`；
2. 对每个制品，从该 tag 的 GitHub Release 下载到 `vendor/ppvpn-core/<版本>/build/`（已被 git 忽略）；
   已存在且校验通过的文件保留不动；
3. 用 `desktop/scripts/verify-vendored-core.mjs` 按 manifest 校验大小和 SHA-256，不符即失败。

`desktop-rust` 在每次运行时执行这一步。Linux 的打包脚本（`apps/linux/scripts/build-package.sh`）在
打包前也会校验 vendored core。

## 尚未迁入

打包与发布的工作流还没有迁入：安装包（Windows NSIS、macOS dmg、Linux deb/rpm）暂时不从本仓库构建。
发布流水线（GitHub Releases，tag `desktop-vX.Y.Z`）之后加入，届时在这里补充。
