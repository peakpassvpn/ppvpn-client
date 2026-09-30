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

解析器**忽略未知字段**（0.4.4 起），以便后端新增可选字段时不破坏已发布的客户端；缺少或非法的必填字段、尾随 JSON、未知协议/传输和不匹配的凭据联合体仍然拒绝。
已删除或改名的字段会以带编码的错误拒绝（例如 `shadowsocks.server_key` → `SHADOWSOCKS_SERVER_KEY_REMOVED`），避免后端仍发旧形状时被静默忽略。
新增字段必须是可选的、客户端不理解时可以安全忽略的；改变已有字段语义或新增必填字段需要新的 `schema_version`。

## Node（逻辑节点）

Node 表示一个出口身份。选择节点、`routing` 中的 `node_id`、每节点本地代理、探测结果都以 Node `id` 为准；一个 Node 可以有多个入口（ingress），入口只是到达该出口的方式。

| 字段 | 说明 |
| --- | --- |
| `id` | 后端分配且永久稳定，正则为 `[A-Za-z0-9][A-Za-z0-9._-]{0,127}`；不得使用线路 IP 或临时索引 |
| `name` | 展示名称；不用于路由身份 |
| `entry_key` | 必填，入口层级标识（例如 `cn-optimized`），正则 `[A-Za-z0-9][A-Za-z0-9._-]{0,63}`；核心不解释其取值，原样透传到节点列表，客户端不得假设固定取值集合 |
| `entry_label` | 可选，入口层级的展示名称；原样透传到节点列表 |
| `exit` | 对象，字段均可选：`ip`（存在时必须是合法 IP）、`region`、`country_code`（ISO 3166-1 alpha-2，仅展示） |
| `capabilities` | `tcp`、`udp` 至少一个为 true；每一项都必须被第一个（primary）入口支持 |
| `ingresses` | Ingress[]，1–64 个；数组顺序即故障转移顺序。`ingresses[0]` 的 `role` 必须为 `primary`，其余必须为 `backup` |

### Ingress 字段

| 字段 | 说明 |
| --- | --- |
| `role` | 必填。`ingresses[0]` 为 `primary`，其余全部为 `backup`；与位置不符以 `INGRESS_ROLE_INVALID` 拒绝 |
| `endpoint_key` | 必填，副本的稳定标识，正则 `[A-Za-z0-9][A-Za-z0-9._:-]{0,127}`，在整个 Profile 内唯一。核心用它生成稳定的 outbound tag，并在节点列表/探测结果中标识副本 |
| `label` | 可选，仅用于展示的副本名称（例如 `东京中转`），不是标识符：核心不用它生成 tag、路由或故障转移。存在时必须非空、首尾无空白、不含控制字符、最多 32 个字符（Unicode 码点），否则以 `INGRESS_LABEL_INVALID` 拒绝。核心在节点列表、入口探测结果和 `selected_ingress` 中以 `label` 原样返回 |
| `replica_ordinal` | 必填（`0` 也必须显式给出），非负整数；同一 Node 内唯一且按数组顺序严格递增（不要求连续） |
| `protocol` | `shadowsocks`、`vless` 或 `anytls` |
| `endpoint.domain` | 实际连接域名；AnyTLS 的 TLS `server_name` 必须等于它（REALITY 不要求） |
| `endpoint.ip` | 可选。存在时必须是公网单播 IP，核心直接拨这个 IP 连接节点（TLS/REALITY 仍用 `tls.server_name`），并用于入口探测和 TUN 路由排除；缺省时拨号和探测都解析 `domain` |
| `endpoint.port` | 1–65535 |
| `credentials` | 必须且只能包含与该入口协议同名的一项 |
| `tls` | VLESS REALITY 与 AnyTLS 必须提供 |
| `transport` | 只能缺省或 `type` 为空；非空传输会被拒绝 |
| `capabilities` | `tcp`、`udp` 至少一个为 true |

`endpoint.ip` 会拒绝私网、回环、链路本地、组播、未指定、文档网段、基准测试网段、CGNAT 和其他保留地址。建议后端为每个入口（包括 backup）都提供 IP：桌面 TUN 只能对已知 IP 做操作系统级路由排除；没有 IP 的入口每次连接前都要解析域名，TUN 模式下这次解析可能绕回隧道自身的 DNS。

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
      "exit": {"ip": "203.0.113.10", "region": "日本", "country_code": "JP"},
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
              "identity_keys": ["ZmVkY2JhOTg3NjU0MzIxMA=="],
              "user_key": "MDEyMzQ1Njc4OWFiY2RlZg=="
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
      "exit": {"region": "美国", "country_code": "US"},
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
`rule_set_ids`（见下文 [规则集](#规则集rule_sets)）、`protocols`（仅 `tcp`/`udp`）、`ports` 和包含首尾的 `port_ranges`
（`start-end`）。域名、suffix、CIDR、private 和规则集构成一个“目标地址”类别并互为 OR；
单端口与端口范围互为 OR；目标地址、协议、端口三个非空类别之间为 AND。空 matcher、
空 suffix、通配符、非法 CIDR/端口范围和重复 rule ID 都会被拒绝（未知字段按上文忽略）。

规则可带可选的 `baseline: true`（缺省 `false`，缺省时不必下发）：宿主以全局模式（`routing_mode: "global"`，见
core-api.md）应用 Profile 时，只保留 baseline 规则，其余规则丢弃、`final` 固定为代理。适合标为 baseline 的是
任何模式下都必须生效的规则，例如 `bypass-private`、官方 API 直连。0.5.6 之前的核心忽略该字段。

域名在比较前去掉一个末尾 `.`、转换成 IDNA ASCII A-label 并转为小写。suffix 只在 DNS
label 边界匹配：`example.com` 匹配自身和 `a.example.com`，不匹配
`badexample.com`；IP literal 不进入域名匹配。

`ip_is_private` 固定表示不应经代理的本地与特殊用途地址（核心 0.5.7 起，与订阅侧的系统私网直连对齐）：
IPv4 `10/8`、`172.16/12`、`192.168/16`、`100.64/10`（CGNAT）、`0/8`、`127/8`、`169.254/16`、
`224/4`（组播）、`240/4`、`255.255.255.255/32`；IPv6 `fc00::/7`、`fe80::/10`、`ff00::/8`、`::1/128`。
文档网段（TEST-NET）与 `198.18/15` 不在其中。0.5.7 之前只含 RFC 1918 与 `fc00::/7`。桌面 TUN 另有一条
与之相同的内置直连规则（见 security.md），不依赖 Profile。

动作联合体只有以下四种合法形状：

- `{"type":"direct"}`
- `{"type":"reject"}`
- `{"type":"proxy","target":"selected"}`
- `{"type":"proxy","target":"node","node_id":"stable-node-id"}`

固定优先级为平台安全/防递归、每节点本地入口绑定、Profile 显式规则、`routing.final`。
切换 selected 只影响之后创建的 flow，已建立连接不迁移也不中断。

### 规则集（rule_sets）

`routing.rule_sets` 声明 sing-box 二进制规则集（`.srs`），规则通过 `match.rule_set_ids` 引用。
该字段是 schema_version 1 内的增量字段。注意兼容性：旧核心（0.5.0 之前）忽略 `rule_sets` 与
`rule_set_ids`，于是只含 `rule_set_ids` 的规则在旧核心上是空 match，整份 Profile 会以 `RULE_MATCH_EMPTY`
被拒绝。后端只应向核心版本 ≥ 0.5.0 的客户端下发引用规则集的规则。

```json
"routing": {
  "rule_sets": [
    {"id": "cn-ip", "url": "https://<api host>/api/v1/proxy-profile/rule-sets/cn-ip.srs",
     "sha256": "<64 位 hex>", "update_interval_seconds": 86400}
  ],
  "rules": [
    {"id": "official-api", "match": {"domains": ["<api host>"]}, "action": {"type": "direct"}},
    {"id": "bypass-private", "match": {"ip_is_private": true}, "action": {"type": "direct"}},
    {"id": "<policy>", "match": {"rule_set_ids": ["<id>"]}, "action": {"type": "direct|proxy|reject", "target": "selected"}},
    {"id": "geoip-cn", "match": {"rule_set_ids": ["cn-ip"]}, "action": {"type": "direct"}}
  ],
  "final": {"type": "proxy", "target": "selected"}
}
```

| 字段 | 约束 |
| --- | --- |
| `id` | 必填，`[A-Za-z0-9][A-Za-z0-9._-]{0,127}`，在 `rule_sets` 内唯一（`RULE_SET_ID_INVALID` / `RULE_SET_ID_DUPLICATE`） |
| `url` | 必填，绝对 `https` URL，不含 userinfo/fragment（`RULE_SET_URL_INVALID`）；主机必须等于宿主下发 Profile 所用的 API 主机（`RULE_SET_HOST_NOT_ALLOWED`，见 core-api.md 的 `allowed_rule_set_hosts`） |
| `sha256` | 必填，文件内容的 SHA-256，64 位 hex（大小写均可；`RULE_SET_SHA256_INVALID`） |
| `update_interval_seconds` | 可选；缺省或 0 为 24 小时；负数拒绝（`RULE_SET_INTERVAL_INVALID`）；其余钳制到 [1 小时, 7 天] |

- 最多 32 个规则集（`RULE_SET_COUNT_INVALID`）。`rule_set_ids` 中的 id 必须存在（`RULE_SET_NOT_FOUND`）且
  在同一规则内不重复（`RULE_SET_REF_DUPLICATE`）。只含 `rule_set_ids` 的 match 是合法的。
- 文件格式：sing-box 二进制规则集，version ≤ 3（核心能读 sing-box 1.13 支持的全部版本，后端按 ≤ 3 生成）。
  单个文件不超过 32 MiB。
- 下载端点：`GET`，无需认证，返回 `application/octet-stream`；响应 `ETag` 为带引号的 sha256 hex
  （`"<hex>"`）。核心有旧副本时发送 `If-None-Match: "<旧副本 sha256>"`，内容相同时应返回 304。
  端点不得重定向（核心不跟随重定向）。
- 内容不可变：同一 `sha256` 对应的内容永远不变。更新规则集 = 在新 revision 中下发新的 `sha256`；
  核心只接受 SHA-256 等于 Profile 中 `sha256` 的内容。
- 语义：`rule_set_ids` 与 `domains`/`domain_suffixes`/`ip_cidrs`/`ip_is_private` 互为 OR，与
  `protocols`/端口为 AND（与既有语义一致）。
- 降级：规则集从未下载成功时（下载失败、主机未固定、无状态目录），核心照常启动，但该规则集被视为不可用：
  只依赖它的规则整条跳过；同时有其他地址 matcher 的规则保留其余 matcher。已有旧副本时继续使用旧副本。
  状态通过 `get-status` 的 `rule_sets` 上报。
- TUN 模式的 DNS：只含域名（不含 IP CIDR）的规则集按所在规则的动作镜像到 DNS 规则（direct → 系统解析器，
  proxy → 经节点的远程解析，reject → 拒绝）；含 IP CIDR 的规则集（如 cn-ip）不影响 DNS。
- 移动端 flow 分类器（`ClassifyFlow`）不解析规则集，始终按“不可用”处理。

## 协议约束

### Shadowsocks 2022

支持的方法和解码后的 Base64 密钥长度：

| method | `user_key` / 每个 `identity_keys` 元素 |
| --- | --- |
| `2022-blake3-aes-128-gcm` | 16 bytes |
| `2022-blake3-aes-256-gcm` | 32 bytes |
| `2022-blake3-chacha20-poly1305` | 32 bytes |

凭据字段按 SIP022 命名：

| 字段 | 含义 | 是否必需 |
| --- | --- | --- |
| `identity_keys` | 服务端的 identity PSK（iPSK）列表，用于 EIH（Extensible Identity Headers），按顺序由外到内（最外层中继的 iPSK 在前） | 可选；单用户、无 EIH 的 SS2022 可省略或为空数组 |
| `user_key` | 当前用户自己的 PSK（uPSK） | 必需 |

后端分别发送 `identity_keys` 和 `user_key`，不得预拼接。核心在内部构造 EIH 密码 `iPSK1:…:iPSKn:uPSK`（即 `identity_keys` 依次以 `:` 连接，最后接 `user_key`）；没有 `identity_keys` 时密码就是 `user_key`。

旧字段 `server_key` 已删除，不做兼容：Profile 只要在 `credentials.shadowsocks` 中带有 `server_key`，解析即以 `SHADOWSOCKS_SERVER_KEY_REMOVED` 拒绝（`field` 指向该 `server_key`）。旧版后端把 uPSK 放在 `identity_keys[0]`、把服务端 iPSK 放在 `server_key`，与核心的拼接顺序正好相反，服务端会报 `shadowsocks: invalid request`。

### VLESS + REALITY

- `uuid` 必须是 RFC 4122 形状、版本 1–5 的 UUID。
- 必须提供 REALITY；`public_key` 是 base64url（无填充）编码的 32 字节 X25519 公钥（`sing-box generate reality-keypair` 的输出格式）。
- 核心为 REALITY 启用 uTLS（`chrome` 指纹）；发布构建必须带 `with_utls` 构建标签（Makefile 默认）。
- `short_id` 是最长 16 个字符的偶数长度十六进制字符串，也允许空字符串。
- `tls.server_name` 是借用的第三方站点 SNI（如 `cloudflare-dns.com`），与 `endpoint.domain` 不同是正常的；只要求是合法域名，否则以 `TLS_SERVER_NAME_INVALID` 拒绝。

### AnyTLS

- `password` 非空。
- 必须提供 TLS，且 `tls.server_name == endpoint.domain`。

## revision 与更新策略

节点的 `id` 表示逻辑线路，入口（增删 backup、IP、端口、密钥）或展示信息变化时保持 ID 不变并生成新 `revision`。核心据此保留用户选择和本地代理端点，并在任一入口 endpoint 变化时产生 `NodeEndpointChanged` 事件。同一 revision 的重复下发是幂等操作，核心返回 `applied=false`。

后端不得下发 `inbounds`、`outbounds`、`route`、`clash_api`、sing-box tag、本地端口、TUN 设置、平台名称、系统代理设置或日志级别。
