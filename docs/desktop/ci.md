# CI

仓库只有两个 workflow：`.github/workflows/ci.yml` 在每个 PR 推送时跑全部测试，`.github/workflows/release.yml`
在合入 main 时构建安装包、手动触发时发行。测试分层、全部 job 和平台测试的约定见
[仓库的测试分层](../testing.md)。

## 桌面端在 `ci.yml` 里

7 个 job 都在每个 PR 上并行运行（只改文档的 PR 跳过构建和测试步骤）。和桌面端有关的是：

| Job | 运行环境 | 桌面端的内容 |
| --- | --- | --- |
| `desktop` | ubuntu-latest | 桌面各层，客户端 crate 只编译一次（`CARGO_BUILD_TARGET` 固定目标目录）：`crates/service`（进程内托管 TUN 引擎）的 `cargo test` 和 clippy（`-D warnings`）；`ppvpn-client` 的测试和 clippy（链接 Rust `ppvpn-core`、sail 和预编译 BoringSSL）；`bash -n` 和 `tools/desktop/ci` 的 Python 测试；用 `uniffi-bindgen-cs` 生成 C# 绑定，跑 `PPVPN.App.Core.Tests` 和 `PPVPN.Linux.Tests`（.NET 8，不需要 GTK 运行时）；Swift 绑定（`apps/macos/scripts/build-swift-linux.sh`，Swift 用 swift.org 的工具链并校验签名）和 macOS 的 `AppLogic` 测试（不含 AppKit/SwiftUI） |
| `linux` | ubuntu-latest | 增强模式端到端：以 root 运行特权 service，客户端 crate 经它连接和断开（`src/e2e_linux.rs`，feature `linux-e2e`），本地假后端和 Shadowsocks 节点（`test/fakenode`）；检查 TUN、策略路由、经节点的流量，以及断开后和 kill -9 后的清理 |
| `macos` | macos-15 | 客户端和 service 在 aarch64、x86_64 上的 clippy；`ppvpn-client` 的平台测试（`detect`、`service`、`sysproxy`）；`bootstrap.sh` 之后用 `xcodebuild` 构建 Debug app（ad-hoc 签名，不嵌入 core 和 service） |
| `windows` | windows-latest | 客户端和 service 在 MSVC 上的 clippy；`ppvpn-client` 的平台测试；生成 C# 绑定；`PPVPN.App.Core.Tests`；app 的 Release 构建（`-warnaserror`）；推送代理（NativeAOT）。不打安装包，不签名 |

`crates/desktop` 和 `crates/service` 是 workspace 成员但不在 default-members 里，所以由这些 job
而不是 `lint`、`unit` 检查。

工具链版本固定为 1.98.1，与 `rust-toolchain.toml` 保持一致。PR 只读 Rust 缓存；缓存由 `release.yml` 在合入 main、
依赖变了时保存（`cache-desktop`、`cache-linux`、`cache-macos`、`cache-windows`）。

`uniffi-bindgen-cs` 的版本（`v0.11.0+v0.31.0`）在工作流的 `UNIFFI_BINDGEN_CS_TAG` 和
`crates/desktop/scripts/build-dotnet.sh` 里各写了一次，要一起改。

## 打包与发行（`release.yml`）

- 每次合入 main：`linux`（deb 和 rpm）、`macos`（每个架构一个 DMG）、`windows`（NSIS）构建安装包，不读任何密钥
  （Windows 不签名、macOS ad-hoc 签名、没有更新签名），作为 workflow artifact；`linux-rpm-smoke` 在 dnf 下安装一次，
  `linux-repo` 用这些包自测 apt/dnf 仓库。什么都不发布。
- 手动触发 `channel=stable`：在 tag `vX.Y.Z`（0.6.0 及以后；桌面端、CLI 和引擎同一个版本、同一个 Release）上，
  经 `desktop-release-approval` 审批，在 `desktop-release-stable` 里构建并签名，`release-files` 整体核对，
  `publish` 发 GitHub Release，`site` 重建更新站点，`site-deploy` 上传到 R2（`https://pkg.peakpassvpn.com`）。
  `rehearsal` 只出草稿 Release 和站点 artifact `site`，不上传。
- 手动触发 `channel=dev`：在 main 上、`desktop-release-dev` 里构建，从不发布。
- 手动触发 `channel=site`：经同一审批，按已发布的 Release 重建并上传更新站点。

更新站点的内容、上传顺序、缓存设置和需要建的变量与 secret 见 [构建与发布](../release.md#更新站点r2)。
