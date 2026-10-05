# 构建与发布

## 分支与版本

- 只有 `main` 一个长期分支，与 PPVPN Desktop 一致；功能与修复分支都向 `main` 提 PR，经合并队列合入。
- Rust `ppvpn-core` 不单独发布二进制：Desktop 和 CLI 从本仓库的源码编译它（#214），桌面端的发行流程是
  `desktop-release.yml`。
- 原来的 Go 内核已从 main 移除。它的最后一版是 v0.5.21：tag `v0.5.21` 和同名 GitHub Release 的文件保留，
  切换前 Desktop 仍 vendor 这个版本（`desktop/vendor/ppvpn-core`），需要 Go 基线的验收（例如 G4、G6）
  也用这些 Release 文件，不再从源码构建。

## CI

测试按依赖分三层，什么时候跑哪一层见 [测试分层](testing.md)。工作流只有三个：

- `ci.yml`：全部测试。PR 上只在 Linux 跑 lint、L1、L2 和敏感信息检查；合并队列（以及手动触发、带
  `ci:full` 标签的 PR）再跑 L3 和 macOS、Windows；main 推送只为保存缓存。
- `desktop-release.yml`：桌面端发行，调用 `desktop-package.yml`（安装包）和 `desktop-pages.yml`（更新站点）。
  这两个也可以手动触发；`desktop-package.yml` 在合并队列里由 `ci.yml` 调用，检查改动了打包输入的变更。

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

- `ci.yml` 在合并队列里全部通过，L3 没有被跳过。
- 源码归档包含 `LICENSE`、`THIRD_PARTY_NOTICES.md` 和对应版本的依赖清单。
- 发行构建不含测试专用 feature（`tools/release-features-check.sh`）。
- Profile golden 变化已人工审阅，文档示例与公开 API（`docs/host-integration.md`）同步。
- 上层产品完成平台签名、公证/发布签名、权限声明和升级演练。
- 发布环境不包含 Profile、会话密钥、本地代理状态或开发日志。
