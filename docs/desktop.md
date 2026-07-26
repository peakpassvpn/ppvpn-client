# Desktop 平台接入

桌面产品只依赖版本化的 ppvpn-core 公共契约，不感知或选择 core 的内部实现。

## Windows

特权 `ppvpn-service` 作为唯一 runtime owner 启动 Windows x64 core 制品，先读取
`version` 并要求 Core API v1、Profile Schema v2。service 负责权限、TUN、路由、DNS、
进程校验和控制通道；Profile 的 `DIRECT`、`REJECT`、selected/fixed-node `PROXY`
语义只由 core 判定。

每节点本地代理由同一 runtime 创建：一个 node ID 对应一个 loopback mixed
listener，同一端口支持 HTTP 和 SOCKS5 并固定走该节点。service 可读取完整 endpoint，
但发往 WebView 的 DTO 只能使用不含 secret 的 metadata；credential 只进入原生凭据
面板调用栈。

## macOS

`build/PPVPNCore.xcframework` 提供 macOS 13+ universal slice，运行在
`NETransparentProxyProvider` System Extension 进程内。公开 Objective-C API 包括：

- `MobileBridge.start/applyProfile/stop/status`
- `classifyFlow`：只读已编译规则快照，不做 host、磁盘或网络 I/O
- `openFlow(flowJSON, decisionJSON, timeoutMS)`：验证首次决策的 snapshot/HMAC 后，为其中
  固定的 node 打开 PROXY TCP/UDP outbound；不得按当前 selected 重新分类
- `MobileFlowConnection.read/write/close`
- `localProxyMetadata` 与 `localProxyCredential`

Provider 把 Apple flow 的 hostname、目标 IP、端口和 TCP/UDP 编码成严格 JSON DTO。
`DIRECT` 返回系统处理，`REJECT` 由 Provider 关闭，`PROXY` 在后台把首次 decision 原样
交给 `openFlow` 并负责双向复制、背压与关闭。`handleNewFlow` 回调内只能调用
`classifyFlow`，不能同步拨号。selected 在两次调用之间切换时，`openFlow` 仍执行首次
decision 的 node；Profile snapshot 已替换时则拒绝旧 decision。

Objective-C module 名为 `PPVPNCore`；生成头文件中的关键签名是：

```objc
- (NSString *)classifyFlow:(NSString *)flowJSON error:(NSError **)error;
- (MobileFlowConnection *)openFlow:(NSString *)flowJSON
                      decisionJSON:(NSString *)decisionJSON
                         timeoutMS:(long)timeoutMS
                             error:(NSError **)error;
- (NSData *)read:(long)maxBytes timeoutMS:(long)timeoutMS error:(NSError **)error;
- (BOOL)write:(NSData *)data timeoutMS:(long)timeoutMS error:(NSError **)error;
```

UDP 的一次 `read`/`write` 对应一个完整 datagram。若调用方 read buffer 太小，core 消费该
datagram 并显式返回错误，不会把截断数据当成功结果；单次写入上限为 65507 bytes。
FlowConnection 的 `timeoutMS <= 0` 表示不安装 I/O deadline，适合长连接空闲等待；
正值最大 120 秒。`openFlow` 的拨号 `timeoutMS <= 0` 仍使用 15 秒默认值。

## 固定优先级

1. 平台安全、控制通道和防递归
2. 每节点 local-proxy 固定节点
3. Profile v2 ordered rules
4. `routing.final`

selected-node 切换只影响新 flow；Profile revision 更新走候选构建、受控替换和失败回滚。
