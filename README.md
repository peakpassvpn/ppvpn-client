# ppvpn-core

`ppvpn-core` 是PPVPN第一方网络核心。后端只下发版本化、平台无关的 Proxy Profile；核心负责严格校验、转换为固定版本的内部运行配置，并统一管理路由判定、运行时、探测、节点独立本地代理、流量统计、桌面 IPC 和平台绑定。

当前版本：Core `0.3.0`、Core API `v1`、Profile Schema `v2`、Flow Adapter `v1`。支持 Shadowsocks 2022（含多用户/EIH）、VLESS + REALITY 和 AnyTLS。

## 文档导航

- [五分钟快速开始](docs/quickstart.md)：构建、启动桌面核心并完成第一个 API 调用
- [架构与生命周期](docs/architecture.md)：模块边界、状态机、热更新和失败回滚
- [Backend Profile v2](docs/backend-profile.md)：完整字段、路由语义、协议示例和后端生成规则
- [Core API v1](docs/core-api.md)：认证、请求/响应、所有方法、DTO、事件及错误码
- [桌面平台接入](docs/desktop.md)：Windows 特权 TUN 与 macOS 原生 Network Extension
- [移动端接入](docs/mobile.md)：iOS Network Extension 与 Android `VpnService`
- [安全模型](docs/security.md)：密钥边界、IPC、日志、持久化和威胁假设
- [构建与发布](docs/release.md)：测试门禁、跨平台构建、校验和与发布检查表
- [完成度审计](docs/completion-audit.md)：需求到代码和测试证据的映射

## 开发命令

```sh
go test ./...
go test -race ./...
go vet ./...
go run golang.org/x/vuln/cmd/govulncheck@v1.7.0 ./...
go run ./cmd/ppvpn-core version
go run ./cmd/ppvpn-core validate profile.json
go run ./cmd/ppvpn-core probe-entrance --timeout 5s --concurrency 4 profile.json
```

桌面客户端只能通过经过认证的 Core API 调用核心；移动端只能通过 `mobile.Bridge` 调用。后端和 App 均不得生成 sing-box JSON、依赖内部 tag 或调用 Clash API。Go 的 `internal/` 包边界会在编译期阻止外部项目导入 sing-box 配置与运行时类型。

## 许可证

`ppvpn-core` 以 [GNU GPL v3 或更高版本](LICENSE) 发布。项目包含和依赖的第三方组件可能适用额外条款；参见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

安全问题请勿通过公开 Issue 披露，报告方式见 [SECURITY.md](SECURITY.md)。
