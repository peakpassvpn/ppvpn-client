# 构建

命令都在 `desktop/` 下执行，路径相对 `desktop/`。各平台的完整步骤在
`apps/macos/README.md`、`apps/windows/README.md`、`apps/linux/README.md`。

工具链：Rust 1.98.1（仓库根的 `rust-toolchain.toml`）；C# 部分用 .NET 8 SDK 和
`uniffi-bindgen-cs` v0.11.0+v0.31.0（见 `apps/shared/PPVPN.Client/README.md`）；macOS 用
Xcode、XcodeGen 和 Swift 6.4。

## 共用的前置步骤

```bash
# Go core 的二进制不入库：按 vendor/ppvpn-core/<CURRENT>/manifest.json 从 ppvpn-core 的
# GitHub Release 下载，并校验大小和 SHA-256。需要 curl、jq、node。
scripts/fetch-vendored-core.sh                 # 全部平台
scripts/fetch-vendored-core.sh linux-x86_64    # 只取一个（manifest 里的 key）

# 客户端 crate（仓库根 workspace 的成员，默认经 IPC 驱动 Go core）
(cd crates/ppvpn-client && cargo test --locked)
# 链接 Rust ppvpn-core 的构建（feature rust-core，默认关闭）
(cd crates/ppvpn-client && cargo test --locked --features rust-core --lib rust_core)

# 特权服务（不在 workspace 里，有自己的 Cargo.lock）
(cd service && cargo build --release --locked --bins)
```

下载的二进制在 `vendor/ppvpn-core/<版本>/build/`（已被 git 忽略），已存在且校验通过的文件不会重新下载。
`scripts/verify-vendored-core.mjs` 可以单独校验某一个：

```bash
node scripts/verify-vendored-core.mjs --vendor-dir vendor/ppvpn-core/0.5.21 \
  --artifact linux-x86_64 --expected-version 0.5.21
```

## 各平台

- **macOS**：`apps/macos/scripts/bootstrap.sh` 生成 `PPVPNClient` Swift 包和 `PPVPN.xcodeproj`，
  之后用 Xcode 或 `xcodebuild` 构建 Debug。Debug 构建不需要嵌入 core 和 service（缺少时只给出警告）。
- **Linux**：`crates/ppvpn-client/scripts/build-dotnet.sh x86_64-unknown-linux-gnu` 生成 C# 绑定和
  原生库，之后 `dotnet run --project apps/linux`。deb/rpm 由 `apps/linux/scripts/build-package.sh`
  在 Ubuntu 22.04（glibc 2.35）基线上构建，它自己构建 service 并校验 vendored core。
- **Windows**：只在 Windows 上构建（`crates\ppvpn-client\scripts\build-dotnet.ps1`，MSVC 工具链），
  见 `apps/windows/README.md`。

## 嵌入 app 的二进制

```bash
# macOS：把 vendored core 的两个架构合成 universal，放到 build/binaries/
scripts/stage-macos-core.sh
# 构建特权服务（macos / windows），产物同样放到 build/binaries/
scripts/build-service.sh macos
```

macOS 的 Release 构建和 `apps/macos/scripts/package-dmg.sh` 需要这些产物。

升级 vendored core 用 `scripts/vendor-core-release.sh X.Y.Z`（Go core 已冻结在 0.5.21，目前用不到）：
它只提交 `CURRENT` 和 manifest，二进制留在被忽略的 `build/` 目录里。

## 尚未迁入的部分

安装包的打包与发布工作流尚未迁入：安装包暂时不从本仓库构建。每次提交的 CI 见
[docs/ci.md](docs/ci.md)。
