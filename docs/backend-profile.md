# Backend Profile

本文是 `ppvpn-backend` 生成 Profile 的规范。代码定义以 [`profile/model.go`](../profile/model.go) 和 [`profile/validate.go`](../profile/validate.go) 为准。

只有一种 Profile 格式：`schema_version` 必须为 `1`，没有版本协商，也没有迁移路径；其他任何值都会以 `SCHEMA_UNSUPPORTED` 拒绝。

## 顶层结构

| 字段 | 类型 | 必填 | 规则 |
| --- | --- | --- | --- |
| `schema_version` | integer | 是 | 只能为 `1` |
| `revision` | string | 是 | 非空；有效配置发生变化时必须变更，内容由后端定义 |
| `generated_at` | RFC 3339 timestamp | 否 | 非零时必须早于 `expires_at` |
| `expires_at` | RFC 3339 timestamp | 否 | 非零时校验时刻必须早于该时间 |
| `nodes` | Node[] | 是 | 至少一个，`id` 唯一 |
| `selection` | object | 是 | 仅支持 `mode=manual`，默认节点必须存在 |
| `routing.rules` | RoutingRule[] | 否 | 数组顺序、first-match-wins，rule ID 必须唯一 |
| `routing.final` | RoutingAction | 是 | 没有显式规则命中时执行 |

解析器拒绝未知字段、尾随 JSON、未知协议/传输和不匹配的凭据联合体。不要通过“客户端忽略未知字段”做灰度。

## Node（逻辑节点）

Node 表示一个出口身份。选择节点、`routing` 中的 `node_id`、每节点本地代理、探测结果都以 Node `id` 为准；一个 Node 可以有多个入口（ingress），入口只是到达该出口的方式。

| 字段 | 说明 |
| --- | --- |
| `id` | 后端分配且永久稳定，正则为 `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`；不得使用线路 IP 或临时索引 |
| `name` | 展示名称；不用于路由身份 |
| `entry_key` | 必填，入口层级标识（例如 `cn-optimized`），正则 `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`；核心不解释其取值，原样透传到节点列表，客户端不得假设固定取值集合 |
| `entry_label` | 可选，入口层级的展示名称；原样透传到节点列表 |
| `exit` | 对象，字段均可选：`ip`（存在时必须是合法 IP）、`region` |
| `capabilities` | `tcp`、`udp` 至少一个为 true；每一项都必须被第一个（primary）入口支持 |
| `ingresses` | Ingress[]，1–64 个；数组顺序即故障转移顺序。`ingresses[0]` 的 `role` 必须为 `primary`，其余必须为 `backup` |

### Ingress 字段

| 字段 | 说明 |
| --- | --- |
| `role` | 必填。`ingresses[0]` 为 `primary`，其余全部为 `backup`；与位置不符以 `INGRESS_ROLE_INVALID` 拒绝 |
| `endpoint_key` | 必填，副本的稳定标识，正则 `[A-Za-z0-9][A-Za-z0-9._:-]{0,127}`，在整个 Profile 内唯一。核心用它生成稳定的 outbound tag，并在节点列表/探测结果中标识副本 |
| `replica_ordinal` | 必填（`0` 也必须显式给出），非负整数；同一 Node 内唯一且按数组顺序严格递增（不要求连续） |
| `protocol` | `shadowsocks`、`vless` 或 `anytls` |
| `endpoint.domain` | 实际连接域名，同时必须等于 TLS `server_name`（需要 TLS 的协议） |
| `endpoint.ip` | 可选。存在时必须是公网单播 IP，用于入口探测和 TUN 路由排除；缺省时探测解析 `domain` |
| `endpoint.port` | 1–65535 |
| `credentials` | 必须且只能包含与该入口协议同名的一项 |
| `tls` | VLESS REALITY 与 AnyTLS 必须提供 |
| `transport` | 只能缺省或 `type` 为空；非空传输会被拒绝 |
| `capabilities` | `tcp`、`udp` 至少一个为 true |

`endpoint.ip` 会拒绝私网、回环、链路本地、组播、未指定、文档网段、基准测试网段、CGNAT 和其他保留地址。建议后端为每个入口（包括 backup）都提供 IP：桌面 TUN 只能对已知 IP 做操作系统级路由排除。

### 故障转移语义

每个入口渲染为一个独立的 sing-box outbound；多入口 Node 渲染为核心内置的 `ppvpn-failover` 组（单入口 Node 直接使用该入口 outbound）：

- primary 即 `ingresses[0]`，backup 为其余入口，按数组顺序尝试；故障转移只在同一 Node 的入口之间进行，绝不跨 Node。
- 新连接总是优先使用 primary；拨号失败（非调用方取消）时立即在同一次拨号中尝试下一个 backup，并把 primary 标记为不健康。
- 健康检查是经过入口发出的 HTTP 204 请求，只在该节点被使用时运行（空闲 30 分钟后停止）。primary 健康时每 3 分钟只检查 primary；primary 不健康时每 20 秒检查全部入口，primary 一旦恢复即切回。
- 切换不会中断已建立的连接。
- 多入口 Node 的成员 outbound tag 为 `node-<sha256(node.id) 前 8 字节>-<sha256(endpoint_key) 前 4 字节>`（十六进制），只依赖 Node `id` 与 `endpoint_key`，与数组位置无关；因此 revision 间调整顺序或增删其他副本不会改变已有副本的 tag。Node 自身的 tag（选择器、本地代理、路由目标）只依赖 Node `id`。

上游 `urltest` 按最低延迟选择且带容差，不会“优先 primary、恢复后切回”，因此没有使用。

## 完整示例

以下示例用于说明形状。域名、IP 和所有密钥均是演示值，部署前必须替换。注意 `endpoint.ip` 必须是公网单播地址：文档网段（如 `198.51.100.0/24`、`203.0.113.0/24`）会以 `ENTRY_IP_NOT_PUBLIC` 拒绝（`exit.ip` 不受此限制）。另见 [`testdata/profiles/multi-ingress.json`](../testdata/profiles/multi-ingress.json)。

```json
{
  "schema_version": 1,
  "revision": "cfg-2026-09-29-0001",
  "generated_at": "2026-09-29T00:00:00Z",
  "expires_at": "2099-01-01T00:00:00Z",
  "nodes": [
    {
      "id": "3f2c9a1e-5b7d-4c1e-9f00-123456789abc-128",
      "name": "日本-203.0.113.10",
      "entry_key": "cn-optimized",
      "exit": {"ip": "203.0.113.10", "region": "日本"},
      "capabilities": {"tcp": true, "udp": true},
      "ingresses": [
        {
          "role": "primary",
          "endpoint_key": "9001",
          "replica_ordinal": 0,
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
          "role": "backup",
          "endpoint_key": "9002",
          "replica_ordinal": 1,
          "protocol": "shadowsocks",
          "endpoint": {"domain": "relay.example.com", "port": 8443},
          "credentials": {
            "shadowsocks": {
              "method": "2022-blake3-aes-128-gcm",
              "server_key": "MDEyMzQ1Njc4OWFiY2RlZg==",
              "identity_keys": ["ZmVkY2JhOTg3NjU0MzIxMA=="]
            }
          },
          "capabilities": {"tcp": true, "udp": true}
        }
      ]
    },
    {
      "id": "3f2c9a1e-5b7d-4c1e-9f00-123456789abc-129",
      "name": "美国-203.0.113.20",
      "entry_key": "cn-optimized",
      "exit": {"region": "美国"},
      "capabilities": {"tcp": true, "udp": false},
      "ingresses": [
        {
          "role": "primary",
          "endpoint_key": "9003",
          "replica_ordinal": 0,
          "protocol": "anytls",
          "endpoint": {"domain": "anytls.example.com", "ip": "9.9.9.9", "port": 443},
          "credentials": {"anytls": {"password": "REPLACE_WITH_SECRET"}},
          "tls": {"server_name": "anytls.example.com", "alpn": ["h2"]},
          "capabilities": {"tcp": true, "udp": false}
        }
      ]
    }
  ],
  "selection": {"mode": "manual", "default_node_id": "3f2c9a1e-5b7d-4c1e-9f00-123456789abc-128"},
  "routing": {
    "rules": [
      {
        "id": "bypass-private",
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
        "action": {"type": "proxy", "target": "node", "node_id": "3f2c9a1e-5b7d-4c1e-9f00-123456789abc-129"}
      }
    ],
    "final": {"type": "proxy", "target": "selected"}
  }
}
```

## Routing

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
- 必须提供 REALITY；`public_key` 是 base64url（无填充）编码的 32 字节 X25519 公钥（`sing-box generate reality-keypair` 的输出格式）。
- 核心为 REALITY 启用 uTLS（`chrome` 指纹）；发布构建必须带 `with_utls` 构建标签（Makefile 默认）。
- `short_id` 是最长 16 个字符的偶数长度十六进制字符串，也允许空字符串。
- `tls.server_name` 必须与 `endpoint.domain` 完全相同。

### AnyTLS

- `password` 非空。
- 必须提供 TLS，且 `tls.server_name == endpoint.domain`。

## revision 与更新策略

节点的 `id` 表示逻辑线路，入口（增删 backup、IP、端口、密钥）或展示信息变化时保持 ID 不变并生成新 `revision`。核心据此保留用户选择和本地代理端点，并在任一入口 endpoint 变化时产生 `NodeEndpointChanged` 事件。同一 revision 的重复下发是幂等操作，核心返回 `applied=false`。

后端不得下发 `inbounds`、`outbounds`、`route`、`clash_api`、sing-box tag、本地端口、TUN 设置、平台名称、系统代理设置或日志级别。
