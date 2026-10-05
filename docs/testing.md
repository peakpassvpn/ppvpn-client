# 测试分层

本仓库的测试分三层。层由测试需要的外部依赖决定：写测试之前先问一句「它要什么外部资源才能跑」，答案就决定它放在哪一层。每往上一层，多一级依赖：

| 层 | 名称 | 外部依赖 | 标记 | `cargo test` 默认跑吗 | 入口 |
|---|---|---|---|---|---|
| **L1** | 单元 | 无：内存里的假 runtime，不起 Sail，不碰网络，不要权限 | 无 | ✅ | `make test-unit` |
| **L2** | 集成 | 进程外的真实依赖，但不要权限：进程内的真 Sail 实例（只走回环地址和本地假节点）、平台的密钥存储 | Cargo feature `it-sail` | ❌ | `make test-integration` |
| **L3** | 系统 | 系统级资源，要特权：真 TUN、路由、ip rule、DNS、WFP，网络命名空间 | Cargo feature `system`（加 `#[ignore]`） | ❌ | `make test-system` |

层数的依据是依赖深度的真实台阶：L1 什么都不要；L2 要一个真的 Sail（或系统服务），但仍在一个普通用户进程里；L3 改的是机器本身的网络状态，必须隔离在命名空间里并在事后核对宿主没被改动。想加一层，先证明它在依赖上和上下两层都不重合。

**过渡期说明。** 现有测试还没有按层打标记（引擎组负责搬迁）。在那之前：

- `make test-unit` 就是 `cargo test --locked`，其中也包括今天已有的、在进程内起真 Sail 的测试（例如 `runtime::sail_tests`、`ppvpn-core-lab` 的测试），它们按定义属于 L2；
- `make test-integration` 目前只有 CLI 的密钥存储测试（`crates/ppvpn-cli/tests/keystore.rs`，Linux 的 Secret Service、macOS 的登录钥匙串）；
- `make test-system` 是 `tools/test-system-linux.sh`，跑的是已经用 `#[ignore]` 隔开的 netns 测试和网络变化脚本。

搬迁完成后，`cargo test` 只剩 L1，`it-sail` 打开 L2，`system` 打开 L3，入口不变。

---

## L1 单元

**位置：** 紧挨着被测代码，`foo.rs` 的测试写在 `foo_tests.rs` 或同文件的 `#[cfg(test)] mod tests`；跨模块的契约测试在 `crates/*/tests/`。

**约束：**
- 不启动 Sail 实例：用 `runtime::fake`（feature `testing`）或纯函数；
- 不监听真端口、不连网络、不读环境里的网络配置；
- 不要 root，不改任何系统状态；
- 可以读 `testdata/`（Profile、golden、规则集）。

**写什么：** 默认写在这里。Profile 的严格解析和校验、翻译出的配置、golden（`testdata/golden`）、错误码和事件的契约、Engine 的状态机（用假 runtime）、CLI 的文本和 JSON 输出。

**反模式：**
- ❌ 在 L1 里起 Sail 或绑真端口——那是 L2；
- ❌ 用 `sleep` 等后台任务——用假时钟或显式的通知。

## L2 集成

**位置：** 与被测的 adapter 同模块（例如 `runtime/sail_tests.rs`），或 `crates/*/tests/`。代码用 `#[cfg(feature = "it-sail")]` 圈起来。

**约束：**
- 真 Sail 实例只在当前进程里，只监听回环地址；
- 对端是本地起的服务：**用 Sail 自己的入站在回环上做协议服务端**（Shadowsocks 2022、VLESS + REALITY、AnyTLS），或 `test/` 里的 Go 辅助程序；不用 Docker，不连外网；
- 不要 root，不改宿主网络；
- 每个测试自己的 `state_dir`、端口用 0 让系统分配，测试之间互不依赖。

**写什么：**
- 运行时适配层：启动、reload、停止、事件、遥测，对真 Sail；
- 三种协议的互通和路由判定（原来 Docker 实验网 `test/lab/engine/cases` 里的用例搬到这里，对端改成 Sail 入站）；
- 平台密钥存储（Secret Service、钥匙串）。

**反模式：**
- ❌ 用 L2 测参数校验和错误码——那是 L1；
- ❌ 依赖外网或真实节点——那是人工验收，不是测试层；
- ❌ 需要 TUN 或特权——那是 L3。

## L3 系统

**位置：** `runtime/netns_tests.rs`、`tunrules/linux_tests.rs`、`runtime/windows_tests.rs`，桌面端的 `desktop/crates/ppvpn-client/src/e2e_linux.rs`；脚本在 `test/netns/` 和 `test/lab/localdns/`。代码用 `#[cfg(feature = "system")]` 圈起来，测试本身 `#[ignore]`，所以普通的 `cargo test` 永远不碰宿主的路由。

**约束：**
- Linux：每个测试在自己的网络命名空间里跑（`test/netns/run.sh`），前后核对宿主的路由、规则、网卡、nftables 和 DNS，任何差异都算失败；
- Windows：管理员身份，TUN 只路由 `198.18.0.0/16`，runner 自己的流量不受影响，前后核对适配器、路由、DNS 和 WFP 过滤器；
- 需要故障注入时用 feature `fault-injection`，只有测试构建打开它，发行构建拒绝它（`tools/release-features-check.sh`）。

**写什么：** TUN 的策略路由守护、网络变化（dns-local 跟随默认网卡、断网期间不切内核，严格模式 `SWITCH_GRACE_MS=0`）、实例失败和被 kill -9 之后的清理与重建（G7）、桌面增强模式端到端。

**目标：** 少而精。L3 的一条用例抵得上几十条 L1，验证的是"串起来没串味"，不是覆盖每个分支。

**反模式：**
- ❌ 用 L3 测本可以在 L1、L2 验证的逻辑；
- ❌ 在宿主命名空间里改任何东西。

---

## CI 什么时候跑哪一层

只有两个 workflow：`.github/workflows/ci.yml` 在 PR 推送时跑全部测试，`.github/workflows/release.yml` 在合入 main 时构建安装包（见 [release.md](./release.md)）。没有合并队列，没有定时运行。

`ci.yml` 在每个 PR 的每次推送上并行跑 7 个 job。命名规则：与平台无关的 job 按它检查的内容命名（实际跑在 ubuntu 上，只是实现细节）；平台 job 以系统命名，只覆盖该系统自己的代码。

| job | 内容 |
|---|---|
| `lint` | 敏感信息检查（tree 和新增提交）、`cargo fmt`、clippy、Sail 的 patch 与 lock 核对、发行 feature 检查 |
| `unit` | L1（`make test-unit`） |
| `integration` | L2（`make test-integration`），与 L1 并行 |
| `desktop` | 桌面各层：特权 service 和客户端 crate 的测试与 clippy、桌面脚本、C# 绑定和 App.Core / Linux 测试、Swift 绑定和 AppLogic 测试 |
| `linux` | Linux 自己的：网络命名空间里的 L3（`make test-system`：tun 及 G7、network-change），以及桌面增强模式端到端 |
| `macos` | macOS 自己的：整个 workspace 在 aarch64 和 x86_64 上的 clippy、macOS 平台测试、Debug app 构建 |
| `windows` | Windows 自己的：桌面链接的 crate 在 MSVC 上的 clippy、Windows 平台测试、G7 Windows、C# 绑定、App.Core 测试、app 和推送代理构建 |

L1 和 L2 只跑一遍（`unit`、`integration`）；`macos`、`windows` 不重跑，只编译（clippy 覆盖各平台 `cfg` 的代码）并跑平台测试。

只改了 `docs/` 或 Markdown 的 PR，7 个 job 照样运行并报告，但都跳过构建和测试步骤（`tools/ci-changes.sh` 的 `code`）；`lint` 仍做敏感信息检查。

手动重跑：`gh workflow run ci.yml --ref <分支>`（`windows` 的 G7 Windows 这时跑 5 遍）。

### 平台测试

只对某一个系统有意义的测试，要么放在以该系统命名的模块里（`darwin`、`windows`、`windows_tests`），要么列在下表里。`macos` 和 `windows` 只按这些过滤条件跑测试（`cargo test -p <crate> -- <过滤条件>`）；新增平台测试时改这张表和 `ci.yml` 里对应的那一步：

| crate | macOS | Windows |
|---|---|---|
| `ppvpn-core` | `hostipv6::darwin`、`localdns::source`、`probe::icmp`、`runtime::sail_tests`（Sail 的网卡监视器按平台实现，整个模块在 macOS 上再跑一遍） | `hostipv6::windows`、`localdns::source`、`probe::icmp`；`runtime::windows_tests`（feature `fault-injection`，管理员，G7 Windows） |
| `ppvpn-cli` | `identity`；`--test keystore`（登录钥匙串，`PPVPN_TEST_KEYSTORE=1`） | 不支持 Windows |
| `ppvpn-client` | `detect`、`service`、`sysproxy` | `detect`、`service`、`sysproxy` |

### 缓存

PR 只读缓存，不写。缓存在合入 main 时由 `release.yml` 的 `cache-*` job 保存，而且只在 `Cargo.toml`、`Cargo.lock`、工具链或共享 action 变了的时候（`tools/ci-changes.sh` 的 `deps`）。每个缓存键（`shared-key`）对应一个 `cache-*` job：`linux`（`lint` 和 `linux` 共用）、`unit`、`integration`、`desktop`、`macos`、`windows`。rust-cache 只缓存依赖，所以依赖不变时，旧缓存一直有效。

## 不是测试层的东西

- **切换前的一次性验收**：G4 弱网、G5 三端实机、G6 24 小时长跑、G9 团队自用一周。它们在测试机上（hostq）或真机上手动执行一次，结果记在 #214，不进 CI。需要 Go 0.5.21 作对照时，用 v0.5.21 Release 的二进制。
- **性能数字**：`tools/perf`，按需在测试机上跑，不进 CI。
- **Sail 与 sing-box 的逐项对比（repro）**：属于 Sail 自己的 CI。本仓库只用钉住的 Sail 提交跑自己的 L1–L3。
- **真实节点**：人工验收，见 #214 的验收清单。

## 测试用的 Go 辅助程序

`test/go.mod` 是一个只给测试用的 Go module，不进入任何发行物：

- `test/fakenode`：桌面增强模式端到端用的 Shadowsocks 2022 节点；
- `test/lab/localdns/ldnslab`：网络变化脚本用的 DNS 辅助程序；
- `test/perf/*`：性能检查的假节点和负载；
- `test/lab/engine/mksrs`：实验网的规则集生成。

`make test-tools` 把前两个构建到 `build/test-tools/`。

`crates/ppvpn-core/src/rulesets/testdata/gen.go` 生成规则集测试用的 `.srs` 文件（已提交，很少需要重新生成）。它不在这个 module 的目录里，而且按当前目录写出 `crates/ppvpn-core/src/rulesets/testdata/<名字>`，所以在 `test/` 里用这个 module 的依赖运行，再把生成的文件移回去：

```sh
cd test
mkdir -p crates/ppvpn-core/src/rulesets/testdata
go run ../crates/ppvpn-core/src/rulesets/testdata/gen.go
mv crates/ppvpn-core/src/rulesets/testdata/*.srs ../crates/ppvpn-core/src/rulesets/testdata/
rm -r crates
```
