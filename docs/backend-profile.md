# Backend Profile v2

本文是 `ppvpn-backend` 生成 Profile 的规范。代码定义以 [`profile/model.go`](../profile/model.go) 和 [`profile/validate.go`](../profile/validate.go) 为准。

## 顶层结构

| 字段 | 类型 | 必填 | 规则 |
| --- | --- | --- | --- |
| `schema_version` | integer | 是 | 当前只能为 `2` |
| `revision` | string | 是 | 非空；有效配置发生变化时必须变更，内容由后端定义 |
| `generated_at` | RFC 3339 timestamp | 否 | 非零时必须早于 `expires_at` |
| `expires_at` | RFC 3339 timestamp | 否 | 非零时校验时刻必须早于该时间 |
| `nodes` | Node[] | 是 | 至少一个，`id` 唯一 |
| `selection` | object | 是 | v2 仅支持 `mode=manual`，默认节点必须存在 |
| `routing.rules` | RoutingRule[] | 否 | 数组顺序、first-match-wins，rule ID 必须唯一 |
| `routing.final` | RoutingAction | 是 | 没有显式规则命中时执行 |

解析器拒绝未知字段、尾随 JSON、未知协议/传输和不匹配的凭据联合体。不要通过“客户端忽略未知字段”做灰度；新字段必须进入新的兼容 Schema 设计。

## Node 字段

| 字段 | 说明 |
| --- | --- |
| `id` | 后端分配且永久稳定，正则为 `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`；不得使用线路 IP 或临时索引 |
| `name` | 展示名称；不用于路由身份 |
| `protocol` | `shadowsocks`、`vless` 或 `anytls` |
| `endpoint.domain` | 实际连接域名，同时必须等于 TLS `server_name`（需要 TLS 的协议） |
| `endpoint.ip` | 仅用于入口探测的字面量公网单播 IP |
| `endpoint.port` | 1–65535 |
| `exit` | 可选出口 IP、区域和两字母国家/地区展示信息 |
| `credentials` | 必须且只能包含与协议同名的一项 |
| `tls` | VLESS REALITY 与 AnyTLS 必须提供 |
| `transport` | v2 只能缺省或 `type` 为空；非空传输会被拒绝 |
| `capabilities` | `tcp`、`udp` 至少一个为 true |

`endpoint.ip` 会拒绝私网、回环、链路本地、组播、未指定、文档网段、基准测试网段、CGNAT 和其他保留地址。后端必须提供可直接拨号的真实公网入口 IP。

## 完整示例

以下示例用于说明三种协议的形状。域名、IP 和所有密钥均是演示值，部署前必须替换；替换后的 `endpoint.ip` 必须是真实公网地址。

```json
{
  "schema_version": 2,
  "revision": "cfg-2026-07-23-0001",
  "generated_at": "2026-07-23T00:00:00Z",
  "expires_at": "2099-01-01T00:00:00Z",
  "nodes": [
    {
      "id": "hk-ss-001",
      "name": "香港 SS 01",
      "protocol": "shadowsocks",
      "endpoint": {"domain": "ss.example.com", "ip": "8.8.8.8", "port": 443},
      "exit": {"region": "Hong Kong", "country_code": "HK"},
      "credentials": {
        "shadowsocks": {
          "method": "2022-blake3-aes-128-gcm",
          "server_key": "MDEyMzQ1Njc4OWFiY2RlZg==",
          "identity_keys": ["ZmVkY2JhOTg3NjU0MzIxMA=="]
        }
      },
      "capabilities": {"tcp": true, "udp": true}
    },
    {
      "id": "jp-vless-001",
      "name": "日本 VLESS 01",
      "protocol": "vless",
      "endpoint": {"domain": "vless.example.com", "ip": "1.1.1.1", "port": 443},
      "credentials": {
        "vless": {"uuid": "123e4567-e89b-42d3-a456-426614174000", "flow": "xtls-rprx-vision"}
      },
      "tls": {
        "server_name": "vless.example.com",
        "alpn": ["h2", "http/1.1"],
        "reality": {"public_key": "REPLACE_WITH_REAL_PUBLIC_KEY", "short_id": "1a2b3c4d"}
      },
      "capabilities": {"tcp": true, "udp": true}
    },
    {
      "id": "us-anytls-001",
      "name": "美国 AnyTLS 01",
      "protocol": "anytls",
      "endpoint": {"domain": "anytls.example.com", "ip": "9.9.9.9", "port": 443},
      "credentials": {"anytls": {"password": "REPLACE_WITH_SECRET"}},
      "tls": {"server_name": "anytls.example.com", "alpn": ["h2"]},
      "capabilities": {"tcp": true, "udp": false}
    }
  ],
  "selection": {"mode": "manual", "default_node_id": "hk-ss-001"},
  "routing": {
    "rules": [
      {
        "id": "private-direct",
        "match": {"ip_is_private": true},
        "action": {"type": "direct"}
      },
      {
        "id": "company-fixed-node",
        "match": {
          "domain_suffixes": ["example.org"],
          "protocols": ["tcp"],
          "ports": [443],
          "port_ranges": ["8000-9000"]
        },
        "action": {"type": "proxy", "target": "node", "node_id": "jp-vless-001"}
      }
    ],
    "final": {"type": "proxy", "target": "selected"}
  }
}
```

## Routing v2

`match` 支持 `domains`、`domain_suffixes`、`ip_cidrs`、`ip_is_private`、
`protocols`（仅 `tcp`/`udp`）、`ports` 和包含首尾的 `port_ranges`
（`start-end`）。域名、suffix、CIDR 和 private 构成一个“目标地址”类别并互为 OR；
单端口与端口范围互为 OR；目标地址、协议、端口三个非空类别之间为 AND。空 matcher、
空 suffix、通配符、非法 CIDR/端口范围、重复 rule ID 和未知字段都会被拒绝。

域名在比较前去掉一个末尾 `.`、转换成 IDNA ASCII A-label 并转为小写。suffix 只在 DNS
label 边界匹配：`example.com` 匹配自身和 `a.example.com`，不匹配
`badexample.com`；IP literal 不进入域名匹配。

`ip_is_private` 固定表示 RFC 1918 IPv4（`10/8`、`172.16/12`、`192.168/16`）和
RFC 4193 IPv6 ULA（`fc00::/7`），不把 loopback、link-local 或文档网段混入“私网”。

动作联合体只有以下四种合法形状：

- `{"type":"direct"}`
- `{"type":"reject"}`
- `{"type":"proxy","target":"selected"}`
- `{"type":"proxy","target":"node","node_id":"stable-node-id"}`

固定优先级为平台安全/防递归、每节点本地入口绑定、Profile 显式规则、`routing.final`。
切换 selected 只影响之后创建的 flow，已建立连接不迁移也不中断。

## 协议约束

### Shadowsocks 2022

支持的方法和解码后的 Base64 密钥长度：

| method | `server_key` / 每个 `identity_key` |
| --- | --- |
| `2022-blake3-aes-128-gcm` | 16 bytes |
| `2022-blake3-aes-256-gcm` | 32 bytes |
| `2022-blake3-chacha20-poly1305` | 32 bytes |

后端分别发送 `server_key` 和有顺序的 `identity_keys`。核心在内部构造 EIH 密码，后端不得预拼接。

### VLESS + REALITY

- `uuid` 必须是 RFC 4122 形状、版本 1–5 的 UUID。
- v2 必须提供 REALITY；`public_key` 非空。
- `short_id` 是最长 16 个字符的偶数长度十六进制字符串，也允许空字符串。
- `tls.server_name` 必须与 `endpoint.domain` 完全相同。

### AnyTLS

- `password` 非空。
- 必须提供 TLS，且 `tls.server_name == endpoint.domain`。

## revision 与更新策略

节点的 `id` 表示逻辑线路，入口 IP、端口、密钥或展示信息变化时保持 ID 不变并生成新 `revision`。核心据此保留用户选择和本地代理端点，并为入口变化产生 `NodeEndpointChanged` 事件。同一 revision 的重复下发是幂等操作，核心返回 `applied=false`。

后端不得下发 `inbounds`、`outbounds`、`route`、`clash_api`、sing-box tag、本地端口、TUN 设置、平台名称、系统代理设置或日志级别。
