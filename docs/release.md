# 构建与发布

## 分支与版本

- 只有 `main` 一个长期分支，与 PPVPN Desktop 一致；功能与修复分支都向 `main` 提 PR。
- 发版：在 PR 中只更新 `version/version.go` 的 `CoreVersion`（版本号的唯一来源；文档示例写 `X.Y.Z`，
  不随发版修改），合入后在该合并提交上打注解 tag `vX.Y.Z`。
- Desktop 的 `vendor/ppvpn-core/<ver>/manifest.json` 以 `source.commit` 固定完整 SHA，
  该 SHA 必须就是 `vX.Y.Z` 指向的提交。

## CI 分层与 GitHub Release

- `ci.yml`（每个 PR、每次推送 `main`）：`go vet`（本机与 `GOOS=windows`）和三组构建标签下的 `go test`。
- `release.yml`（`v*` tag；改动 `version/version.go` 或该工作流的 PR；手动触发）：上面全部，加
  `-race`、`govulncheck`、Windows 原生测试，以及全部平台产物。发版 PR 改了版本号，所以完整门禁在合并前
  就已跑过，tag 不会是第一次遇到它。
- `release-files` job 把各平台文件汇总成一组发布文件：二进制、`SHA256SUMS`（`sha256sum` 格式，不带目录）
  和 `build-info.json`（`tag`、`commit`、`core_version`、`go_version`、`desktop_tags`、`mobile_tags`）。
  PR 上也会运行并作为 artifact 上传。
- 在 tag 上，`release-files` 还要求 tag 等于 `v<core_version>`，否则失败，后面不会发布。
- `publish` job 只在 tag 上运行：对文件做 GitHub artifact attestation 并创建同名 GitHub Release
  （0.5.14 起）。桌面端使用其中的
  `ppvpn-core-darwin-arm64`（Apple Silicon）、`ppvpn-core-darwin-amd64`（Intel）、`ppvpn-core-windows-amd64.exe`
  和 `ppvpn-core-linux-amd64`（0.5.15 起 macOS 按架构分两个文件，不再提供 `ppvpn-core-darwin-universal`），
  均以 `DESKTOP_TAGS` 构建。

下载后校验：

```sh
sha256sum -c --ignore-missing SHA256SUMS   # macOS: shasum -a 256 -c --ignore-missing SHA256SUMS
gh attestation verify ppvpn-core-linux-amd64 \
  --repo peakpassvpn/ppvpn-core \
  --signer-workflow peakpassvpn/ppvpn-core/.github/workflows/release.yml \
  --source-ref refs/tags/vX.Y.Z
```

`--ignore-missing` 允许只下载部分文件。Windows 没有 `sha256sum`，用 `Get-FileHash -Algorithm SHA256 <文件>` 与 `SHA256SUMS` 中的值比对。

只带 `--repo` 时，attestation 只证明文件由本仓库的某个工作流构建；加上 `--signer-workflow` 和
`--source-ref` 才把它限定为 `release.yml` 在该 tag 上的构建。手动上传到 Release 的文件无法通过校验。

`-race` 偶尔会报上游依赖内部的竞态，发生在测试里充当节点的 Shadowsocks 服务端
（`sing-shadowsocks` 的 `serverConn.writeResponse` 对 `serverConn.Close`）。只有两侧堆栈都是这两个
函数时才重跑，其他竞态一律当作真实问题处理。

## 固定工具链

- Go：以 `go.mod` 的 `go` 指令为准（当前 1.26.8）。
- sing-box：`v1.13.12`，必须保持直接依赖固定版本。
- gomobile/gobind：Makefile 的 `MOBILE_VERSION`，禁止 CI 使用 `@latest`。
- Android：NDK `29.0.14206865`，最低 API 23。

依赖升级属于兼容性变更。升级 sing-box 或 gomobile 后必须重跑完整 Profile golden、真实运行时、本地代理认证和移动绑定检查。

## 测试门禁

```sh
go test ./...
go test -race ./...
go vet ./...
go run golang.org/x/vuln/cmd/govulncheck@v1.7.0 ./...
```

覆盖重点：Profile 严格解析及安全地址校验、三种协议构建、SS 2022 EIH、golden 配置、入口/可用性探测、热更新回滚、真实 sing-box 启动、每节点 HTTP/SOCKS5 认证、遥测、IPC 权限、API 认证与脱敏、移动端秘密不泄露。

## 桌面构建

当前交付矩阵：

```sh
mkdir -p build
GOOS=darwin GOARCH=arm64 go build -trimpath -o build/ppvpn-core-darwin-arm64 ./cmd/ppvpn-core
GOOS=darwin GOARCH=amd64 go build -trimpath -o build/ppvpn-core-darwin-amd64 ./cmd/ppvpn-core
GOOS=windows GOARCH=amd64 go build -trimpath -o build/ppvpn-core-windows-amd64.exe ./cmd/ppvpn-core
GOOS=windows GOARCH=arm64 go build -trimpath -o build/ppvpn-core-windows-arm64.exe ./cmd/ppvpn-core
GOOS=linux GOARCH=amd64 go build -trimpath -o build/ppvpn-core-linux-amd64 ./cmd/ppvpn-core
GOOS=linux GOARCH=arm64 go build -trimpath -o build/ppvpn-core-linux-arm64 ./cmd/ppvpn-core
```

GitHub Actions 按平台拆分桌面产物：

- `build:macos-artifact`：macOS arm64/x86_64 XCFramework 与
  `macos-SHA256SUMS`。
- `build:windows-artifact`：Windows amd64/arm64 可执行文件与
  `windows-SHA256SUMS`。
- `build:linux-artifact`：Linux amd64/arm64 静态可执行文件与
  `linux-SHA256SUMS`。

本地可分别运行 `make build-macos-artifact`、`make build-macos-cli-artifact`
（`CGO_ENABLED=0` 的 `ppvpn-core serve` CLI，按架构分别输出
`build/ppvpn-core-darwin-arm64` 和 `build/ppvpn-core-darwin-amd64`，用 `lipo -verify_arch` 确认架构，
并写入 `macos-cli-SHA256SUMS`）和
`make build-windows-artifact`、`make build-linux-artifact`；`make build-desktop-artifact` 同时构建四者。

这些是未签名核心二进制。最终产品仍需在桌面 App 的发布流水线中完成平台代码签名、安装包封装、公证/信誉链和更新签名。

## 移动构建

```sh
make bootstrap-mobile
make build-mobile-ios
make build-mobile-macos
make build-mobile-android
make build-ios-artifact
make build-android-artifact
```

预期产物：

- `build/PPVPNCore.xcframework`：由所选目标生成；desktop artifact 必须是 macOS
  arm64/x86_64 universal slice，最低系统版本 13.0。
- `build/ppvpn-core.aar`：armeabi-v7a、arm64-v8a、x86、x86_64。

GitHub Actions 分别通过 `build:ios-artifact` 和 `build:android-artifact` 发布移动产物。
iOS job 交付 `PPVPNCore.xcframework.zip` 与 `ios-SHA256SUMS`；Android job
交付 `ppvpn-core.aar` 与 `android-SHA256SUMS`。两个 job 都会在上传前检查
iOS device/simulator 架构或 Android ABI，并永久保留产物。

XCFramework 必须由 iOS App/Extension 的 Xcode 流水线签名；AAR 由 Android App 的 Gradle/R8/签名流水线消费。核心仓库本身不保存产品签名密钥。

## 产物校验

```sh
cd build
shasum -a 256 -c SHA256SUMS
unzip -t PPVPNCore.xcframework.zip
unzip -t ppvpn-core.aar
unzip -l ppvpn-core.aar | grep 'jni/.*/libgojni.so'
```

对桌面文件再用 `file` 或平台等价工具确认架构，并逐个运行可执行架构的 `version`
命令。发布元数据必须同时记录 Core、Core API、Profile Schema、Flow Adapter 和 Local
Proxy Contract 版本；上层产品不依赖 core 内部实现版本。

任何源代码或依赖变化后，已有 `SHA256SUMS` 都失效，必须重新构建全部产物并生成新校验和：

```sh
cd build
shasum -a 256 \
  ppvpn-core-darwin-arm64 \
  ppvpn-core-darwin-amd64 \
  ppvpn-core-windows-amd64.exe \
  ppvpn-core-windows-arm64.exe \
  ppvpn-core-linux-amd64 \
  ppvpn-core-linux-arm64 \
  PPVPNCore.xcframework.zip \
  ppvpn-core.aar > SHA256SUMS
```

## 发布检查表

- 所有测试门禁通过，真实 sing-box 集成测试未被跳过。
- `govulncheck` 没有报告可达漏洞。
- 源码归档包含 `LICENSE`、`THIRD_PARTY_NOTICES.md` 和对应版本的依赖清单。
- `version` 输出与 `version/version.go`、tag 和发布元数据（`build-info.json`）一致。
- `go.mod` 与 Makefile 中固定依赖版本没有漂移。
- Profile golden 变化已人工审阅，未新增 Clash API 或公开内部 tag。
- 文档示例与公开 DTO 同步。
- 四个桌面架构、XCFramework 和 AAR 均从同一提交构建。
- 归档完整性、架构/ABI 和 SHA-256 全部验证。
- 上层产品完成平台签名、公证/发布签名、权限声明和升级/回滚演练。
- 发布环境不包含 Profile、会话密钥、本地代理状态或开发日志。
