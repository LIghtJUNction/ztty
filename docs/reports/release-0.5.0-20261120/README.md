# ztty 0.5.0 正式发行记录

**ztty 0.5.0 已于 2026-11-20 03:41:44 UTC 正式发布。**
[Release](https://github.com/LIghtJUNction/ztty/releases/tag/v0.5.0) 不是 draft 或 prerelease；
[正式工作流 44668398577](https://github.com/LIghtJUNction/ztty/actions/runs/44668398577)
在第一次 attempt 完成，九个 job 全部 success。六平台的实际原生验收与发行制品在本页单独记录，
没有沿用此前 edae 的六平台结果。

## 固定来源

| 项目 | 实际身份 |
| --- | --- |
| 正式源码 / tag 指向 | `c4f8f48c9598284a589e609f25242b49ceabfea8` |
| annotated tag 对象 | `4288d646b3273285483cf1fdad0ccc8e0c42b9ae`；未作密码学签名 |
| Release ID / run / attempt | 452238617 / 44668398577 / 1 |
| 正式输入 | `workflow_dispatch`，`build_only=false`，`candidate_platform=all` |
| 公开资产 | 47 个，合计 28,446,629 B；全部实际 HTTP 200 下载并核对 size/digest |

c4 的父提交为生产检查点 9855，新增范围只有前一 9f 的性能文档；生产代码、工具、测试与 workflow 相同。
[9855 功能/性能报告](../host-9855da5-20261120/README.md)保留自己的源码、ELF 和测量条件。
正式发行的二进制由各平台 runner 构建，其身份如下；本地性能数字没有被改写成六种制品各自的测量结果。

[总审计回执](audit/verification.json)固定了来源、工作流、资产、归档与 AUR 证据。
其中 13,751 项是下载/哈希/结构核验断言，不是新增产品测试数。

## 六个平台实际验收范围

| 平台 | 本次实际结果 | 运行环境与边界 |
| --- | --- | --- |
| Linux x86_64 | 278 GUI；另 6 EGL、17 driver、4 cwd；198 input vectors | Ubuntu 24.04、私有 Xvfb/headless Sway；GPU 为 llvmpipe LLVM 20.1.2 / 256 bits |
| Linux aarch64 | 同上，各项在 ARM runner 实际执行 | Ubuntu 24.04 ARM；llvmpipe LLVM 20.1.2 / 128 bits |
| macOS x86_64 | 24 checks、13 stages、10 张像素 PNG | AppKit/Darwin PTY；实际 GUI 在 ad-hoc 签名前执行 |
| macOS aarch64 | 同上，各项在 ARM runner 实际执行 | AppKit/Darwin PTY；未测试交互 IME keyboard |
| Windows x86_64 | 31 contracts，8 次 cold start 逐次通过；另 workspace 22、fixture contract 41 | ConPTY、DIB 与私有 desktop；maximize 保留 PTY 身份已通过 |
| Windows aarch64 | 同上，各项在 ARM runner 实际执行 | 同样保留全部冷启动、workspace、清理原件 |

Linux 的 278 GUI = base 101 + GPU text 8 / UI 24 / repaint 11 + retained-frame 2 +
graphics 8×2 + OSC52 13×2 + clipboard 25 + Wayland 65。
base 内的 input 14 checks 对应 198 个向量；向量数不再加作 198 项独立 GUI 契约。
Windows 的八轮冷启动反复执行相同 31 项，不合计成 248 种不同功能。
各平台自有进程清理结果均为空。

[native-platforms.json](audit/native-platforms.json)保留每项原始数量、runner、renderer 和身份；
六份 build/native 原报告位于 [native](native/)，Linux 的 20 份补充 GUI JSON 也已归档。
Linux headless Wayland 与软件 EGL 证明对应协议/像素路径，未测物理桌面、显示扫描或硬件输入延迟。
不能把本地主机 LLVM 23.1.1 或另一检查点的 NVIDIA renderer 套用到这次 CI。

### 测试二进制与打包身份

| 制品 / 阶段 | SHA-256 |
| --- | --- |
| Linux x86_64，native 测试及安装烟测 ELF | `3ed8cead1c4ca20aedd4aa262de8fe992bcf9639546d95f89f7a782eeef7a491` |
| Linux aarch64，native 测试 ELF | `39ced58ddb1f3fd2abb2558d33949a24dc6bf8801e3809cbbda187090c10057d` |
| Windows x86_64，包内 ztty.exe | `b415dae9aea8f59e9caf6784b09e5951efb0e3bfa0a6178990ea51ff631ebdcd` |
| Windows aarch64，包内 ztty.exe | `bba1525b2aef06d80ca4950bd00e94ad1d20e1e694b1fca2658929b38d9cbff7` |
| macOS x86_64，native 测试 / 签名后包内二进制 | `6416aa3724fe5bd47e930f07a8f52d95cc405ebbda6aab1b187abdbff8ff0277` / `d9b8936c1eba636a781c6dcd00e6706870f281edef70bbb37d18683f0d704f24` |
| macOS aarch64，native 测试 / 签名后包内二进制 | `a940e025ee5826b70696ac252de5869cbe975c7bcc926ede8f9136d36a758a8a` / `ff6bc6135df90e1a90f2995793c1dd98a1fdaf6adac37a594b61b9e4a2270ccc` |

macOS 的 `BUILD.json.source_binary_sha256` 接回实际运行过的签名前二进制。
tar.gz 与 zip 中的五个 bundle 文件逐字相同并匹配 manifest；原生 package 步骤执行
`codesign --verify --strict`。两包均为 **ad-hoc 签名、未公证**；未在签名后再次运行 GUI，也没有 DMG。
[独立 Mac 交叉核验](native/macos-cross-review.json)保留这条来源链，414 项只读断言全部通过。

## 公开资产与原始证据

[ASSETS.md](ASSETS.md)逐项列出 47 个官方 URL、精确字节数和 SHA-256；
[下载索引](audit/public-asset-index.json)保存每个实际 HTTP 状态。
`SHA256SUMS` 只绑定源码 tarball，不能把它称作所有 47 项的总校验表。
其他资产分别与 GitHub digest、对应 sidecar 和工作流原件核对。

官方 [native-evidence.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.5.0/ztty-0.5.0-native-evidence.zip)
为 16,732,253 B，SHA-256 `e989afb67dacd1046e0620581023cff4701a88f4701c6d4b613dbefb494bce3d`。
[归档核验](audit/native-evidence-verification.json)确认 949 个文件与六个 Actions 原件及 install.sh 逐字相同。
ZIP 包含 949 份原文件和 1 份 manifest.json，共 950 个文件条目；10 个顶层空日志仍保留。
两个 Linux 嵌套 native tar 各有 146 个文件、75 张 PNG、24 个空文件；“10”不是全部层级的空文件总数。

源码 [ztty-0.5.0.tar.gz](https://github.com/LIghtJUNction/ztty/releases/download/v0.5.0/ztty-0.5.0.tar.gz)
为 6,666,022 B，SHA-256 `cca31ef869c0872e0fd4a772f008f5919bab85628c11824c13fe4908016d6059`。
[源码归档核验](audit/source-archive-verification.json)确认展开后的 1,700 个 tracked 文件与 c4 及五个固定 gitlink 的 Git blob 一致，
其中 zterm 为已发布的 0.5.1 / a2074ba。这里完成的是内容核验，未另行重编译解包源码。
发行二进制、源码 tar 与完整证据 ZIP 复用官方资产，没有重复提交到本目录。

## AUR 与安装脚本

正式 publish 日志记录 AUR 实际提交 `70b24f3`，03:41:50 UTC 创建，03:41:58 UTC 将
`15b4853..70b24f3` 推到 master，提交标题为 `Update ztty to 0.5.0`。
这是实际 push 回执，不只依据 workflow step 名称或绿色状态判断。
对应原始行保存在[总审计的 aur 字段](audit/verification.json)。

公开 install.sh 为 9,889 B，SHA-256 `e078404f113ce6337c70f0878ab2be057f7135572a5e2c4ad19c6025206f2b08`，
与 tag 中脚本逐字一致。[独立烟测](installer/verification.json)固定 `ZTTY_VERSION=v0.5.0`，
把 binary、desktop 与临时目录全部放入自有 prefix；脚本实际下载并校验正式包。
安装用时 2.880 s，所得 1,217,232 B ELF 与上表正式 Linux x86_64 native 二进制一致；
`--version` 为 0.5.0，`--help` 含 backend/text-plugin，desktop 使用正确的私有绝对路径。

本次只执行安装、version/help，未重复 GUI；临时目录为空，自有 prefix 已清理。
六个现有安装/默认终端相关路径的前后快照一致。它没有替换用户当前安装，
清理测试 prefix 也不代表 ztty install.sh 提供卸载功能。
脚本、desktop 文本及全部 stdout/stderr（含空日志）均在 [installer](installer/) 中。

## Release 说明与审计中的中间失败

最初的 [Release API 回应](release/original-api.json)与 [458 B 原正文](release/original-body.md)分别保存。
[已发布的新正文](release/published-body.md)为 3,361 B，SHA-256
`a2ef81f3444c822cb75789df8b72209e08fcb71d1e53d10cce99e9f58b3cf252`；原自动说明和 native ZIP 提示作为连续原文只保留一次。
[编辑回执](release/release-notes-edit.json)确认 GitHub 插件逐字读回，47 项资产身份不变。

第一次外层脚本在成功编辑之后遇到 `gh api` 网络 EOF，wrapper exit 1；
[原失败](release/release-notes-first-readback-failure.json)与日志保留，编辑没有重复执行。
这是读回异常，不是发行失败。随后插件实际正文与交付文件一致。
原资产审计中出现的 ANSI 日志捕获拒绝和 API 字段读取错误也保留在
[audit-process-receipts.json](audit/audit-process-receipts.json)，没有因此重跑产品门禁。

## 离线复核与后续消费

```sh
python3 docs/reports/release-0.5.0-20261120/verify-record.py
```

脚本只读本目录，检查附件 SHA、来源关系、47 项资产元数据、六平台原报告与 GUI 计数、
安装烟测、说明原文保留、归档表格及全部公开副本。
它不重新下载制品、运行产品或把元数据核验数加到产品测试数中。
47 次实际下载及 949 原件的字节比较由固定的原审计回执记录，完整原件继续位于官方资产。

[redaction.json](redaction.json)保存各原件与公开副本的 SHA、路径变化字段和无损压缩信息。
只替换开发机个人目录；公开 CI runner 路径、所有数字、顺序、失败和空日志保持原值。
这份记录完成应用发行证明。suite 与 Project Z 的具体消费提交、验收和主线接收状态在统一迭代报告中分别列出。
