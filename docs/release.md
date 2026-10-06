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
  `version.workspace = true` 继承，`Cargo.lock` 随之更新。三个平台工程的版本（合入 main 时安装包用的版本）
  与它保持一致，改版本时一起改：`apps/windows/Directory.Build.props` 的 `VersionPrefix`、
  `apps/macos/project.yml` 的 `MARKETING_VERSION`、`apps/linux/PPVPN.Linux.csproj` 和
  `apps/linux/PPVPN.PushAgent/PPVPN.PushAgent.csproj` 的 `Version`。
- 一个版本一个 tag `vX.Y.Z`、一个 GitHub Release：里面是三个平台的安装包和 CLI 的归档。发行时安装包的版本
  来自手动触发时填的版本（必须在 tag `v<版本>` 上运行）；CLI 的构建核对它等于 `Cargo.toml` 的版本，不等则
  发行失败。
- Go 内核的 Release 用的是同样的 tag 形式（v0.5.14 到 v0.5.21）。`release.yml` 拒绝 0.6.0 以下的 stable 版本，
  更新站点也只从 v0.6.0 及以后的 Release 构建，所以这些旧 Release 原样保留、不会被当成桌面端的发行。
  stable 发行成为仓库的 latest release（勾选 `allow_downgrade` 的回滚除外）。
- 每个版本的发行说明在 `docs/releases/<版本>.md`，例如 [0.6.0](releases/0.6.0.md)。

## CI

测试按依赖分三层，各个 job 和平台测试的约定见 [测试分层](testing.md)。只有两个 workflow：

- `ci.yml`：PR 推送时跑全部测试，7 个 job 并行（`lint`、`unit`、`integration`、`desktop`、`linux`、`macos`、`windows`）。
- `release.yml`：
  - 每次合入 main：三个平台的安装包（`linux`、`macos`、`windows`，不读任何密钥：Windows 不签名、macOS ad-hoc 签名、
    没有更新签名），rpm 在 dnf 下安装一次，apt/dnf 仓库自测（`linux-rpm-smoke`、`linux-repo`）；依赖变了时保存
    `ci.yml` 恢复的构建缓存（`cache-*`）。什么都不发布。
  - 手动触发：`channel=stable` 在 tag `vX.Y.Z` 上、经 `desktop-release-approval` 审批后，在
    `desktop-release-stable` 里构建并签名，`release-files` 整体核对，`publish` 发 GitHub Release（安装包和 CLI，
    SHA256SUMS 和构建 provenance），`site` 按 Release 重建更新站点，`site-deploy` 把它上传到 R2。
    `channel=dev` 在 main 上、`desktop-release-dev` 里构建，从不发布。`channel=site` 经同一审批，只按已发布的
    Release 重建并上传更新站点。
  - 构建号是 `DESKTOP_BUILD_OFFSET` 加 `release.yml` 的 run number，只增不减。

## 更新站点（R2）

更新站点 `https://pkg.peakpassvpn.com`（仓库变量 `DESKTOP_SITE_BASE`）是 Cloudflare R2 的一个 bucket，
不是 GitHub Pages（Pages 上限 1 GB，只 apt/dnf 仓库 5 个版本就约 600 MB）。内容：

- `desktop/channels/stable.json`（频道指针）和 `desktop/stable/appcast-<平台>.xml`（macOS、Windows 的 Sparkle 源）；
- `linux/`：apt 仓库（`linux/apt`，suite `stable`，保留最近 5 个版本的 deb）、dnf 仓库（`linux/rpm/stable`，
  rpm 本身从 GitHub Release 下载）、安装源设置包、公钥和 `linux/stable/latest.json`（Linux 应用的更新检查）；
- 已停用的 dev 频道：`linux/dev/latest.json` 和 `desktop/dev/appcast-<平台>.xml` 是 stable 的副本，让以前装的
  dev 构建收到 stable 的更新。dev 的 apt suite 和 `linux/rpm/dev` 不再写，bucket 里旧的原样留着。

每次都从 GitHub Releases 重建整个站点（`site`，带签名密钥），再由 `site-deploy`（只带 R2 的密钥）用 S3 API
（runner 自带的 AWS CLI，`tools/desktop/ci/upload-site.sh`）覆盖上传：先传不会变的文件（pool 里的包、by-hash
索引、repodata 数据文件、带版本的设置包），再传索引（`Packages*`）、签名的索引（`Release`/`InRelease`/
`repomd.xml`）、更新检查（`latest.json`、appcast），频道指针最后。不变的文件 `Cache-Control: public,
max-age=31536000, immutable`，其余 `no-cache`；bucket 里已有、内容不同的不变文件会被拒绝，整个上传不开始。
不删除任何对象：客户端已经拿到的索引所指的文件一直在，旧版本的包和旧的 by-hash 文件会累积，需要时手动清理。

需要在仓库里建的（名字固定）：

| 名称 | 类型 | 放在 | 内容 |
| --- | --- | --- | --- |
| `R2_ACCOUNT_ID` | 变量 | 环境 `desktop-release-stable` 或仓库 | Cloudflare 账号 ID（32 位十六进制） |
| `PPVPN_PKG_R2_BUCKET` | 变量 | 环境 `desktop-release-stable` 或仓库 | `pkg.peakpassvpn.com` 背后的 bucket 名 |
| `PPVPN_PKG_R2_ACCESS_KEY_ID` | secret | 环境 `desktop-release-stable` | 只对该 bucket 有读写权限的 R2 API token |
| `PPVPN_PKG_R2_SECRET_ACCESS_KEY` | secret | 环境 `desktop-release-stable` | 同上 |

`DESKTOP_SITE_BASE`（仓库变量，`https://pkg.peakpassvpn.com`）保持不变。不再需要 GitHub Pages 和
`github-pages` 环境。

演练（`rehearsal`，在 main 上的 stable 运行）照常构建、签名、出草稿 Release，按本次的文件构建站点，作为
artifact `site` 保留三天，并在日志里打出上传的顺序和缓存设置（`upload-site.sh --plan`）。审批之后，
`site-check` 在 `desktop-release-stable` 里带着 R2 的密钥只读地核对一遍（`upload-site.sh --check`）：
列出桶里 `linux/` 和 `desktop/` 下的对象，像上传那样比对不可变文件（内容不同就失败），并列出真正上传时
会覆盖的现有文件，结果写在运行摘要里。演练不上传任何东西；密钥的写权限在第一次正式发布时才用到。

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
