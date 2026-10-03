# 开发文档

| 文档 | 内容 |
|------|------|
| [ci.md](./ci.md) | `desktop.yml` 的各个 job、触发条件、vendored core 的 manifest 与下载校验 |
| [baseline-go-0.5.21.md](./baseline-go-0.5.21.md) | Go core 0.5.21 的安装包大小与内存基线（切换到 Rust 引擎时的对比基准，#214） |
| [qa-checklist.md](./qa-checklist.md) | 各平台的真机 QA 清单 |

构建见 [../BUILD.md](../BUILD.md)，平台细节在 `apps/<platform>/README.md`，第三方组件见
[../THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md)。引擎本身（Core API、Profile、Rust 版行为对照）
的文档在仓库根的 `docs/`。
