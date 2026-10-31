# 构建

命令都在仓库根执行，路径相对仓库根。完整步骤在 `apps/flutter/README.md`（三个平台的 Flutter
桌面应用和安装包）和 `apps/macos/README.md`（macOS 的推送代理和打包）。

工具链：Rust 1.98.1（仓库根的 `rust-toolchain.toml`）；Flutter（`apps/flutter/scripts/install-flutter.sh`）；
macOS 用 Xcode、XcodeGen 和 Swift 6.4。

## 共用的前置步骤

引擎是 Rust `ppvpn-core`，客户端和特权服务都在进程内链接它（经 `crates/engine-host`），没有单独的
core 二进制。

```bash
# 客户端 crate（仓库根 workspace 的成员，在进程内链接 Rust ppvpn-core 和 sail）
(cd crates/desktop && cargo test --locked)

# 特权服务（workspace 成员，增强模式的引擎在它的进程里）
cargo build --locked -p ppvpn-service --profile service --bins
```

## 各平台

- 三个平台的桌面应用都是 Flutter 版（`apps/flutter`），`apps/flutter/scripts/ci-build.sh` 构建。
- **macOS**：`apps/flutter/scripts/package-macos.sh` 打包，它用 `apps/macos/scripts/bootstrap.sh`
  生成 `PPVPNClient` Swift 包和 `PPVPN.xcodeproj`，构建推送代理（`PPVPNAgent`）。
- **Linux、Windows**：deb/rpm 由 `apps/flutter/scripts/package-linux.sh` 打包，Windows 安装包由
  `apps/flutter/scripts/package-windows.ps1` 打包（MSVC 工具链），两者都自己构建 service。Windows
  安装包里另有 `wintun.dll`（增强模式的 TUN 驱动，`package-windows.ps1` 下载并按哈希校验）。

## 嵌入 app 的二进制

```bash
# 构建特权服务（macos / windows），产物放到 build/binaries/
tools/desktop/build-service.sh macos
```

macOS 的打包（`apps/flutter/scripts/package-macos.sh`）需要这些产物。

## 尚未迁入的部分

安装包的打包与发布工作流尚未迁入：安装包暂时不从本仓库构建。每次提交的 CI 见
[docs/desktop/ci.md](ci.md)。
