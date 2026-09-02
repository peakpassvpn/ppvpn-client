# 安全模型

## 资产与信任边界

高敏感资产包括 Profile 协议凭据、桌面会话密钥和每节点本地代理凭据。受信组件只有后端、核心进程、桌面主进程以及移动端 Network Extension/`VpnService`。WebView、渲染进程、第三方插件、系统剪贴板、日志/分析平台和崩溃报告默认不受信。

本设计防御同机非特权进程读取 IPC、误把秘密写入日志、后端注入 sing-box 私有配置、内部 tag 泄露成产品协议，以及恶意/错误 Profile 使用私网入口做探测。它不防御已经控制当前操作系统用户、已越狱/root 的设备或被替换的应用二进制。

## 秘密生命周期

| 数据 | 来源 | 内存 | 持久化 | 对外可见范围 |
| --- | --- | --- | --- | --- |
| Profile 凭据 | 后端 | 当前 Core Profile | 核心不持久化 | Apply/Validate 输入；不出现在节点/状态 API |
| 桌面会话密钥 | 核心启动时随机生成 | 核心与桌面宿主 | 临时交换文件；退出删除 | Authorization header |
| 本地代理凭据 | 核心随机生成 | 核心与宿主 | `local-proxies.json` | 仅 GetLocalProxyEndpoints / Bridge 对应方法 |
| sing-box 内部 tag | 核心构建 | 核心内部 | 不持久化 | 永不进入公开 DTO |

Profile 可能由宿主暂存以完成进程间交接，但这属于宿主责任：使用应用私有目录、原子写入、平台数据保护并在读取后删除。

## 桌面 IPC

- Unix socket 创建为 `0600`；只在旧路径确实是 socket 时清理，拒绝覆盖普通文件或符号目标。
- Windows Named Pipe 使用当前 owner-only ACL；状态目录必须位于当前用户私有 LocalAppData。
- 每次启动轮换至少 256 bit 随机会话密钥；Unix 交换文件为 `0600`。
- Bearer 比较使用恒定时间比较。所有调用都要发送认证，事件流也不例外。
- 生产宿主应使用 `--exit-on-stdin-close` 的父进程存活管道，并在启动超时、异常退出时回收子进程。
- 不要让 HTTP 客户端自动把本机 Bearer header 重定向到 TCP/网络 URL。

## 本地代理

每节点 HTTP/SOCKS5 只绑定 `127.0.0.1`，每节点使用独立随机用户名和密码。状态目录 Unix 权限为 `0700`，文件为 `0600`，写入采用同目录临时文件加原子 rename。启动时仅对已占用端口重新分配；宿主不能缓存端点跨越一次启动而不刷新。

回环监听并不等于无认证：本机其他进程也可访问回环端口。因此宿主必须使用返回的凭据，不能降级为无认证代理，也不得把凭据注入环境变量或子进程命令行。

核心不提供无认证 loopback 代理。Core API v1 的旧系统代理查询路由只返回
`SYSTEM_PROXY_UNAVAILABLE`，不会创建监听器或持久化端点状态。

## Profile 防护

- JSON 严格解码，未知字段、歧义 credential union 和尾随值全部失败关闭。
- 只接受固定协议集合；Profile v2 拒绝未知 transport。
- 入口探测 IP 必须是公开单播，并明确拒绝私网、回环、链路本地、CGNAT、文档、基准测试和保留网段。
- 实际协议连接使用域名；需要 TLS 时 SNI 必须与该域名相等。
- 后端不能控制平台 TUN、本地监听、日志或任何 sing-box/Clash 字段。

这些约束减少 SSRF 和配置注入面，但可用性探测的 `target` 目前由受信宿主提供。产品层应把目标限制为PPVPN运营的固定 HTTPS 健康检查 URL，不要直接接受网页或不受信 IPC 调用方输入。

## 日志、错误与诊断

sing-box 上游日志被关闭，因为其文本没有第一方脱敏保证。公开事件只含稳定节点 ID、revision 和安全摘要。未知运行时错误统一折叠为 `CORE_OPERATION_FAILED` / `core operation failed`，不回传上游错误文本。

CLI `render` 会对已知敏感 JSON 键和代理 URL 认证信息脱敏，但脱敏输出仍可能暴露拓扑、域名、IP 和节点数量。仅在受控开发环境使用，不要自动上传。

## 运维检查表

- 应用签出、卸载或“清除数据”时删除核心状态目录。
- 崩溃采集器排除 Profile、session secret 文件、`local-proxies.json` 和 API 原始 body/header。
- 密钥文件、socket、Named Pipe 名称不放入全局可读目录。
- API 客户端对认证失败重新握手，不复用上一次进程的 secret。
- 发布前执行 race 测试、跨平台构建和校验和验证，详见 [构建与发布](release.md)。
