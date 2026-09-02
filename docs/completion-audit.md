# 完成度审计

本表用于实现验收；具体接入步骤见[快速开始](quickstart.md)、[Core API](core-api.md)、[移动端接入](mobile.md)和[构建发布](release.md)。

| 要求 | 实现与证据 |
| --- | --- |
| 版本化第一方 Profile、严格解析和迁移 | `profile/model.go`、`parse.go`、`validate.go` 及 Profile 测试 |
| 稳定节点 ID、字面量公网入口 IP、Fake-IP/保留地址拒绝 | Profile 校验与 `TestReservedEntryIPsRejected` |
| Shadowsocks 2022 EIH、VLESS REALITY、AnyTLS | 类型化凭据、内部配置构建器、协议与 golden 测试 |
| 直接构建 `option.Options`、固定 sing-box、公开层无 sing 类型 | `internal/config`、`go.mod`、`version` 及 Go `internal` 编译边界 |
| 平台/Profile 分离和路由 | 结构化平台能力、稳定状态与运行时/API 测试 |
| TUN 边界 | 配置构建已接通；桌面提权 helper 与移动 TUN-FD bridge 尚未实现，并在平台文档中明确 |
| 真实生命周期、revision 幂等、重载、回滚和入口迁移 | `internal/runtime`、假引擎测试与真实 sing-box 重载集成测试 |
| 入口和端到端可用性探测 | `probe` 的超时、取消、字面量 IP 和认证代理测试 |
| 每节点稳定独立 HTTP/SOCKS5 | `localproxy`、固定 inbound 路由与真实并发认证测试 |
| 流量、连接、事件和凭据安全诊断 | 第一方运行时 tracker、事件总线、脱敏与 API 测试；关闭上游日志 |
| 不依赖 Clash/私有 tag 的稳定 Core API | `api`、API 文档、认证/版本/脱敏测试 |
| 认证桌面 IPC 和崩溃退出 | Unix `0600` socket 测试、Windows owner-only Named Pipe、轮换密钥、stdin 父进程存活选项 |
| CLI | version、validate、脱敏 render、probe-entrance、serve |
| macOS/Windows | `build/` 下 arm64 与 amd64 二进制 |
| iOS 边界 | gomobile bridge、移动测试、XCFramework 与接入文档 |
| Android 边界 | gomobile bridge、移动测试、AAR；四 ABI JNI 库及 Bridge/event 绑定类 |
| 必要测试门禁 | `go test ./...` 与 `go test -race ./...`；包括 golden 和真实 sing-box 集成测试 |

Android NDK `29.0.14206865` 在明确接受许可证后安装。官方仓库大小 `1049519838`、SHA-1 `03d29fbb57e3c05a7d53597dd011d856c1456a4f` 均匹配，源 ZIP 和生成的 AAR 均通过归档完整性检查。
