# CI

桌面端的 job 在仓库根的 `.github/workflows/ci.yml` 里（名字以 `desktop-` 开头），测试分层和运行时机见
[仓库的测试分层](../../docs/testing.md)。这些 job 只做测试和 Debug 构建；安装包由 `desktop-package.yml`
构建，发行由 `desktop-release.yml` 负责。

## 何时运行

`tools/ci-changes.sh` 判断变更涉及哪些部分，不相关的 job 用 job 级的 `if` 跳过：

- pull request：只跑 Linux 上的 job（`desktop-rust`、`desktop-client`、`desktop-swift`），
  而且只在改动了 `desktop/` 或桌面端工作流时；
- 合并队列、手动触发、带 `ci:full` 标签的 PR：再跑 macOS 和 Windows 上的 job 和 `desktop-linux-enhanced`；
  改动了打包输入时，调用 `desktop-package.yml` 构建不签名的安装包；
- main 推送：只在 `Cargo.toml`、`Cargo.lock` 或工具链变了时跑一次，用来更新缓存。

## 与引擎 job 的分工

- `lint`、`test`、`windows-msvc`、`macos` 等覆盖 workspace 的 default-members（`crates/` 下的 `ppvpn-core`、
  `ppvpn-cli`、`ppvpn-account`）；它们是 `main` 的必需检查。
- 桌面端的 job 都**不是**必需检查；一旦运行，和其他检查一样要全部通过才合并。
- `desktop/crates/ppvpn-client` 是 workspace 成员但不在 default-members 里，所以由桌面端的 job
  而不是引擎的 job 检查。

工具链版本固定为 1.98.1，与 `rust-toolchain.toml` 保持一致。Rust 缓存只在 `main` 上保存，
pull request 和合并队列只读取。

## Job

| Job | 运行环境 | 内容 |
| --- | --- | --- |
| `desktop-rust` | ubuntu-latest | `desktop/service`（进程内托管 TUN 引擎） 的 `cargo test` 和 clippy（`-D warnings`）；`bash -n` 检查 `desktop/scripts/*.sh` |
| `desktop-client` | ubuntu-latest | `ppvpn-client` 的测试和 clippy（链接 Rust `ppvpn-core`，sail 和预编译 BoringSSL）；用 `uniffi-bindgen-cs` 生成 C# 绑定；`PPVPN.App.Core.Tests` 和 `PPVPN.Linux.Tests`（.NET 8，不需要 GTK 运行时） |
| `desktop-swift` | ubuntu-latest，容器 `swift:6.4.0-noble` | 构建客户端 crate 的 Linux 版和 Swift 绑定（`apps/macos/scripts/build-swift-linux.sh`），测试 macOS 的 `AppLogic`（不含 AppKit/SwiftUI） |
| `desktop-macos` | macos-15 | `ppvpn-client` 在 macOS 上的测试；`bootstrap.sh` 之后用 `xcodebuild` 构建 Debug app（ad-hoc 签名，不嵌入 core 和 service，Debug 构建不需要） |
| `desktop-windows` | windows-latest | `ppvpn-client` 在 Windows（MSVC）上的测试；生成 C# 绑定；`PPVPN.App.Core.Tests`；app 的 Release 构建（`-warnaserror`）；推送代理（NativeAOT）。不打安装包，不签名 |

`uniffi-bindgen-cs` 的版本（`v0.11.0+v0.31.0`）在工作流的 `UNIFFI_BINDGEN_CS_TAG` 和
`desktop/crates/ppvpn-client/scripts/build-dotnet.sh` 里各写了一次，要一起改。


## 打包与发行

安装包（Windows NSIS、macOS dmg、Linux deb/rpm）由 `desktop-package.yml` 构建；发行（GitHub Releases，tag
`desktop-vX.Y.Z`）由 `desktop-release.yml` 负责，它调用 `desktop-package.yml` 和 `desktop-pages.yml`。
