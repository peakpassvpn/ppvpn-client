# ppvpn-core

`ppvpn-core` 是PPVPN第一方网络核心。后端只下发版本化、平台无关的 Proxy Profile；核心负责严格校验、转换为固定版本的内部运行配置，并统一管理路由判定、运行时、探测、共享端口认证本地代理（用户名选节点）、流量统计、桌面 IPC 和平台绑定。

引擎是 Rust 库 `ppvpn-core`（`crates/core`，内嵌 Sail），由 Desktop 和 CLI 在进程内承载，见 [#214](https://github.com/peakpassvpn/ppvpn-client/issues/214)。原来的 Go 内核已从 main 移除，最后一版是 v0.5.21，它的 tag 和 Release 文件保留；切换前 Desktop 仍 vendor 这个版本。接口版本：Core API `v1`、Profile Schema `1`。支持 Shadowsocks 2022（含多用户/EIH）、VLESS + REALITY 和 AnyTLS。

## 文档导航

- [五分钟快速开始](docs/quickstart.md)：构建、在 Rust 宿主或 CLI 里运行引擎、调用测试宿主的 Core API
- [架构与生命周期](docs/architecture.md)：模块边界、状态机、原地 reload 与完整重启
- [Backend Profile](docs/backend-profile.md)：逻辑节点/多入口故障转移、完整字段、路由语义、协议示例和后端生成规则
- [Core API v1](docs/core-api.md)：认证、请求/响应、所有方法、DTO、事件及错误码
- [桌面平台接入](docs/desktop.md)：Windows 与 macOS 都由特权 service 以 TUN 模式运行 core
- [移动端接入](docs/mobile.md)：移动端现状（FFI 尚未提供）与宿主职责
- [Rust 版行为对照](docs/rust-parity.md)：Go 测试与 Rust `ppvpn-core`（`crates/core`）用例的逐条对照（#214 硬切换的前提）
- [测试分层](docs/testing.md)：L1 单元、L2 集成、L3 系统，各层的依赖、入口和 CI 时机
- [宿主接入](docs/host-integration.md)：Rust API 契约
- [贡献须知](CONTRIBUTING.md)：敏感信息检查与 pre-push 钩子（每个 clone 执行 `git config core.hooksPath .githooks`）
- [安全模型](docs/security.md)：密钥边界、IPC、日志、持久化和威胁假设
- [构建与发布](docs/release.md)：测试门禁、跨平台构建、校验和与发布检查表
- [完成度审计](docs/completion-audit.md)：需求到代码和测试证据的映射

## 开发命令

```sh
make test-unit          # L1
make test-integration   # L2
make test-system        # L3，Linux，需要 sudo
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --all --check
```

分层和 CI 的安排见 [docs/testing.md](docs/testing.md)。

> 下列文档描述的是 Go 内核（v0.5.21），保留作参考：完成度审计、构建与发布中关于 Go 构建的部分。

## 许可证

`ppvpn-core` 以 [GNU GPL v3 或更高版本](LICENSE) 发布。项目包含和依赖的第三方组件可能适用额外条款；参见 [THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md)。

安全问题请勿通过公开 Issue 披露，报告方式见 [SECURITY.md](SECURITY.md)。
