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
{"core_version":"0.3.0","core_api_version":1,"profile_schema_version":2,"flow_adapter_version":1,"local_proxy_contract_version":1}
```

## 2. 准备 Profile

参考 [Backend Profile v2](backend-profile.md) 生成 `profile.json`，将所有演示地址和凭据换成真实服务值，然后先做离线校验：

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

`serve` 默认启用每节点认证代理。`--local-proxy=false` 可关闭每节点代理；核心不提供
无认证的系统代理兼容入口。
方式。

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

随后调用 `/v1/start`，并用 `/v1/get-local-proxy-metadata` 读取不含 secret 的每节点
HTTP/SOCKS5 端点；只有原生凭据面板按需调用 `/v1/get-local-proxy-credential`。Windows
产品由特权 service 以 TUN 模式运行 core，macOS 产品使用 XCFramework 和原生 Network
Extension；两者都不写系统 HTTP/SOCKS 设置。节点切换只调用 `/v1/select-node`，退出时
调用 `/v1/stop`。完整顺序和 DTO 见 [Core API v1](core-api.md)。

## 5. 常见问题

- `SCHEMA_UNSUPPORTED`：核心和后端的 Profile Schema 不兼容，先停止应用配置。
- `ENTRY_IP_NOT_PUBLIC`：`endpoint.ip` 不是可拨号公网单播 IP；不要填域名或文档地址。
- `TLS_SERVER_NAME_MISMATCH`：TLS SNI 必须等于 `endpoint.domain`。
- `CORE_OPERATION_FAILED`：上游错误已安全折叠。读取状态、检查第一方事件，并在受控环境用脱敏 `render` 辅助定位。
- 本地代理端口变更：进程启动时发现持久端口已占用，核心只为冲突节点重新分配；调用 `GetLocalProxyMetadata` 刷新。
- TUN/Network Extension 启动失败：保持未连接并由原生宿主通知用户；不要静默回退到系统代理。
