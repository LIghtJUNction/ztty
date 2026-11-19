# ztty 性能与功能迭代阶段记录

日期：2026-11-19。源码已保存到 [GitHub 恢复分支](https://github.com/LIghtJUNction/ztty/tree/iter/performance-function-recovery-20261119)，固定代码提交为 [`dfe363f`](https://github.com/LIghtJUNction/ztty/commit/dfe363f3a16bf4b410185679638832c5df52b2e8)。它合并了本轮迭代与并行小键盘修改，并保留双方历史；GitHub 中的文件树与本地审核版本完全一致。

**前半轮改进已安装，后续修复已分别验收，最终整合验收与安装尚未完成。** 开发机 ssmcp 连接中断，最后一次综合检查只读回格式检查成功；本记录保存已取得的结果，恢复连接后继续原任务。

## 当前完成到哪里

| 阶段 | 已确认结果 |
| --- | --- |
| 前半版本 `11a74a5` | 本轮失联前已安装为本机包 `0.4.1-2`，同步 `/usr/bin/ztty` 与 `/home/lightjunction/.cargo/bin/ztty`；通过 331 项 Rust 单元／集成测试、6 项文档测试，以及独立的 9 项 SessionCore 真实 PTY 检查；另通过 70 项输入向量、10 项历史滚动 GUI、19 项日常操作、11 项重绘检查 |
| 后续小键盘整合 | 198 项真实 PTY 输入向量、4 组本地界面检查通过 |
| Sixel 修复 | 8 组 GUI 检查、8 项新增核心测试通过；共享快照容量记账补丁也完成独立测试 |
| 新窗口 broker 换代 | 10 项真实 GUI／PTY／IPC 检查通过 |
| 最终整合 | 开发机 `3ba09ad` 版本综合任务 `-143` 仅确认 `fmt` 成功，其余结果因离线未知；已推送的恢复分支尚待重新整体验证，最终新包未由本轮安装 |

远端主分支另有并行任务通过 PR #17 合入的小键盘修复，提交为 `5d66749`。恢复分支已保留这些修改；该并行任务的发布或安装不计作本轮完成。源码可查：[前半版本](https://github.com/LIghtJUNction/ztty/tree/11a74a51f0f6c52471c006582d46439cd713ce26)、[已推送迭代分支](https://github.com/LIghtJUNction/ztty/tree/iter/performance-function-20261119)、[主分支小键盘提交](https://github.com/LIghtJUNction/ztty/commit/5d66749805e73c41ca252e297bddabc67efd1ff5)。

## 历史阅读与局部滚动

历史缓冲满后，新行进入、旧行淘汰，总行数不变。旧实现用总行数差补偿阅读位置，因而得到零，画面随新输出滑动。修复改用终端模型记录的历史前进量，并同步到 Linux、Windows、macOS 视图：目标行仍保留时固定位置，被淘汰后夹紧到最早保留行。

私有 Xvfb 复现使用 8 行历史、真实 PTY 和不同颜色的行：旧版追加两行后，顶部像素由 `[100,30,50]` 变成 `[110,30,50]`；修复后保持 `[100,30,50]`。继续输出到目标行被淘汰时正确夹紧。10 项检查还覆盖输入回到实时底部、ED3 清历史及主备屏切换。[回归脚本](https://github.com/LIghtJUNction/ztty/blob/11a74a51f0f6c52471c006582d46439cd713ce26/tests/scrollback_gui.py)。

局部滚动 N 行原先重复移动单行，同一批单元格会复制多次。现在一次移动保留行，再清空空出的区域；环形缓冲按连续片段搬运，保留字符、样式和软换行信息。整屏滚动沿用原有环形历史路径。正确性检查包括 300 组批量／逐行等价用例及 5,106 组环形边界组合。

### 固定提交的滚动测量

比较 `5ce8a1d` 与 `e13c197`。机器为 i9-12900HX、Rust 1.100.0，固定逻辑 CPU 23、powersave；五组交替 A/B，每次每场景七个样本，每样本 1,024 次操作，计时期间暂停本轮其他构建。

下表单位为 **µs/次**，数值是五次运行中位数的中位数，方括号为这五个中位数的最小／最大值，**不是置信区间**。S 为上滚，T 为下滚。场景为环形缓冲已旋转后的内侧滚动区，只测 VT 解析与内存屏幕模型，不包含 PTY、绘制或显示延迟。[基准源码](https://github.com/LIghtJUNction/ztty/blob/e13c19731dacc1bd40f16f8419f3fdeba49757b7/crates/zterm/examples/scroll_benchmark.rs)。

| 网格 | 操作／行数 | 基线中位数 [范围] | 候选中位数 [范围] | 基线／候选 |
| --- | --- | ---: | ---: | ---: |
| 120×48 | S／1 | 4.566 [4.533, 5.248] | 4.568 [4.512, 7.415] | 1.00× |
| 120×48 | T／1 | 4.958 [4.784, 5.452] | 4.450 [4.401, 6.095] | 1.11× |
| 120×48 | S／8 | 37.373 [36.009, 39.354] | 5.703 [5.671, 6.840] | 6.55× |
| 120×48 | T／8 | 42.747 [38.322, 48.548] | 5.988 [5.944, 7.966] | 7.14× |
| 120×48 | S／24 | 116.438 [107.896, 144.030] | 7.124 [7.120, 7.477] | 16.34× |
| 120×48 | T／24 | 125.258 [116.289, 125.852] | 7.253 [7.058, 8.432] | 17.27× |
| 240×80 | S／1 | 14.627 [14.180, 21.840] | 14.591 [14.139, 18.551] | 1.00× |
| 240×80 | T／1 | 18.864 [14.368, 23.474] | 16.180 [14.054, 20.002] | 1.17× |
| 240×80 | S／8 | 119.991 [113.887, 197.717] | 17.888 [17.839, 22.909] | 6.71× |
| 240×80 | T／8 | 129.907 [119.112, 181.156] | 17.371 [17.330, 24.851] | 7.48× |
| 240×80 | S／24 | 363.066 [348.878, 376.947] | 19.963 [19.939, 33.718] | 18.19× |
| 240×80 | T／24 | 395.913 [364.132, 427.563] | 21.496 [20.738, 22.223] | 18.42× |

8 行批量滚动为 6.55–7.48 倍，24 行为 16.34–18.42 倍；单行上滚基本持平。稳态分配均为零，初始核心堆均为 107,932 B。通用核心基准未出现一致提升：ASCII 为 214.161→206.979 MiB/s，SGR 为 134.551→129.629 MiB/s；1 KiB 分块输入 p50 为 3.878→3.436 µs。加速结论限定于表中的局部批量滚动。

## 搜索：Alt+C 切换大小写

Linux 按 `Ctrl+Shift+F` 打开历史搜索，搜索栏中按 `Alt+C` 切换 `Case`／`Ignore case`，`Enter`／`Shift+Enter` 前后选择命中；默认仍区分大小写。忽略大小写使用 Unicode 字符小写映射，`error` 可匹配 `ERROR`、`Error`、`error`；它不提供完整 case folding、重音归一化或正则，`ß` 与 `SS` 不视为等价。复制保留原文。

同时修复单独按修饰键导致当前命中重置的问题：查询实际改变才重新搜索，准备按 Ctrl+Shift 复制时保留当前选中项。真实 GUI 检查核对了剪贴板和子 PTY 无输入泄漏。[搜索实现](https://github.com/LIghtJUNction/ztty/blob/11a74a51f0f6c52471c006582d46439cd713ce26/src/search.rs)、[操作回归](https://github.com/LIghtJUNction/ztty/blob/11a74a51f0f6c52471c006582d46439cd713ce26/tests/daily_workflow.py)。

## 小键盘与旧 broker

新版也存在独立遗漏：Num Lock 开启时，应用键盘模式（DECKPAM）仍可能把数字键编码为 `ESC Op` 等特殊序列。修复让 Num Lock 的数字语义优先；Kitty 明确请求完整事件报告时仍保留对应键身份。X11 同时保留小键盘实际的 Shift／Num Lock 层，按住按键后松开 Shift 时，自动重复与释放事件延续首次按下的身份。搜索补齐 KP_Enter／Shift+KP_Enter，命令面板补齐 KP_Up／KP_Down／KP_Enter，并避免单独的修饰键重置选中项。[整合后的输入回归](https://github.com/LIghtJUNction/ztty/blob/dfe363f3a16bf4b410185679638832c5df52b2e8/tests/input_gui.py)保留动态解析的物理小数点键 91，避免只测始终为小数点的别名键 129。

这组整合源码在独立二进制上通过 198 项真实 XTEST→PTY 字节向量及 4 组本地界面检查，包括前后搜索命中与实际字体／PTY 网格变化。恢复分支的输入测试、session、X11、zinput 四个文件与该已验收源码逐字相同；这不代替整个恢复版本的最终运行检查。Num Lock 优先级参考 [xterm 官方控制序列文档](https://invisible-island.net/xterm/ctlseqs/ctlseqs.pdf)中的应用小键盘说明。

现场只读核对还发现，7 个现有窗口仍属于旧 `0.3.1-2` broker，运行映像已标记为 deleted；磁盘文件更新不会改写存活进程。从该进程读取旧 ELF，在私有 Xvfb 中复现：Num Lock 开启、Kitty keyboard flags 为 1 时，数字和运算符会被编码为 `CSI 57399u` 等特殊键序列，当前 ssmcp 输入解析器不能还原为普通数字。新版在同一回归输出普通文本。现场使用 fcitx，但没有读取用户按键或会话模式，因此不把全部现场症状归因于这一种模式。

新窗口换代针对这个已观察到的问题：仅当**没有显式 `ZTTY_SOCKET`、没有目标窗口／标签／分屏**时，启动端用 `SO_PEERCRED` 确认 broker 进程，再比较 `/proc/PID/exe` 与当前执行文件的内容指纹；新建独立窗口优先复用当前构建的备用池，否则在默认池映像不同时选择备用 socket。相同字节的 `/usr/bin`、Cargo 副本共享 broker。选择在创建 PTY 前写入环境，使子终端继承正确地址；指纹只在启动选路时计算，不进入事件循环，也不承担安全认证。

显式 socket、attach、tab、split、MCP 继续使用既定目标；现有窗口保留原 broker，**不会热升级**。10 项隔离检查覆盖已删除旧映像、同内容不同路径、PTY 继承、旧池保留及旧池退出后的重复启动。[实现](https://github.com/LIghtJUNction/ztty/blob/e6ceb1959a688214981b75fc0f6d0051ccb8fea7/src/broker.rs)、[回归脚本](https://github.com/LIghtJUNction/ztty/blob/e6ceb1959a688214981b75fc0f6d0051ccb8fea7/tests/broker_upgrade.py)。

## Sixel 图片转文字后的残留

这个问题在本轮已安装新版中也能复现：文字、空格、ECH、EL、ED0、ED1 更新了字符，却没有清除对应 Sixel 像素，下一帧图片又覆盖回来。修复让文字覆盖与局部擦除清除相交源像素，并更新图像修订号；未覆盖部分、其它 Sixel 和 Kitty 图片继续保留，图片全部擦完后回收存储。共享快照复制时的容量记账也已补齐。

擦除单位是**源像素**。纵向缩放后，一个源像素跨越擦除边界时会整体清除，最多多清除 `scale_y - 1` 行显示像素；默认 2 倍缩放与奇数行高已有回归。1∶1 图像的范围外像素逐边界核对通过。8 组 GUI 使用合成图片；新增 8 项核心测试覆盖宽字符、组合字符、历史、缩放、隔离、反复替换回收及共享快照。[修复源码](https://github.com/LIghtJUNction/ztty/commit/e6ceb1959a688214981b75fc0f6d0051ccb8fea7)、[GUI 脚本](https://github.com/LIghtJUNction/ztty/blob/e6ceb1959a688214981b75fc0f6d0051ccb8fea7/tests/sixel_preview_gui.py)。

无图路径额外比较 `11a74a5` 与主修复 `eaddb76`，固定 CPU 23，三组交替配对；共享快照容量补丁在该次测量之后单独验证。

| 指标 | 基线 | Sixel 主修复 |
| --- | ---: | ---: |
| ASCII，MiB/s | 214.845 | 213.741 |
| 满历史 ASCII 滚动，MiB/s | 139.138 | 139.237 |
| SGR，MiB/s | 133.687 | 129.212 |
| Unicode，MiB/s | 88.534 | 98.268 |
| 初始核心堆，B | 107,932 | 107,932 |
| 稳态分配次数 | 0 | 0 |

ASCII 中位差约 −0.5%，满历史滚动基本持平；SGR 与 Unicode 差异一并保留。共享机器的三组样本不足以确定微小差异，最终整合仍需复核。

## 原始材料与恢复步骤

以下开发机文件已生成，但尚未取回归档。本记录保留精确绝对路径，不将它们伪装成公开下载链接：

```text
/home/lightjunction/Documents/GITHUB/ztty/.worktrees/core-perf-20261119/artifacts/core-scroll/raw-ab-final.json
/home/lightjunction/Documents/GITHUB/ztty/.worktrees/core-perf-20261119/artifacts/core-scroll/summary-ab-final.json
/home/lightjunction/Documents/GITHUB/ztty/.worktrees/core-perf-20261119/artifacts/core-scroll/run_ab.py
/home/lightjunction/Documents/GITHUB/ztty/artifacts/performance-function-20261119/docs/measurements/performance-function-2026-11-19/scrollback-baseline.json
/home/lightjunction/Documents/GITHUB/ztty/artifacts/performance-function-20261119/docs/measurements/performance-function-2026-11-19/scrollback-gui.json
/home/lightjunction/Documents/GITHUB/ztty/artifacts/performance-function-20261119/artifacts/initial-validation/summary.json
/home/lightjunction/Documents/GITHUB/ztty/artifacts/performance-function-20261119/artifacts/final-validation/summary.json
/home/lightjunction/Documents/GITHUB/ztty/artifacts/keypad-review-20261119/artifacts/input.json
/home/lightjunction/Documents/GITHUB/ztty/artifacts/preview-audit-20261119/result.json
/home/lightjunction/Documents/GITHUB/ztty/artifacts/preview-fix-20261119/artifacts/preview-fix/gui/sixel-preview.json
/home/lightjunction/Documents/GITHUB/ztty/artifacts/preview-fix-20261119/artifacts/preview-fix/no-image-ab.json
/home/lightjunction/Documents/GITHUB/ztty/artifacts/preview-fix-20261119/artifacts/preview-fix/no-image-summary.json
/home/lightjunction/Documents/GITHUB/ztty/artifacts/preview-fix-20261119/artifacts/preview-fix/run_no_image_ab.py
/home/lightjunction/Documents/GITHUB/ztty/artifacts/broker-upgrade-20261119/artifacts/broker-upgrade/acceptance.json
```

上述自动化 GUI 检查均使用私有 Xvfb、真实 PTY 和自有测试进程，未接管用户窗口。Windows 只做过交叉编译，macOS 未做新增原生验收；物理键盘、完整 fcitx 输入法组合和输入到显示延迟也未完成测量。

**请在开发机的 ztty 项目目录重新运行 ssmcp，恢复访问。** 随后继续读回综合任务结果、归档原始数据与截图、验收本地恢复分支，再制作并安装统一候选，核对新窗口实际执行文件，同时保留现有会话。这份报告是连接中断时的阶段保存，不是最终验收完成记录。
