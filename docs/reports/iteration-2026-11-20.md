# ztty 0.5.0 迭代与发行报告

2026-11-20 · 正式源码 `c4f8f48c9598284a589e609f25242b49ceabfea8`

**ztty 0.5.0 已正式发布。** Linux、macOS、Windows 的 x86_64 与 ARM64 六个平台均完成本次正式原生门禁，47 个公开资产已实际下载并核验。AUR 已推送更新，公开安装脚本也完成了隔离安装烟测。现有用户安装和默认终端没有被这轮验收替换。[正式下载](https://github.com/LIghtJUNction/ztty/releases/tag/v0.5.0) · [完整发行证据](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/release-0.5.0-20261120/README.md)

本轮完成了独立库拆分、长文件整理、共享 Wayland 整合和短首页改写，也保留了性能调查中不理想的结果。采用的 zterm 0.5.1 只恢复了部分 Unicode 吞吐；最终宿主在软件环境下消除了此前约 100 MiB 的额外内存开销，但 10 窗口吞吐仍低于旧版。套件已用正式制品完成真实消费验收，中央仓库已提交最终固定版本；两者的最新候选尚未合入各自主线。各项证据和接收责任在下文分别列出。

## 1. 交付状态与固定来源

| 项目 | 已核实的状态 |
| --- | --- |
| 应用发行 | v0.5.0，Release 452238617，2026-11-20 03:41:44 UTC 发布；非 draft / prerelease |
| 正式源码 | `c4f8f48c9598284a589e609f25242b49ceabfea8`；annotated tag 对象 `4288d646b3273285483cf1fdad0ccc8e0c42b9ae` |
| 正式工作流 | [44668398577](https://github.com/LIghtJUNction/ztty/actions/runs/44668398577)，attempt 1，九个 job 全部 success，`build_only=false`、六平台全部选择 |
| 公开制品 | 47 个，合计 28,446,629 B；实际 HTTP 200 下载、大小与 digest 全部核对 |
| 独立库与 API | 六个 crate 已发布，宿主固定消费；registry 独立与组合消费者均有实际记录 |
| 套件消费 | oh-my-ztty 候选已固定正式源码/资产，双架构组装与 x86_64 实际 smoke / GUI 通过；[PR 1](https://github.com/LIghtJUNction/oh-my-ztty/pull/1) 待接收 |
| 中央仓库 | Project Z `7096e61` 已提交最终成对 pins 与 sync/check；仍在既有协调分支 |

c4 的父提交是生产检查点 [9855da5](https://github.com/LIghtJUNction/ztty/commit/9855da563f3038adee2fa88e1e24e20eafb77010)。c4 只增加前一检查点的性能文档，生产代码、工具、测试和 workflow 与 9855 相同。正式二进制由各平台 runner 构建；本地 9855 的性能数字保留其原 ELF 身份，没有改写成六个发行制品各自的性能结果。之后合入的报告也没有移动 v0.5.0 标签。[来源与资产表](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/release-0.5.0-20261120/ASSETS.md)

## 2. 实际改动与平台范围

### 职责拆分

终端可以分成几项独立工作：解释输入字节、保存屏幕和历史、管理子进程、布局 UI，以及把内容画到窗口。本轮按这些职责拆库，并拆分 session、Windows、macOS 的长文件。移出的实现仍由对应库维护，不计作整个项目生态的代码删除。[短首页与库职责](https://github.com/LIghtJUNction/ztty/blob/c4f8f48c9598284a589e609f25242b49ceabfea8/README.zh-CN.md)

zgpui 提供跨平台的受限 scene、协议适配、布局与 renderer 契约；ztty 仍负责窗口、实际渲染、输入和会话权限。布局记录中的 surface/revision 不是跨宿主身份认证，宿主继续保留自己的 ownership/generation 检查。它没有被包装成自带三平台原生窗口的框架；zsys 是系统信息库，也没有被当作窗口层引入。[zgpui 0.1.0](https://github.com/LIghtJUNction/zgpui/releases/tag/v0.1.0)

### 用户可见能力

- Linux X11 保留共享会话、Attach/MCP、标签页和分屏，支持 GPU 文字整形与 Xft 回退；图形协议包括 Kitty 图片/Sixel，以及相应 Linux 路径的动画、Unicode 占位符、相对位置、文件/共享内存传输和有界 OSC 52。
- 原生 Wayland 通过 `--backend wayland` 显式启用，与 X11 共用会话进程，包含 Attach/MCP、剪贴板与 text-input 输入法协议路径。官方 Linux 包编译该功能，运行时仍默认 X11。
- macOS 使用 AppKit/CoreText 与 Darwin PTY；Windows 使用 Win32/GDI 与 ConPTY，包含本地标签页和分屏。共享 broker、Attach 与 MCP 当前属于 Linux 范围。

这些能力及使用入口见[正式源码首页](https://github.com/LIghtJUNction/ztty/blob/c4f8f48c9598284a589e609f25242b49ceabfea8/README.zh-CN.md)。Wayland 仍为实验后端，会拒绝 tab/split/workspace 容器；物理桌面输入法、HiDPI/fractional scale 与硬件输入延迟不在本次 headless 验收的结论中。从源码克隆需要 `--recurse-submodules`；已有克隆先执行 `git submodule update --init --recursive`。

### 同规则源码规模

固定比较 `f2db31d7ea35bf1a0554e76d69d2e44faafd0cab` → `9855da563f3038adee2fa88e1e24e20eafb77010`：

| 范围 | 文件数：前 → 后 | 物理行：前 → 后 | 最终字节 |
| --- | --- | --- | ---: |
| 根仓库直接维护 | 79 → 89 | 40,436 → 30,893 | 1,094,322 |
| gitlink 库 | 3 → 39 | 1,345 → 14,306 | 491,136 |
| 展开合计 | 82 → 128 | 41,781 → 45,199 | 1,585,458 |

根直接维护减少 **9,543 行**，展开总量增加 **3,418 行**。这说明维护归属发生转移，同时共享 Wayland 等实现增加了代码。物理行包含空白、注释和 inline tests；专用 tests/examples 排除，`build/wayland.rs` 在双方都按原规则排除，不能与较早 37,848 行的另一口径混比。[规模原件及规则](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/host-9855da5-20261120/README.md)

## 3. 已发布库与真实 registry 消费

| Crate | 版本 | 发布源码 |
| --- | --- | --- |
| [zgpui](https://github.com/LIghtJUNction/zgpui/releases/tag/v0.1.0) | 0.1.0 | `1d796d1fc63808dc099ed7b10c92e75835ae2814` |
| [zgraphics](https://github.com/LIghtJUNction/zgraphics/releases/tag/v0.2.0) | 0.2.0 | `fe6357f98cf80fb6cc74739e45e8866ee17706ad` |
| [zinput](https://github.com/LIghtJUNction/zinput/releases/tag/v0.1.3) | 0.1.3 | `37617313dd7aaabcca9dd567344a76c4cbe03907` |
| [zpty](https://github.com/LIghtJUNction/zpty/releases/tag/v0.2.2) | 0.2.2 | `66919bc6dcb7757e0437fe955bfa809fb1282b68` |
| [zterm](https://github.com/LIghtJUNction/zterm/releases/tag/v0.5.1) | 0.5.1 | `a2074bac4c030cfd7c0b2514c852a89dcf54648f` |
| [ztty-plugin-api](https://crates.io/crates/ztty-plugin-api/0.1.0) | 0.1.0 | `36b367ffda4cea9b3b3dfbf38dc6375567912323` |

API 0.1.0 实包为 3,482 B，SHA-256 `e00adc5df1eefa237ea1ccd8d837ceecabef1976141b78918901d95cb8e9d4a6`，依赖精确 `zplugin = 0.3.1`。它的发布 VCS 是 36b，不随宿主更新而变成 9f、9855 或 c4。[API 首发记录](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/plugin-api-registry-20261120/README.md)

API 首发后，36b 完成六个独立 registry 消费者和一个组合消费者。zterm 升至 0.5.1 后，9f 只复验发生变化的 **zterm 独立 + 六 crate 组合**：六条 Cargo 命令全部通过。no_std 是库目标，运行使用 std harness；组合消费者真实创建 PTY。9855/c4 的锁文件和五个 gitlink 与该已验证消费图一致，没有把复用证据写成重新运行全部七个消费者。[9f 消费图与原件](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/host-9f5a77a-20261120/README.md)

本轮独立化的 zgpui/zgraphics/zpty/zterm 由本轮 owner 传播到 zterm、ztty 和套件。zinput、zplugin、zsys 的其他外部下游仍属于各自维护者，不能把 ztty 的成功消费计作所有下游完成迁移。

## 4. 功能验收：区分每一次源码和运行

| 检查点 | 实际结果 | 范围 |
| --- | --- | --- |
| 9f / zterm 0.5.1 | 472 Rust；另 6 EGL；完整 21 阶段、365 GUI/CLI checks | 原 EGL 收尾解析器 exit 1 与补充核验都保留，未重跑；全部 owned 清理完成 |
| 9855 本地 | make check exit 0；472 passed、0 failed、6 ignored | Wayland + bare-metal；ignored 不记为这次已经执行 |
| 9855 定向 GUI | Auto native UI 24 + 显式 GPU text 8，32/32 | 包含同会话 GPU→software→GPU 热切换；补充收据保留原 postprocessor KeyError |
| c4 正式发行 | 六平台均在正式 run 中重新执行对应 native gate | 使用本次 runner 构建的制品，不沿用旧 edae/9f 结果 |

[9f 记录](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/host-9f5a77a-20261120/README.md)、[9855 记录](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/host-9855da5-20261120/README.md)与[正式发行记录](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/release-0.5.0-20261120/README.md)各自保留输入、原日志和清理回执。9f 六项 EGL 的实际 renderer 是 NVIDIA RTX 4060 Laptop GPU；正式 CI 的 Linux renderer 则是下面列出的 llvmpipe，两者不混用。

### 正式六平台的实际范围

| 平台 | 本次实际结果 | 环境与限制 |
| --- | --- | --- |
| Linux x86_64 | 278 GUI；另 6 EGL、17 driver、4 cwd；input 内含 198 vectors | 私有 Xvfb/headless Sway；llvmpipe LLVM 20.1.2 / 256 bits |
| Linux aarch64 | 同上，各项在 ARM runner 实际执行 | llvmpipe LLVM 20.1.2 / 128 bits |
| macOS x86_64 | 24 checks、13 stages、10 PNG | AppKit/Darwin PTY；GUI 在签名前执行 |
| macOS aarch64 | 同上，各项在 ARM runner 实际执行 | IME keyboard 未执行 |
| Windows x86_64 | 31 contracts，8 次 cold start 逐次通过；workspace 22、fixture contract 41 | ConPTY/DIB/private desktop；maximize 保留 PTY 身份已通过 |
| Windows aarch64 | 同上，各项在 ARM runner 实际执行 | 全部冷启动、workspace 与清理原件保留 |

Linux 的 198 向量属于已有 input checks，不另加作 198 项 GUI 契约；Windows 八轮重复相同 31 项，也不计成 248 种不同功能。Linux headless Wayland 证明相应协议和像素路径，没有测物理显示扫描或硬件输入延迟。Mac 包采用 **ad-hoc 签名、未公证**：签名前测试二进制经 BUILD 来源字段接回签名后包内文件，tar.gz/zip 与 manifest 一致；未在签名后重复 GUI，也没有 DMG。[六平台原报告与签名链](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/release-0.5.0-20261120/README.md)

9855 本地 gate 用时 **47.163 s**，峰值 **805,306,368 B**，触及 768 MiB 限额，记录 2,856 次 max 事件，OOM/swap 为 0。资源限制下完成不表示没有触顶。其 Wayland ELF 为 1,237,824 B，SHA-256 `9c0297477b2a24355f865d12413135a327a69a7a8f7dd50e40927ae9e21f1e66`。所有测试阶段均有自有进程清理记录。[资源与原始收据](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/host-9855da5-20261120/README.md)

## 5. 性能结果与采用决定

### 核心 Unicode：部分恢复，仍有明显历史差距

a207 同一组三方核心测量中，Unicode 中位数为旧版 **91.404**、zterm 0.5.0 **19.330**、采用的 0.5.1 **26.101 MiB/s**。相对 0.5.0 提高 **35.03%**，仍只有旧版的 **28.56%**。这是核心处理场景，不含 PTY、窗口和完整渲染。

后续 28d 批处理候选没有采用。同窗口六对测量的 batch/scalar 中位数比分别为 ASCII 0.9474、history 0.9136、SGR 0.9741、Unicode 0.9620，不支持其增量收益。五批正式核心采样（6、9、9、9、12 次运行）全部保留，GDB/profile 只作诊断，不加入吞吐结果。[完整核心报告与未采纳附录](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/benchmarks/resume-2026-11-20.md)

### 软件回退内存：先保留回归，再限定修复

9f 的完整宿主采样发现，新旧 1 / 10 窗口吞吐中位数下降 24.88% / 29.45%，PSS 从 13.45 / 18.98 MiB 增至 123.79 / 148.41 MiB。同一 9f ELF 的单次 Auto/software 诊断进一步测到 **95.52 MiB** 差额：Auto 最终虽为 software，仍映射 LLVM/Gallium/NVIDIA 库；显式 software 没有这些映射。诊断用于归因，没有混入正式三轮采样。[9f 原结果与诊断](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/benchmarks/host-9f5a77a-20261120/README.md)

9855 只在 **X11 Auto 且 `LIBGL_ALWAYS_SOFTWARE` 恰好为 `1`** 时，提前进入已有 software 路径，避免先初始化 GPU。显式 GPU、变量未设或其他值继续原规则。该修复只改 `src/x11/gpu.rs`，没有改核心 Unicode 方案。[精确修复](https://github.com/LIghtJUNction/ztty/commit/9855da563f3038adee2fa88e1e24e20eafb77010)

### 9855 最终宿主：两类矩阵分别解释

新三阶段全部 exit 0：12 次新旧配对、18 次三应用运行、6 组 output，合计 513 个输出 burst、50,700 个 DSR 原值；paired/output/terminals 分别用时 32.458 / 6.469 / 130.197 s。以下均为该 cohort 内三轮中位数，全部样本、区间、主机压力和 source/ELF/collector 身份见[9855 完整报告](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/host-9855da5-20261120/README.md)。

**新旧软件环境配对：**新版实际 renderer 为 software、driver 为空；旧版未提供 actual renderer 字段。

| 窗口 | 旧 → 新 MiB/s | 本轮新/旧变化 | 旧 → 新应用 PSS MiB |
| --- | ---: | ---: | ---: |
| 1 | 193.328 → 217.730 | +12.62% | 12.090 → 12.827 |
| 10 | 278.808 → 217.971 | −21.82% | 17.736 → 21.416 |

此前的大额内存增长没有在新路径重现；10 窗口吞吐仍有差距，PSS 也并非与旧版完全相同。旧版单窗口基线本轮约 193 MiB/s，前一 cohort 约 296 MiB/s，不能把跨轮差异算作修复收益。内存归因依据是同 9f ELF 的定向诊断、代码路径和新 9855 实际 renderer 三者相互对应。

**同轮三应用矩阵：**这里 ztty 强制 GPU，实际为 llvmpipe；不能用上表 software 的 12.83 MiB 代替这一组内存值。

| 应用 | 1 窗口 PSS MiB / MiB/s | 10 窗口 PSS MiB / MiB/s |
| --- | ---: | ---: |
| Ghostty | 315.09 / 171.837 | 814.20 / 187.582 |
| Kitty | 177.02 / 57.896 | 356.34 / 59.001 |
| ztty，GPU/llvmpipe | 136.11 / 120.201 | 326.55 / 149.798 |

PSS 为应用进程口径，不含专用 X server。Ghostty 日志只记录 Mesa/OpenGL 4.6，Kitty actual driver 未知；这不是同一物理 GPU 的比较或通用终端排名。PTY/DSR 计时终点是输出被处理后的光标报告，不是物理键盘到显示像素的延迟。ztty 的完整屏幕文本校验覆盖配对 66 次加三应用 33 次窗口运行，共 99 次；Ghostty、Kitty 各 33 次采用 CPR/字节 hash，未宣称取得它们的整屏文本校验。

## 6. 发行资产、安装与原始证据

正式 native evidence ZIP 包含 **949 份原文件和 1 份 manifest.json，共 950 个文件条目**；949 份原文件与六份 Actions 原件及 install.sh 逐字一致，顶层 10 个空日志保留。两个 Linux 嵌套 native tar 各含 146 文件，其中 24 个为空，不能把顶层的 10 当作所有层级总数。源码 tar 的 **1,700 个文件、五个 gitlink** 与 c4 Git blob 精确匹配。源码、安装包和完整证据直接复用官方资产，不重复提交大二进制。[47 项 URL / 字节 / SHA-256](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/release-0.5.0-20261120/ASSETS.md)

`SHA256SUMS` 只覆盖源码 tarball；其余资产按各自 digest/sidecar 核对。总审计的 13,751 项是来源、结构和哈希断言，不增加产品测试数。AUR 有实际 push 回执：03:41:50 UTC 创建 `70b24f3`，03:41:58 UTC 将 `15b4853..70b24f3` 推至 master。[正式审计与 AUR 原始行](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/release-0.5.0-20261120/audit/verification.json)

公开 install.sh 的隔离烟测固定 `ZTTY_VERSION=v0.5.0`，把安装、desktop、临时目录放入私有 prefix，真实下载并校验正式包。所得 Linux x86_64 ELF 与正式 native 一致，`--version` 为 0.5.0，`--help` 含 backend/text-plugin；六个用户安装/默认终端路径前后不变，私有 prefix 已清理。这里只验安装与 version/help；它不是替换用户现安装，也不表示 install.sh 提供卸载功能。[安装原件](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/release-0.5.0-20261120/installer/verification.json)

Release 正文已更新为本次功能、限制和性能说明，原自动生成说明与 evidence ZIP 提示连续保留一次。编辑成功后的首次 `gh api` 读回曾遇网络 EOF；原失败保留，未重复编辑，随后 GitHub 插件逐字核对正文，47 个资产不变。[正文及读回记录](https://github.com/LIghtJUNction/ztty/blob/b0c5c8e9dd9a2730fbc95b279464da1b47e9be0d/docs/reports/release-0.5.0-20261120/README.md#release-说明与审计中的中间失败)

## 7. 依赖传播与接手责任

传播按实际边完成：上游发布后，消费者更新版本、固定源码和锁，再记录测试与提交。单独发布通知或消息 ACK 不算完成消费迁移。

### ztty → oh-my-ztty：真实候选通过，等待维护者接收

套件 [PR 1](https://github.com/LIghtJUNction/oh-my-ztty/pull/1) 当前 open、非 draft、mergeable clean，head `0fdc87873c53e7c87d18952f83a7b94fc4149de5`，base `740b82f3a1628d0c14b0405bfb36c76832fedfec`。三笔提交分别是 CLI 兼容 86bb、正式 pin 5a99、公共证据 0fdc；组件固定 c4，锁记录双 Linux 正式 URL/hash。

既有 42 项检查通过，两架构包各 84 文件摘要通过。x86_64 原 smoke 实际覆盖安装、重复安装、恢复和卸载；原四项 GUI 通过，使用 `--backend x11`，私有 `SHELL=/bin/sh` 下仍启动预设 `/usr/bin/zsh`，PTY 100×28、首次 Tab 缓存复用，组件版本准确。23.236 s 的验收峰值为 456,372,224 B，swap/OOM 为 0，所有自有 unit/子进程已清理，既有安装未变。[完整套件记录与原件](https://github.com/LIghtJUNction/oh-my-ztty/blob/0fdc87873c53e7c87d18952f83a7b94fc4149de5/docs/reports/ztty-0.5.0-consumer-20261120.md)

ARM64 本次只完成组装、ELF 和 manifest 核验，没有运行；`--text-plugin` 只做 stub 参数转发，不计实际插件执行。套件 VERSION 仍为 0.2.1 私有候选，未替换正式资产、未发布或部署到用户。后续 PR 合入、版本与正式发行由现有 oh-my-ztty 维护者接手。

### Project Z：最终 pins 已推协调分支，主线尚未合入

[7096e61](https://github.com/LIghtJUNction/project-z/commit/7096e61a45347755daa0ae9e4a2720ab7c949edb) 已把 ztty 固定到 c4，与 zterm 0.5.1/a207 成对记录在 `coord/verified-consumers-20261120`。本次 sync/check 覆盖 42 仓库、60 包，各执行 37 次 `metadata --no-deps`；图包含 226 direct、84 internal、56 repository 边，128 项当前 lock/config 的字节、模式及存在性保持不变。

此前 36 个 Rust 仓库的完整 resolve 保留原 9f 身份；因为生产 Cargo/lock/config/gitlink 输入未变，本次没有重复完整 resolve，也没有把旧结果改名到 c4。新 9855 功能证据单独引用。[中央固定证据](https://github.com/LIghtJUNction/project-z/blob/7096e61a45347755daa0ae9e4a2720ab7c949edb/docs/evidence/2026-11-20/ztty-v050-final-source.json)

本报告读回时中央 main 仍为 `46e98822e8357b25670cea52794af3009d66660f`。最终 pins 和必要校验已提交推送，尚不能称已合 main；最新套件 PR 的接收也需要由中央现有 owner 明确登记。root 已向两个既有维护者交付实际提交与验收，后续接收以他们的真实 merge/commit 或具体阻碍为准。

## 8. 后续工作的明确边界

| 后续事项 | 当前依据与接手方 |
| --- | --- |
| Unicode 核心与 10 窗口吞吐差距 | a207 核心和 9855 宿主原始数据均已公开；后续 zterm/ztty 维护者按新固定输入继续调查 |
| 真实桌面 Wayland、输入法和缩放 | 当前是 headless 协议/像素证据；平台维护者另行组织对应实机验证 |
| macOS 分发体验 | 当前为 ad-hoc、未公证包；Developer ID / notarization 若需要，由发行维护者另行完成 |
| 套件正式版本与部署 | 已验收候选在 PR 1；oh-my-ztty 维护者决定合入、版本和发行，不把私有候选当用户已升级 |
| Project Z 主线接收 | 最终 pins 已在 7096 协调分支；现有中央 owner 完成合入与套件新状态登记 |

已完成的应用发行、六平台实测、registry 消费和已公开性能记录不因这些后续事项被改写。各附件保留失败、原始顺序、原/公开 SHA 与必要路径脱敏记录；旧检查点不覆盖，新结果均另开固定来源。首页继续提供简短安装与使用入口，完整证据留在相应报告中。
