# ppvpn-core

`ppvpn-core` 是PPVPN第一方网络核心。后端只下发版本化、平台无关的 Proxy Profile；核心负责严格校验、转换为固定版本的 sing-box 配置，并统一管理运行时、探测、节点独立本地代理、设备系统代理端点、流量统计、桌面 IPC 和移动端绑定。

当前版本：Core `0.2.0`、Core API `v1`、Profile Schema `v1`、sing-box `1.13.12`。支持 Shadowsocks 2022（含多用户/EIH）、VLESS + REALITY 和 AnyTLS。

## 文档导航

- [五分钟快速开始](docs/quickstart.md)：构建、启动桌面核心并完成第一个 API 调用
- [架构与生命周期](docs/architecture.md)：模块边界、状态机、热更新和失败回滚
- [Backend Profile v1](docs/backend-profile.md)：完整字段、约束、协议示例和后端生成规则
- [Core API v1](docs/core-api.md)：认证、请求/响应、所有方法、DTO、事件及错误码
- [桌面平台接入](docs/desktop.md)：sidecar、系统代理调用顺序、崩溃清理与 TUN 边界
- [移动端接入](docs/mobile.md)：iOS Network Extension 与 Android `VpnService`
- [安全模型](docs/security.md)：密钥边界、IPC、日志、持久化和威胁假设
- [构建与发布](docs/release.md)：测试门禁、跨平台构建、校验和与发布检查表
- [完成度审计](docs/completion-audit.md)：需求到代码和测试证据的映射

## 开发命令

```sh
go test ./...
go test -race ./...
go vet ./...
go run ./cmd/ppvpn-core version
go run ./cmd/ppvpn-core validate profile.json
go run ./cmd/ppvpn-core probe-entrance --timeout 5s --concurrency 4 profile.json
```

桌面客户端只能通过经过认证的 Core API 调用核心；移动端只能通过 `mobile.Bridge` 调用。后端和 App 均不得生成 sing-box JSON、依赖内部 tag 或调用 Clash API。Go 的 `internal/` 包边界会在编译期阻止外部项目导入 sing-box 配置与运行时类型。
