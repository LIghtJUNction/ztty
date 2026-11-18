# ztty 0.3.1 正式应用发布验收

报告日期：2026-11-19（Asia/Taipei）。GitHub 原始时间使用 UTC。

## 结论与发布身份

**ztty 0.3.1 已正式发布，GitHub 附件读回和 crates.io 源身份校验均通过。** 发布于 2026-11-18 21:49:22 UTC，即台北时间 2026-11-19 05:49:22。核验时 Release 为公开状态并成为 latest；原始标签和被验收的源码保持不变。[正式 Release](https://github.com/LIghtJUNction/ztty/releases/tag/v0.3.1) · [恢复运行](https://github.com/LIghtJUNction/ztty/actions/runs/44402524733)

| 身份 | 固定值 |
| --- | --- |
| 标签 | `v0.3.1` |
| annotated tag object | `783178f40c4f732c3520c36e2690ba97b027b852` |
| 应用源码 commit | `cbcd480f861904a2265af52ab5ca8a5e19cebe9d` |
| Release ID | `450584279` |
| 最终状态 | `draft=false`；944 个附件 |

源包按该固定源码的原配方 `git archive --format=tar --prefix="ztty-0.3.1/" HEAD | gzip -n` 生成。[ztty-0.3.1.tar.gz](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-0.3.1.tar.gz) 为 4,440,076 B，SHA-256 为 `29383193487de4954081ed1a195f4031cd7756de6d547b262f423b97a22ec043`。标签身份、源包及逐附件摘要可以交叉核对。[标签 API](https://api.github.com/repos/LIghtJUNction/ztty/git/ref/tags/v0.3.1) · [完整恢复审计](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-0.3.1-recovery-44402524733-1.json)

## 原始门禁与恢复结果

原运行 `44375302476` 的 Linux、macOS、Windows 各两种架构，共六个原生门禁，以及 Verify 均通过；版本、Arch 包测试和平台文件来源账本检查也通过。该运行最后上传 0 B 的 `ztty-macos-aarch64.native.application.stderr.log` 时收到 HTTP 400 `Bad Content-Length`，因此原运行仍保留为 failure。[原运行](https://github.com/LIghtJUNction/ztty/actions/runs/44375302476) · [原发布失败 job](https://github.com/LIghtJUNction/ztty/actions/runs/44375302476/job/135065712440)

恢复运行 `44402524733` 已完成并返回 success。它重新审计原运行与六个原始 Actions artifact，下载校验 943 个非空发布附件后才将 draft 公开，再检查发布前后附件 ID 一致，并上传、读回完整审计 JSON。因此最终附件数为 943 + 1 = 944。恢复没有重跑六平台构建，也没有再次执行 `cargo publish`。第一次恢复 `44402143648` 因下载请求的媒体类型导致只读审计 HTTP 415，发布步骤未执行；该失败记录同样保留。[最终恢复 job](https://github.com/LIghtJUNction/ztty/actions/runs/44402524733/job/135149055985)

空日志以原始内容保留在非空证据 ZIP 中：其中包含六个原 Actions ZIP 的完整原始字节，并验证了 10 个空文件；没有上传独立的 0 B 附件。证据归档和校验值如下。

| 证据 | 大小 | SHA-256 |
| --- | ---: | --- |
| [ztty-0.3.1-native-evidence.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-0.3.1-native-evidence.zip) | 5,368,273 B | `97728b14a6c1e261bd4d1a6c4eced77ef7cf552e4dbb4e7dbef23033f8625ec9` |
| [ztty-0.3.1-recovery-44402524733-1.json](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-0.3.1-recovery-44402524733-1.json) | 205,547 B | `ca57386c2c4185005c784052dc8448300645b213cdbbc94aab5cbbe2ba3adce1` |

完整工作流证明另存为 [Actions artifact 14473219084](https://github.com/LIghtJUNction/ztty/actions/runs/44402524733/artifacts/14473219084)，大小 28,478,522 B，SHA-256 为 `32021ce9c3386835ce95a0b53991854f79ac47810b45afd4a432f0c880bcfdee`，到期时间为 2026-12-02 21:49:30 UTC。上表两份 Release 附件独立于 Actions 的这段保留期。

## 六平台主要下载

下列包均来自原验收通过的固定源码；文件大小、SHA-256 和原门禁 job ID 同时记录在配套 JSON 及完整恢复审计中。

| 平台 | 架构 | 下载 | 大小 |
| --- | --- | --- | ---: |
| Linux | x86_64 | [ztty-linux-x86_64.tar.gz](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-linux-x86_64.tar.gz) | 361,782 B |
| Linux | aarch64 | [ztty-linux-aarch64.tar.gz](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-linux-aarch64.tar.gz) | 344,814 B |
| macOS | x86_64 | [ztty-macos-x86_64.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-macos-x86_64.zip) | 185,999 B |
| macOS | aarch64 | [ztty-macos-aarch64.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-macos-aarch64.zip) | 170,034 B |
| Windows | x86_64 | [ztty-windows-x86_64.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-windows-x86_64.zip) | 840,599 B |
| Windows | aarch64 | [ztty-windows-aarch64.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.3.1/ztty-windows-aarch64.zip) | 772,809 B |

## crates.io 只读回读

从官方索引和归档地址读回的 `ztty 0.3.1` 已通过校验，状态为 `verified`，未被 yank。检查只读取现有公开包，没有重新上传。[https://static.crates.io/crates/ztty/ztty-0.3.1.crate](https://static.crates.io/crates/ztty/ztty-0.3.1.crate) · [官方索引](https://raw.githubusercontent.com/rust-lang/crates.io-index/master/zt/ty/ztty) · [实际审计日志](https://github.com/LIghtJUNction/ztty/actions/runs/44402524733/job/135149055985)

| 项目 | 实际结果 |
| --- | --- |
| 归档大小 | 4,254,590 B |
| SHA-256 | `86d4bb0683f7809f7bec61783b50423554e9d29e4ceb3aca26e625055920716b` |
| `.cargo_vcs_info.json` 的 `git.sha1` | `cbcd480f861904a2265af52ab5ca8a5e19cebe9d` |
| `path_in_vcs` | 空字符串，仓库根目录 |
| `git.dirty` | 原字段不存在；校验器按缺省 `false` 判定，未声称文件显式写有 `dirty:false` |
| 规范化依赖 | `zterm = 0.3.0`、`zpty = 0.2.1`，均使用默认 registry |
| UI 排除 | 无 `ztty-ui` 依赖，归档路径中无 `ztty-ui` 目录组件 |

## 适用范围与后续流程

本次正式发布对象为 ztty 原生终端应用，ZUI/1 实验分支的迭代和 Linux 原生界面验收不计入此发布结论。后续发布流程的空日志修复已形成 commit [`99d9463`](https://github.com/LIghtJUNction/ztty/commit/99d94633391150c4ca431b568c27139ef63441dd)，其验收见[发布流程记录](release-assets-validation-2026-11-19.zh-CN.md)；它不修改 `v0.3.1` 的源码或标签。

本机会话已验收的安装仍为 `0.3.1-2`，其源码包含后续修改，与本报告的固定发布源码不同。GitHub 恢复只处理远端发布和读回证据，没有安装软件或将本机降级；本机安装状态沿用会话已有的验收记录，不将其等同于本次 Release 的二进制。 开发机连接中断后未重新现场核验，安装结论来自[已留存的安装验收](https://github.com/LIghtJUNction/ztty/blob/cf7158bc6696cb246184f4df3d6850a6c1f8994b/docs/measurements/atomic-frame-2026-11-19/installation-0.3.1.json)。
