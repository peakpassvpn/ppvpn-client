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
- `desktop/crates/ppvpn-client`、`desktop/crates/engine-host` 和 `desktop/service` 是 workspace 成员但不在
  default-members 里，所以由 `desktop.yml` 而不是 `rust.yml` 检查。

工具链版本固定为 1.98.1，与 `rust-toolchain.toml` 保持一致。Rust 缓存只在 `main` 上保存，
pull request 只读取。

## Job

| Job | 运行环境 | 内容 |
| --- | --- | --- |
| `desktop-rust` | ubuntu-latest | `desktop/service` 的 `cargo test` 和 clippy（`-D warnings`）；`bash -n` 检查 `desktop/scripts/*.sh` |
| `desktop-client` | ubuntu-latest | `ppvpn-client` 的测试和 clippy（进程内链接 Rust `ppvpn-core`）；用 `uniffi-bindgen-cs` 生成 C# 绑定；`PPVPN.App.Core.Tests` 和 `PPVPN.Linux.Tests`（.NET 8，不需要 GTK 运行时） |
| `desktop-swift` | ubuntu-latest，容器 `swift:6.4.0-noble` | 构建客户端 crate 的 Linux 版和 Swift 绑定（`apps/macos/scripts/build-swift-linux.sh`），测试 macOS 的 `AppLogic`（不含 AppKit/SwiftUI） |
| `desktop-macos` | macos-15 | `ppvpn-client` 在 macOS 上的测试；`bootstrap.sh` 之后用 `xcodebuild` 构建 Debug app（ad-hoc 签名，不嵌入 service，Debug 构建不需要） |
| `desktop-windows` | windows-latest | `ppvpn-client` 在 Windows（MSVC）上的测试；生成 C# 绑定；`PPVPN.App.Core.Tests`；app 的 Release 构建（`-warnaserror`）；推送代理（NativeAOT）。不打安装包，不签名 |

`uniffi-bindgen-cs` 的版本（`v0.11.0+v0.31.0`）在工作流的 `UNIFFI_BINDGEN_CS_TAG` 和
`desktop/crates/ppvpn-client/scripts/build-dotnet.sh` 里各写了一次，要一起改。

## 尚未迁入

打包与发布的工作流还没有迁入：安装包（Windows NSIS、macOS dmg、Linux deb/rpm）暂时不从本仓库构建。
发布流水线（GitHub Releases，tag `desktop-vX.Y.Z`）之后加入，届时在这里补充。
