# ztty 0.4.1：小数字键盘修复与发布验收报告

日期：2026-11-19（UTC）  
结果：小键盘输入修复、远程 PR 合并、正式版本发布和开发机升级均已完成。

## 1. 交付结果

正式版本为 **ztty 0.4.1**。GitHub Release 已公开，Linux、macOS、Windows 的 x86_64 与 ARM64 共六个平台完成正式验收；发布流程中的 Verify、附件下载回读和 AUR 更新均成功。开发机已安装 `ztty 0.4.1-1`，原有 5 个终端进程在安装后全部存活。

- [下载 v0.4.1](https://github.com/LIghtJUNction/ztty/releases/tag/v0.4.1)
- [正式发布流程及各平台日志](https://github.com/LIghtJUNction/ztty/actions/runs/44474100248)
- [机器可读验收记录](./numpad-release-0.4.1-2026-11-19.json)
- [本次发布源码](https://github.com/LIghtJUNction/ztty/tree/v0.4.1)

本轮已合并的 PR：

| PR | 内容 | 状态 |
| --- | --- | --- |
| [#11](https://github.com/LIghtJUNction/ztty/pull/11) | 实验性 ZUI/1 协议与 Linux 原生 UI 集成 | 已合并 |
| [#15](https://github.com/LIghtJUNction/ztty/pull/15) | 小键盘编码修复、回归测试及 0.4.0 集成 | 已合并 |
| [#16](https://github.com/LIghtJUNction/ztty/pull/16) | 发布附件收敛、可恢复上传及 0.4.1 | 已合并 |

报告整理时再次查询，仓库没有待合并的开放 PR。

## 2. 输入为何失效，修复改变了什么

终端收到键盘事件后，要把它转换成应用能读取的字节。普通数字通常应发送字符本身；例如输入数字 1，应用收到一个字节 `0x31`，便能将它作为文字插入。方向键等没有直接文字含义的按键，才需要用转义序列表达。增强键盘协议允许应用进一步要求按键身份、修饰键和事件类型。

旧实现把“小键盘按键”本身当成启用增强编码的充分条件。应用只开启 Kitty 协议的消歧义选项时，小键盘数字也被编码成私用按键编号。对仍按普通文字处理这些按键的应用，数字就无法正常输入。问题位于共享输入编码库 `crates/zinput/src/lib.rs`，修复覆盖使用该库的终端输入路径。

已用开发机原先安装的 0.3.1，在独占的 Xvfb、真实 X11 窗口和原始 PTY 中，通过 XTEST 注入小键盘事件复现：

| 情形 | 观察结果 |
| --- | --- |
| 传统输入模式 | 小键盘数字能产生文字 |
| Kitty flags=1，输入 `0123456789.+-*/` | 原版本错误地产生一串 CSI-u 小键盘事件 |
| 数字 1 的具体差异 | 期望 `0x31`，旧版发送 `ESC[57400u` |
| 整串期望字节（十六进制） | `303132333435363738392e2b2d2a2f` |

修复后，**NumLock 开启、DECKPAM 关闭，且系统将按键解析为可打印文本时**，普通小键盘数字、小数点和运算符的按下及重复输入，在测试的 Kitty flags 1–7 下继续发送字符。功能键及应用明确请求的增强行为保留：

| 模式或输入 | 修复后的行为 |
| --- | --- |
| 普通小键盘文本，flags 1–7 | 输入对应字符 |
| flags 8：报告全部按键 | 保留小键盘按键身份 |
| flags 24：8 + 16 | 报告身份，并携带关联文本 |
| 显式 DECKPAM 应用键盘模式 | 保留应用键盘转义序列 |
| Ctrl、Alt、Super、Hyper、Meta 等组合键 | 保留既有修饰键语义 |
| NumLock 关闭后的导航键 | 保留导航语义 |

上述文字规则不包含 Keypad Enter，也不意味着释放事件会再次输入字符。Shift 可能改变操作系统对数字键和导航键的解析，不能将普通数字规则扩大到所有修饰键组合。

实现与测试见 [zinput](https://github.com/LIghtJUNction/ztty/blob/v0.4.1/crates/zinput/src/lib.rs) 和 [输入 GUI 回归](https://github.com/LIghtJUNction/ztty/blob/v0.4.1/tests/input_gui.py)；协议语义参见 [Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)。

## 3. 验证结果与源码对应关系

测试记录区分“修复后的开发提交”“正式发布提交”和“实际下载产物”，避免把不同阶段的成功混作同一次测试。

| 验证项 | 结果与范围 |
| --- | --- |
| zinput 回归 | 21 个单元测试、2 个文档测试通过；增加 3 个针对性测试 |
| Rust 工作区与严格 Clippy | 修复后的开发提交 `be8c2bc` 上全部通过 |
| 最终本地构建 | locked metadata、release build、no_std 检查通过 |
| Linux 原生窗口验收 | 7 套全部通过：工作区、日常操作、会话切换、Sixel、输入、重绘、原生 UI |
| 输入与缩放 | 70 个输入向量、14 个缩放用例通过；原输入向量数为 59 |
| UI 与重绘 | 原生 UI 24 项、重绘 11 项通过 |
| 裸机约束 | 集成阶段的 `x86_64-unknown-none` 检查及无操作系统 ELF 验证通过 |
| 发布附件契约 | 15 个打包检查、12 个上传检查通过 |
| 已发布库的消费验证 | 5 个独立消费者及组合消费者均从 crates.io 获取实际版本并通过检查 |
| 最终应用的消费验证 | 从 crates.io 隔离安装 `ztty 0.4.1` 成功，无本地路径依赖 |
| 正式流水线 | 最终提交上 Verify、六个平台原生任务、publish 全部成功 |
| 公开 Linux 下载 | 实际下载并运行，报告 `ztty 0.4.1`，哈希与正式原生验收记录一致 |

最终本地验收提交为：

```text
2df8ff229d270e45815a470c1bb9e61844186206
```

合并后发布提交及不可变标签为：

```text
v0.4.1 -> 6b91e29e923aaee0207953f2b04393087f19e0d8
tree      58979bd10bb13b8ff0cdb08dfb4104094c3e0e54
```

已核对两个提交的源码树完全一致。正式 CI 在合并提交上重新执行验收，发布产物的 source metadata 也绑定该提交。实际从 crates.io 下载的 `ztty-0.4.1.crate` 内 VCS 记录同样指向该提交。

本地 Linux GUI 验收采用合成 XTEST 事件、真实 PTY/X11 和窗口像素读取。它证明受测输入路径及窗口行为通过，不等同于物理键盘、硬件输入法、原生 Wayland 或物理显示延迟测试。本轮没有修改 ssmcp 源码，也没有将协议复现扩大表述为 ssmcp 的物理键盘端到端验收。

详细测试结果、源码 SHA、公开附件摘要和安装校验均见本报告配套 JSON；完整跨平台原始证据可下载 [native-evidence.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.4.1/ztty-0.4.1-native-evidence.zip)。

## 4. 为什么最终发布为 0.4.1

首次 0.4.0 正式流程中，Verify、六个平台原生验收以及 Arch 包检查都通过。发布阶段把截图和日志逐个作为附件上传，合计 941 个原生附件，加源码包与校验文件共 943 个，触发 GitHub 的 HTTP 403 secondary rate limit。失败发生在附件发布阶段，不能将当时的草稿视为已完成公开发布。

0.4.0 的标签、源码和已发布 crates 保持不变。修复发布流程后，应用版本递增为 0.4.1，并执行新的完整正式发布。

0.4.1 的具体调整：

1. 对独立下载附件采用明确清单，保留各平台程序、校验和、构建与验收元数据。当前共 47 个附件，并设置最多 64 个的硬限制。
2. 将完整原始证据放入可复现 ZIP，保留 949 个文件，包括 10 个空日志；逐文件验证长度、哈希和精确成员集合。
3. 必须单独下载的分发文件仍需独立存在，不能用证据 ZIP 掩盖缺少的公开附件。恢复仅限经过全部验证的原始证据文件，并拒绝覆盖和符号链接。
4. 上传前核对远端版本身份及现有附件。按顺序上传缺少的文件，写操作之间至少间隔 1 秒；遇到错误停止，不在限流错误上循环重试，也不覆盖既有附件。
5. 正式发布前回读公开附件并核验。已发布版本的核验路径不进行额外写入。

先使用原 0.4.0 六个平台的已验收产物做离线演练：保留并核对 949 个文件，恢复 906 个仅存于归档中的证据文件，完成 936 次平台清单文件校验。随后 0.4.1 正式流程实际上传并回读 47 个附件，949 个证据文件全部通过。

最终证据 ZIP 的 SHA-256：

```text
e25a56822929d46857585784058081a173044fb4f1cdb06092954c1777ac0621
```

实现见 [release_assets.py](https://github.com/LIghtJUNction/ztty/blob/v0.4.1/tools/release_assets.py)、[release_upload.py](https://github.com/LIghtJUNction/ztty/blob/v0.4.1/tools/release_upload.py) 和 [发布说明](https://github.com/LIghtJUNction/ztty/blob/v0.4.1/docs/releasing.md)。请求节奏参考 [GitHub REST API 最佳实践](https://docs.github.com/en/rest/using-the-rest-api/best-practices-for-using-the-rest-api)。

## 5. crates.io 与开发机安装

| 包 | 最终验证版本 | 本轮情况 |
| --- | --- | --- |
| [ztty](https://crates.io/crates/ztty/0.4.1) | 0.4.1 | 已发布，并完成实际 registry 安装 |
| [zterm](https://crates.io/crates/zterm/0.4.0) | 0.4.0 | 已发布并验证消费 |
| [zinput](https://crates.io/crates/zinput/0.1.2) | 0.1.2 | 已发布，包含小键盘修复 |
| [ztty-ui](https://crates.io/crates/ztty-ui/0.1.0) | 0.1.0 | 已发布并验证消费 |
| [zgraphics](https://crates.io/crates/zgraphics/0.1.1) | 0.1.1 | 版本不变，重新验证消费 |
| [zpty](https://crates.io/crates/zpty/0.2.1) | 0.2.1 | 版本不变，重新验证消费 |

开发机已通过 pacman 安装 `ztty 0.4.1-1`。`/usr/bin/ztty --version` 返回 `ztty 0.4.1`，包完整性检查为 **9 个文件、0 个变更**。用户启动器仍指向 `/usr/bin/ztty`。安装的是本地构建、通过完整 Linux 原生验收、与发布提交源码树一致的 ELF；不同构建环境产物分别记录哈希。

安装程序 SHA-256：

```text
d1276d06108a8f23bc2b088078ebdbf6a72d50505fb2c5b43aeff342b9c12335
```

原 0.3.1-2 的可安装回滚包已保存并逐字节核验，位置为：

```text
~/Backups/ztty/numpad-0.4.0/ztty-0.3.1-2-x86_64.pkg.tar.zst
```

安装前的 5 个终端进程均未被终止。三个本轮审查 worktree 已在合并内容核对后移除，验收证据继续保留。

## 6. 立即在新版窗口中使用修复

ztty 的后台 broker 管理窗口和 PTY。替换磁盘上的程序不会替换已经运行的 broker；普通启动还可能连接到旧 broker。因此，已有窗口不会因这次安装自动切换到新代码，`ztty --version` 也只说明所执行文件的版本。

可以在 Bash 或 zsh 中运行：

```bash
ZTTY_SOCKET="@ztty-v041-$(id -u)-$$" /usr/bin/ztty --name v041
```

当这个独立地址尚无 broker 时，命令会启动新版 broker 和新窗口，现有终端及 ssmcp 会话继续运行。新窗口中的进程会继承该地址；从其中继续开窗口可省略 `--name v041`，避免同一地址内重名。该操作新建会话，不迁移旧 PTY。

从其他终端使用 `list`、`attach` 或 MCP 连接这组新会话时，要指定同一个具体地址。不同 shell 的 `$$` 不同，重新计算并不一定得到刚才的地址。broker 行为见 [README](https://github.com/LIghtJUNction/ztty/blob/v0.4.1/README.md) 与 [broker.rs](https://github.com/LIghtJUNction/ztty/blob/v0.4.1/src/broker.rs)。
