# 贡献须知

## 敏感信息检查

本仓库公开。以下内容不得进入文件、提交信息或作者信息：

- 本机绝对路径（home 目录、`/private/tmp` 等）；
- 运行日志和证据目录；
- 本地敏感词文件里列出的内容：真实姓名、内部主机名和 IP、内部会话名、测试账号、内部域名等。

`tools/sensitive-check.sh` 负责检查，有两种模式：

```sh
tools/sensitive-check.sh tree               # 当前所有受跟踪文件
tools/sensitive-check.sh range BASE HEAD    # BASE..HEAD 新增的行、文件路径、提交信息和作者
```

命中时只输出 `path:line`，不输出命中的内容，因为 CI 日志是公开的。

**每个 clone 执行一次**，启用推送前检查（对 `origin/main..HEAD` 跑 range 检查，命中就拒绝推送）：

```sh
git config core.hooksPath .githooks
```

本地敏感词文件放在仓库外，不入库：默认 `~/.config/ppvpn-core/sensitive-patterns`，可以用
`$PPVPN_SENSITIVE_PATTERNS` 指定。格式是每行一个扩展正则，空行和 `#` 开头的行忽略。不要在 PR、提交信息或
评论里引用其中的内容。

私有网段不属于通用规则：文档和测试里有大量合法的私有地址（TUN 的 `10.60.159.90`、文档和测试网段）。
具体的内部地址写进本地文件。

确实需要保留的行，在行尾注明 `sensitive-check: allow`。

CI（`.github/workflows/sensitive.yml`）在每次推送和 PR 时运行：先做 tree 检查，再对本次新增的提交做
range 检查（推送用 before..after，PR 用 base..head）。CI 里没有本地敏感词文件，只按通用规则检查。
