# 构建与发布

## 分支与版本

- 只有 `main` 一个长期分支，与 PPVPN Desktop 一致；功能与修复分支都向 `main` 提 PR，`ci.yml` 通过后合入。
- Rust `ppvpn-core` 不单独发布二进制：Desktop 和 CLI 从本仓库的源码编译它（#214），桌面端的发行流程在
  `release.yml`。
- 原来的 Go 内核已从 main 移除。它的最后一版是 v0.5.21：tag `v0.5.21` 和同名 GitHub Release 的文件保留，
  Desktop 已改用 Rust 引擎，不再 vendor 它；需要 Go 基线的验收（例如 G4、G6）用这些 Release 文件，
  不再从源码构建。
- 版本号接着 Go 版往上走：Rust 的第一个对外版本是 **0.6.0**（Go 的最后一版是 0.5.21）。引擎、账号、CLI、
  桌面客户端和特权服务用同一个版本，写在根 `Cargo.toml` 的 `[workspace.package] version`，各 crate 用
  `version.workspace = true` 继承；改版本只改这一处，`Cargo.lock` 随之更新。桌面安装包的版本仍来自
  tag `desktop-vX.Y.Z`，发行时应与这里一致（目前没有自动检查）。

## CI

测试按依赖分三层，各个 job 和平台测试的约定见 [测试分层](testing.md)。只有两个 workflow：

- `ci.yml`：PR 推送时跑全部测试，7 个 job 并行（`lint`、`unit`、`integration`、`desktop`、`linux`、`macos`、`windows`）。
- `release.yml`：
  - 每次合入 main：三个平台的安装包（`linux`、`macos`、`windows`，不读任何密钥：Windows 不签名、macOS ad-hoc 签名、
    没有更新签名），rpm 在 dnf 下安装一次，apt/dnf 仓库自测（`linux-rpm-smoke`、`linux-repo`）；依赖变了时保存
    `ci.yml` 恢复的构建缓存（`cache-*`）。什么都不发布。
  - 手动触发：`channel=stable` 在 tag `desktop-vX.Y.Z` 上、经 `desktop-release-approval` 审批后，在
    `desktop-release-stable` 里构建并签名，`release-files` 整体核对，`publish` 发 GitHub Release（SHA256SUMS 和构建
    provenance），`pages` / `pages-deploy` 重建并部署更新站点；`rehearsal` 只出草稿和站点 artifact。
    `channel=dev` 在 main 上、`desktop-release-dev` 里构建，从不发布。`channel=site` 经同一审批，只按已发布的
    Release 重建并部署更新站点。
  - 构建号是 `DESKTOP_BUILD_OFFSET` 加 `release.yml` 的 run number，只增不减。

## v0.5.21 的发布文件

v0.5.21 的文件由当时的 `release.yml` 在 tag 上构建，带 GitHub artifact attestation。下载后校验：

```sh
sha256sum -c --ignore-missing SHA256SUMS   # macOS: shasum -a 256 -c --ignore-missing SHA256SUMS
gh attestation verify ppvpn-core-linux-amd64 \
  --repo peakpassvpn/ppvpn-core \
  --signer-workflow peakpassvpn/ppvpn-core/.github/workflows/release.yml \
  --source-ref refs/tags/v0.5.21
```

`--ignore-missing` 允许只下载部分文件。Windows 没有 `sha256sum`，用 `Get-FileHash -Algorithm SHA256 <文件>` 与 `SHA256SUMS` 中的值比对。
只带 `--repo` 时，attestation 只证明文件由本仓库的某个工作流构建；加上 `--signer-workflow` 和
`--source-ref` 才把它限定为 `release.yml` 在该 tag 上的构建。

Go 内核的构建、移动端产物和 Go 时代的发布流程见 tag `v0.5.21` 上的本文件。

## 固定工具链

- Rust：`rust-toolchain.toml`（与 Sail 的 CI 和发行一致）；改它时同时改 `ci.yml` 里的版本。
- Sail：`Cargo.toml` 按提交固定（`[workspace.dependencies]`），`[patch.crates-io]` 与该提交的 Sail 保持一致，
  CI 的 lint job 会核对。
- BoringSSL：用 btls 为所固定提交发布的预编译库（校验 `SHA256SUMS` 和构建 attestation），不在 CI 里编译。
- 测试工具里的 Go（`test/go.mod`）：只用于测试时启动的辅助程序（假节点、`ldnslab` 等），不进入任何发行物。

## 发布检查表（Desktop 和 CLI 的发行）

- 发行提交的 PR 上 `ci.yml` 全部通过，没有只改文档而跳过的步骤。
- 源码归档包含 `LICENSE`、`THIRD_PARTY_NOTICES.md` 和对应版本的依赖清单。
- 发行构建不含测试专用 feature（`tools/release-features-check.sh`）。
- Profile golden 变化已人工审阅，文档示例与公开 API（`docs/host-integration.md`）同步。
- 上层产品完成平台签名、公证/发布签名、权限声明和升级演练。
- 发布环境不包含 Profile、会话密钥、本地代理状态或开发日志。
