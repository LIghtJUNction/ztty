# ztty v0.5.0 正式发布审计

固定源码 c4f8f48c9598284a589e609f25242b49ceabfea8 的正式发布审计通过；47 个公开资产、六个平台的原始验收证据和源码包身份均已核实。本审计只读取、下载、解析及校验原件，没有重新编译、运行产品、执行 GUI 或计时。

- 正式 run：[44668398577](https://github.com/LIghtJUNction/ztty/actions/runs/44668398577)，workflow_dispatch，attempt 1，9 个 job 全部 success。
- Release：[v0.5.0](https://github.com/LIghtJUNction/ztty/releases/tag/v0.5.0)，ID 452238617，公开时间 2026-11-20T03:41:44Z。
- 不可变 annotated tag object：4288d646b3273285483cf1fdad0ccc8e0c42b9ae，目标为上述 c4 源码。
- 总回执：verification.json，SHA256 6bf85fefec7a104278eeb7f0b580db4fb4b19d9a8c7bee278f46c51b6ef40f7b；13,751 项结构、身份及内容核对通过。独立 macOS 交叉审计另有 414 项核对。

## 六个平台的实际执行范围

| 平台 | 本次固定源码上的原始验收 | 运行环境和边界 |
|---|---|---|
| Linux x86_64 | 278 GUI checks；输入 198 向量；显式 EGL 6、工作目录 4、fixture driver 17 | Xvfb/XTEST 及私有 headless Sway；GPU 路径实际 llvmpipe (LLVM 20.1.2, 256 bits) |
| Linux aarch64 | 278 GUI checks；输入 198 向量；显式 EGL 6、工作目录 4、fixture driver 17 | Xvfb/XTEST 及私有 headless Sway；GPU 路径实际 llvmpipe (LLVM 20.1.2, 128 bits) |
| Windows x86_64 | 8 次冷启动各 31 项检查、22 项 workspace、41 项 fixture contract | 真实 ConPTY、Win32 retained DIB、私有 desktop；可选 paint/nonclient 诊断关闭 |
| Windows aarch64 | 8 次冷启动各 31 项检查、22 项 workspace、41 项 fixture contract | 真实 ConPTY、Win32 retained DIB、私有 desktop；可选 paint/nonclient 诊断关闭 |
| macOS x86_64 | 24 checks、13 stages、10 个 PNG 原件 | 原生 AppKit/Darwin PTY；GUI 验收作用于签名前二进制 |
| macOS aarch64 | 24 checks、13 stages、10 个 PNG 原件 | 原生 AppKit/Darwin PTY；GUI 验收作用于签名前二进制 |

每个 Linux 的 278 项由首批 101，加 GPU text 8、UI 24、repaint 11、retained 2、graphics 8×2、OSC52 13×2、clipboard 25、Wayland 65 组成。首批 linux_native.json 中对未测 Wayland 的限制仅描述该 101 项 wrapper；同一正式 job 后续实际执行了独立的 65 项 Wayland 验收。所有报告中的明确 owned-process remaining 字段均为空。Windows 的 31 项是同一契约在 8 次冷启动中重复验证，不应包装为 248 种独立行为。

Linux 发布 ELF 与该架构原生报告中的 SHA256 相同；Windows 包内 ztty.exe 也与原生报告完全相同，ConPTY DLL 与 OpenConsole 的 manifest 哈希及 PE 架构一致。macOS 的 BUILD.source_binary_sha256 连接签前受测文件与签后包；CodeDirectory flags=2 为 ad-hoc，codesign --verify --strict 成功，notarized=false。签后包没有第二次 GUI 验收，IME keyboard 标为 not_performed；产物为 ZIP 和 tar.gz，没有 DMG。所有平台都没有物理输入到扫描输出延迟结论。

## 资产及源码完整性

47 个公开资产共 28,446,629 B 均经匿名 HTTP 200 下载，并匹配 Release API 的大小及 SHA256。public-asset-index.json 给出每项公开 URL、字节数及完整摘要。六份 Actions ZIP 同时匹配其官方 artifact digest。

公开 native-evidence.zip 的 949 个文件与六份 Actions 原件及 install.sh 逐字一致，包含 10 个顶层原空日志。两份 Linux native.tar.gz 内各有 146 个文件，其中各 24 个为空；它们随整个 tar 原字节保留。因此“10 个空日志”不是所有嵌套空文件总数。证据 ZIP 本体另有一个 manifest.json（总计 950 个 ZIP entries）。

公开 source tar 的 1,700 个文件逐个匹配 c4 Git blob，且文件集合恰好为根仓与五个 gitlink 的 tracked 集合；五个固定提交见 source-archive-verification.json。公开 SHA256SUMS 只列 source tar，不能称为全部 47 个资产的总表；完整资产身份由独立索引和对应 .sha256 文件说明。

## 六个平台主要包的公开摘要

| 文件 | 字节 | SHA256 |
|---|---:|---|
| [ztty-linux-x86_64.tar.gz](https://github.com/LIghtJUNction/ztty/releases/download/v0.5.0/ztty-linux-x86_64.tar.gz) | 584351 | 4b79ded489c37325ac049e4962e75c7ff773c2c5d073d8fbad154bb47dfed6b2 |
| [ztty-linux-aarch64.tar.gz](https://github.com/LIghtJUNction/ztty/releases/download/v0.5.0/ztty-linux-aarch64.tar.gz) | 552407 | f86ddc20d6359c8301e7198adf80359c067ea02c28228139e2a39705a5ddd09d |
| [ztty-macos-x86_64.tar.gz](https://github.com/LIghtJUNction/ztty/releases/download/v0.5.0/ztty-macos-x86_64.tar.gz) | 277930 | dcf27e3d0dc86b67f35f44f365b6cc07fcc2e480c39ed412c017b2040fe129f8 |
| [ztty-macos-aarch64.tar.gz](https://github.com/LIghtJUNction/ztty/releases/download/v0.5.0/ztty-macos-aarch64.tar.gz) | 251447 | 3cf0005f5d47e05668f2de35da84ab51067c268c60ef7ef2b314e9e2d069c326 |
| [ztty-windows-x86_64.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.5.0/ztty-windows-x86_64.zip) | 932505 | 8ad4f75dc47f8ef73ca86fda6ccceb8befdbb7abb3d788e96f7a0abc9db36390 |
| [ztty-windows-aarch64.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.5.0/ztty-windows-aarch64.zip) | 858047 | 43f3df89ce8a6c7507104df0883ea690848e7a37a77e9eaf76649765de97f06b |

macOS 的额外 ZIP 与 tar.gz 内五个文件逐字相同；ZIP 摘要同样保存在 47 项公开索引。

## 发布和安装的边界

AUR 原日志确认 2026-11-20T03:41:50Z 创建 70b24f3（Update ztty to 0.5.0），03:41:58Z 实际推送 15b4853..70b24f3 HEAD -> master。正式 publish 也在 Arch 容器中执行 makepkg、pacman -U、版本与 desktop 条目校验。

正式 workflow 对公开 install.sh 做上传后下载及逐字比较，没有通过该脚本在私有 prefix 执行安装与卸载。root 随后独立完成公开 installer 的私有 prefix 安装、版本/help 与清理烟测；其另行回执是 published-installer-v050-20261120/verification.json（SHA256 92bb4bcdbff7c40c3fdaad217ed0c678ac7e019e67ce44a56eb5f0920f64d0c7）。这不是再次 GUI 验收，也没有改变用户现有安装。

## 原件索引

| 原件 | SHA256 |
|---|---|
| public-asset-index.json | 1cc945d2420132d7aa4471cae712fd7c7f74e11d6647889324cd6c3f4cd6ae52 |
| workflow-artifact-index.json | 8ea6756020c948229f45fa781e274e9ed299367d5b6878196e3302cde9aaa820 |
| native-platforms.json | b550e977a7f2d81d2f34d5f47abee63986a5d5af3823d5a7bf5c7672bdce593a |
| linux-nested-evidence-index.json | 92c5ee17ce5777550bf9bf6643de2b8b5785ba92fcfcf001eed7587362133466 |
| native-evidence-verification.json | 19609ec1188ba0bacb8d83ee8ee086b5803b2b5af70132e28d8bf0f697c30058 |
| source-archive-verification.json | a3b0aff6dc15d59e5f6d03f0e6b54a804e4fc51d7bc92c7b9fb451b0be0fc25d |
| macos-independent-review.json | db6cf733b9f1fda4fc3add9ddc3d6c0658c3e0105d207b51483119a84c466466 |
| job-logs-verification.json | 410dc2c5df4cd8de5e412dad8c00355ae7ba36837405533504ff9a7d58468547 |

初次 gh 日志下载受到 CLI 对终端 escape 字节的输出保护而停止；已保留当时文件，随后只向文件写入完整 raw bytes，未在终端重放。另一次审计初稿因使用精简 API 快照缺少 tag_name 而停止，修正为完整 API 原文后核验通过；这两项属于采集/审计过程，不是产品门禁失败。audit-process-receipts.json 记录各尝试与源脚本摘要。
