# 五分钟快速开始

本指南用于本地验证和桌面端接入。需要 Go 1.25 或 `go.mod` 指定的兼容工具链。

## 1. 构建与自检

```sh
go test ./...
go build -trimpath -o build/ppvpn-core ./cmd/ppvpn-core
./build/ppvpn-core version
```

预期版本响应：

```json
{"core_version":"0.2.0","core_api_version":1,"profile_schema_version":1,"sing_box_version":"1.13.12"}
```

## 2. 准备 Profile

参考 [Backend Profile v1](backend-profile.md) 生成 `profile.json`，将所有演示地址和凭据换成真实服务值，然后先做离线校验：

```sh
./build/ppvpn-core validate profile.json
./build/ppvpn-core probe-entrance --timeout 5s --concurrency 4 profile.json
```

`validate` 成功输出 `profile valid`。入口探测只验证公网字面量 IP 的 TCP 可达性，不等价于节点协议可用。

需要排查配置转换时可使用：

```sh
./build/ppvpn-core render --platform macos profile.json
```

输出已经过脱敏，但仍只应在本机受控环境查看；该命令不是后端或产品客户端的配置生成接口。

## 3. 启动桌面核心

创建一个仅当前用户可访问的应用状态目录。以下路径仅为 macOS/Linux 开发示例：

```sh
APP_STATE=/private/tmp/ppvpn-core-demo
mkdir -m 700 "$APP_STATE"

./build/ppvpn-core serve \
  --socket "$APP_STATE/core.sock" \
  --session-secret-file "$APP_STATE/session.secret" \
  --state-dir "$APP_STATE/state" \
  --platform macos
```

`serve` 默认启用每节点认证代理和设备系统代理。可用 `--local-proxy=false` 或 `--system-proxy=false` 分别关闭；系统代理监听可通过 `--system-proxy-listen` 指定，但只能是显式回环 IP。

生产环境不要使用共享临时目录。macOS 应使用 App Container/Application Support 私有目录；Windows 应使用带当前用户 ACL 的 LocalAppData 目录和 Named Pipe 路径。

`serve` 每次启动覆盖生成新的会话密钥，正常退出时删除密钥文件。产品桌面端还应启用 `--exit-on-stdin-close`，并保持传入核心的 stdin 写端存活，使父 App 崩溃后核心自动退出。

## 4. 调用 API

在另一个终端：

```sh
APP_STATE=/private/tmp/ppvpn-core-demo
SECRET=$(cat "$APP_STATE/session.secret")

curl --unix-socket "$APP_STATE/core.sock" \
  -H "Authorization: Bearer $SECRET" \
  -H 'X-Core-API-Version: 1' \
  -H 'X-Request-ID: quickstart-1' \
  -H 'Content-Type: application/json' \
  --data '{}' \
  http://localhost/v1/get-version
```

应用 Profile 时，不要用 shell 拼接含密钥的命令。产品代码应直接在内存中编码请求并写入 IPC。仅本机开发可用下列 `jq` 示例：

```sh
jq -n --slurpfile profile profile.json '{profile:$profile[0]}' > /private/tmp/apply-request.json

curl --unix-socket "$APP_STATE/core.sock" \
  -H "Authorization: Bearer $SECRET" \
  -H 'X-Core-API-Version: 1' \
  -H 'Content-Type: application/json' \
  --data-binary @/private/tmp/apply-request.json \
  http://localhost/v1/apply-profile
```

随后调用 `/v1/start`，再调用 `/v1/get-system-proxy-endpoints`。Desktop 将 `http` 端点写为系统 HTTP/HTTPS 代理，将 `socks5` 端点写为系统 SOCKS 代理；当前 mixed 实现中二者端口相同。节点切换只调用 `/v1/select-node`，无需重写系统代理。退出时先撤销操作系统代理设置，再调用 `/v1/stop`。完整顺序和 DTO 见 [Core API v1](core-api.md)。

## 5. 常见问题

- `SCHEMA_UNSUPPORTED`：核心和后端的 Profile Schema 不兼容，先停止应用配置。
- `ENTRY_IP_NOT_PUBLIC`：`endpoint.ip` 不是可拨号公网单播 IP；不要填域名或文档地址。
- `TLS_SERVER_NAME_MISMATCH`：TLS SNI 必须等于 `endpoint.domain`。
- `CORE_OPERATION_FAILED`：上游错误已安全折叠。读取状态、检查第一方事件，并在受控环境用脱敏 `render` 辅助定位。
- 本地代理端口变更：进程启动时发现持久端口已占用，核心只为冲突节点重新分配；调用 `GetLocalProxyEndpoints` 刷新。
- 系统代理端口变更：core 会迁移并持久化新端口，`SystemProxyEndpointReady` 携带新端点；Desktop 必须据此原子更新系统代理。
