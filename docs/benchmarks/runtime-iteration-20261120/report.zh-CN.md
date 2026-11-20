# ztty 运行时迭代与安装验收（2026-11-20）

## 结论与范围

本轮修复启动时孤立的 `%`、X11 INCR 粘贴首块丢失竞态，以及无状态能力查询导致的过度 reload 拒绝；GPU 路径的软件回退像素缓冲改为按需分配。默认后端改为 Auto，并保存 detach/attach 的后端来源。最终源码通过 25 个验收步骤，随后安装并通过真实 Hyprland/XWayland 桌面启动检查。[完整数值、原始样本与证据摘要](results.json)与本报告一起保存。

这次交付是版本号仍为 `0.5.2` 的源码构建，不是新的正式发行版。验收步骤包含构建、静态检查、Rust 测试、GUI、协议和内存采样，不能理解为只有 25 个测试用例。

| 身份 | 值 |
| --- | --- |
| 验收运行时提交 | `f9ccdd369faa730aaa9f6cc9902d86d93ef69723` |
| 最终 ELF SHA-256 | `c1bf6c584dbc193f64b7e3a3c9a4da8a7b29f1e01e6901f4f00f0d9a56f8780d` |
| 源码清单 SHA-256 | `a3e279efe1ff541960d690a568f484b60638013d6be897d2659cf4f56aa1401f` |
| zgraphics gitlink | `aa0ed19fb706ad147aff1df54c4ef762d918e864` |
| zterm gitlink | `f1d68693d652d6d5341f1e2953ba89ab68fa4514` |

zgraphics 与 zterm 的新增接口作为匹配的源码组合验证；子库修复位于 `fix/query-only-checkpoint-20261120` 分支。没有发布新 crate，也没有改变子库主分支。独立使用旧 registry zgraphics 的 zterm 构建不在这次验收范围。

## 1. 启动时的 `%`

zsh 的 `PROMPT_SP` 会用尾行标记提示没有换行的输出。旧实现可能先处理窗口缩窄，再按新宽度解析已经排队的旧宽度提示符填充，于是原本应被覆盖的 `%` 留在上一行。修复在独立 X11 窗口改变网格与 PTY 尺寸前，先按既有有界预算处理已排队的本地 PTY 输出；没有关闭 `PROMPT_SP`，也没有隐藏真正的未换行输出。[zsh 选项说明](https://zsh.sourceforge.io/Doc/Release/Options.html#Prompting)

`tests/resize_prompt_gui.py` 覆盖 padding 0/28、宽度不变、真实未换行内容和排队输出遇到缩窄六种组合。旧对照复现两个目标失败，新候选六项通过；真实 `x%` 标记仍保留。安装后的实际桌面截图也没有孤立 `%`。

## 2. 内存：删除闲置的软件回退缓冲

GPU 窗口不再提前持有完整尺寸的软件 pixmap，只保留 Xft 所需的 1×1 占位；真正进入软件渲染或回退时才分配完整缓冲。resize、隐藏/显示、回退和重新启用 GPU 都有状态与重绘检查。

固定对照使用私有 Xvfb、Mesa llvmpipe、`LIBGL_ALWAYS_SOFTWARE=1`、1200×800 像素窗口、DejaVu Sans Mono 13 和 2000 行历史。每个渲染路径覆盖七个生命周期阶段，各阶段取三次原始样本。下表是 XRes 实测的客户端 pixmap 字节，不是尺寸估算。

| 路径与阶段 | 旧版 XRes pixmap | 最终版 XRes pixmap | 差值 |
| --- | ---: | ---: | ---: |
| GPU，1 个空白窗口 | 3,840,000 B | 4 B | −3,839,996 B，约 3.66 MiB |
| GPU，10 个空白窗口 | 38,400,000 B | 40 B | −38,399,960 B，约 36.62 MiB |
| 软件，1 个空白窗口 | 3,840,000 B | 3,840,000 B | 0 B |
| 软件，10 个空白窗口 | 38,400,000 B | 38,400,000 B | 0 B |

这些数字只证明消除了该软件回退资源。XRes 不是进程 RSS/PSS，也不覆盖全部驱动、字体或 GPU 内存；不能把 40 B 写成十个终端的总内存。两个 GPU 采样运行中，broker 单窗口 PSS 为 133,542,912→137,940,992 B，十窗口为 308,608,000→313,128,960 B，并未下降；X server 十窗口 PSS 为 125,675,520→87,457,792 B。不同运行的 PSS 还受共享页归属和宿主负载影响，不能把固定的 pixmap 节省量等同为整进程收益。关闭九个窗口后仍保留的驱动分配也没有在本轮解决。

真实桌面启动使用 Intel UHD 硬件驱动报告的 OpenGL 路径。一次 1225×1526 窗口快照中，broker RSS 为 102,211,584 B、PSS 为 89,988,096 B，客户端 XRes pixmap 合计 11,216,104 B；硬件路径还拥有其他 pixmap。这是单次安装检查，没有对应硬件旧版配对，不能据此声称硬件内存降低了某个百分比。

此前 GPU 文本 run 暂存复用保留在本轮源码中：固定暖机场景的该路径 alloc/realloc 计数从 ASCII/Unicode/样式文本 27/76/227 降为 0，最多保留约 6 KiB 的 run 容量。它不是整个进程零分配；时间结果不足以支持明显帧率提升。详见[文本暂存报告](../text-run-scratch-20261120/report.zh-CN.md)。

## 3. 粘贴：完整性修复与耗时边界

INCR 是 X11 分块传输大剪贴板内容的机制。消费头部时，`XGetWindowProperty(delete=True)` 已经完成删除确认。旧实现随后再次删除同一属性；如果 owner 已及时写入第一块正文，这次多余删除会把尚未读取的正文擦掉。修复删除了重复确认，保留原来的 flush 与分块流程。[Xlib 手册](https://xorg.freedesktop.org/archive/current/doc/libX11/libX11/libX11.html)

确定性回归夹具只在测试进程中控制 owner 与读端的时序：64/256 KiB、bracketed paste 开/关共四个场景，旧版四次均复现目标数据丢失，新版四次完整接收。内容包含跨块 UTF-8、控制字符过滤与精确框定，按实际字节和 SHA-256 校验，并用新 DSR 回复确认输入次序。该夹具人为同步时序，不能作为速度基准。

最终候选另完成 36 次正常传输/输入对照，保留每次实际接收字节、散列和分阶段时间；另外 13 项 OSC 52 检查通过。正常输入测量包含剪贴板路径与 broker 输入对照，具体 transport、trigger 和重复次数以 `results.json` 为准。

一次独立耗时诊断中，旧运行构建的 64 KiB direct/INCR 传输到 raw PTY 约 0.96–1.42 ms，256 KiB 约 3.03–5.31 ms。它与最终验收中的 36 次样本不是同一实验，也不是本轮前后加速结论。

独立内核 PTY 实验绕过 ztty 和剪贴板，使用实际安装的 zsh 5.9.2 与 suite 配置。12 个样本全部通过字节校验；256 KiB 的 clean zsh bracketed-paste widget 约 370–380 ms，suite 约 350–354 ms，suite 完成 redraw hooks 还观察到约 11.6–12.7 ms。这个级别的 shell 编辑耗时仍存在，关闭套件插件没有得到可支持的提速证据。写端耗时包含背压，会与 widget 处理重叠；不可把两个实验相减或直接相加来推导 ztty 开销。hook 完成也不是屏幕最终像素完成。

未部署批量预读的 shell 快速粘贴实验：`sysread`→`zle -U` 会丢失 NUL/后续内容；带长度的 C 注入虽保留 NUL，却可能把 END 后输入转移到 ZLE 私有队列，后续非 ZLE reader 收不到。另一个逐字节轮询实验的耗时波动重叠，没有稳定收益。本轮没有替换用户 shell 或牺牲输入语义。

## 4. Auto：先保留当前功能，再考虑初始回退

当前源码默认 `--backend auto`。Auto 优先完整初始化 X11/XWayland 视图，以保留 tabs、splits 与当前 executable reload 能力。只有初始 X11 初始化失败、原生 Wayland 可用且 broker 池兼容时，才在创建 PTY 和启动命令之前回退。显式 `--backend x11/wayland` 不回退；后续 RPC、显示或配置失败不重放 `-e` 命令。

已附着视图保持后端；detach 后记录最后成功附着的后端，Auto reattach 恢复该后端与原 PTY。已知 X11 会话不会因缺少 DISPLAY 就被静默迁往 Wayland。现有仅 X11/headless 的池也不会因新窗口回退而失去整池 reload 能力。23 项后端检查与 13 项兼容协议检查通过，包括 detached native 会话跨同 PID exec 后恢复原 PTY。

这是一套按当前支持能力选择的策略，没有逐机测出全局最快后端。原生 Wayland 仍不支持 tabs/splits；活动原生视图会阻止整池 executable reload。分数缩放、跨屏缩放与真实桌面输入法组合没有在本轮验收。构建启用了 `wayland` feature，默认 Cargo feature 集没有改变。详见[配置说明](../../configuration.md#native-wayland-experimental)。

## 5. reload：允许无状态查询，继续保护有状态资源

旧实现将能力探测与真正的图形/UI 状态混在同一个不可逆历史标记中。新实现允许已完成、没有持久资源或未完成输入的查询，并在 checkpoint 中保留已排队回复。真实 Kitty/Sixel/UI 状态、未完成查询/传输（包括零字节 pending）以及原有不支持操作继续拒绝；删图或 RIS 不会把真实非查询历史清成安全状态。

验收中发现并修复 ED2 回归：普通 `CSI 2 J` 原先经内部 `Ga=d` 清图路径，错误地标记为外部图形操作。新增窄接口保留原有清屏、取消 pending、回复、revision 与 cursor 行为，同时保留进入时的历史位。它不能覆盖任意协议的历史，也不会把既有不安全状态改成安全。zgraphics 新增两项、zterm 新增五项针对性测试。

`tests/reload_queries.py` 的四组综合检查包含多次真实同 PID exec、拒绝路径、原 child/PTY 身份、像素与后续输入输出校验；通用 reload GUI 另有九项检查。已经运行的旧 broker 仍执行旧代码，不能靠安装新 CLI 追溯清除旧的保守标记。本次没有重载或结束用户旧会话；默认新窗口使用按安装 ELF 散列区分的新池。

## 6. 验收、安装与可复现性

| 验收组 | 最终结果 |
| --- | --- |
| Workspace Rust | 208 passed；8 个 GPU 测试在后续显式执行 |
| 原生 GPU Rust | 8 passed |
| zgraphics / 文档测试 | 58 + 1 passed |
| zterm / 文档测试 | 193 + 3 passed；1 ignored |
| Clippy、fmt、diff、无 Wayland 构建、Wayland release、no_std | 全部成功 |
| 启动 / Auto 协议 / Auto 后端 | 6 / 13 / 23 |
| GPU 文字 / GPU 重绘 / 软件重绘 / pixmap 生命周期 | 8 / 11 / 11 / 17 |
| 通用 reload / 查询 reload 综合组 | 9 / 4 |
| INCR 竞态 / 正常传输与输入对照 / OSC 52 | 4 / 36 / 13 |
| GPU 与软件内存采样 | 各 7 阶段 × 3 次 |

重绘计数夹具曾在未改动的旧对照上复现同一失败：开始下一项断言前，上一个 frame 尚未安静。增加有界的呈现安静期后，旧对照与新候选都通过，原来“恰好一次 copy”断言没有放宽。首次失败和旧对照证据仍保留。ED2 首次失败同样保留，没有改写失败报告来制造通过。

安装验收把 runtime、native、子模块 gitlink、tests、tools 的内容与文件模式绑定到最终提交，每个 gate 保留实际 argv、exit code、日志与报告摘要。安装器再次检查所有含 report 的 receipt，包括 OSC 52；篡改 OSC 52 摘要的负例被拒绝。安装 ELF 与已验收 ELF 完全一致，37 个非 ztty Cargo 安装记录保持原样。

默认入口为 `oh-my-ztty-local.desktop`，运行保留 suite 环境的本地启动器。真实桌面检查发现此主机只有 `X0_` 路径时，原本的 XWayland 别名恢复未随新启动器带上；补回原有兼容逻辑后，实际入口、同一 ELF、Ink 提示符和 PTY 尺寸一致性全部通过。只关闭了唯一命名的检查会话；未重载用户 broker。初次失败报告与修正后成功报告分别保留。

历史清理过程中，上游 main 已去除二进制工件并重写提交。本轮变更基于新的 main 重放，没有合并回旧历史。原性能基线 `9c0d8c3…` 的声明源码与重写后的 `c01cd39…` 完全等价；内存基线 `76a6e0c…` 与 `d884554…` 同样验证源码等价。原始测量的提交标识与数值未被改写，映射只用于校验祖先关系。截图、ELF、压缩日志继续留在忽略的工件目录，遵循[工件存储规则](../../artifact-storage.md)。

复现时先检出上述运行时提交及其递归子模块，再使用 `results.json` 的 gate argv（`$WORK`、`$REPO`、`$HOME` 为已脱敏路径占位）。主要夹具为 `tests/resize_prompt_gui.py`、`tests/auto_backend_gui.py`、`tests/clipboard_incr.py`、`tests/reload_queries.py`、`tools/measure_paste_stages.py`、`tools/measure_shell_paste.py` 和 `tools/measure_memory_stages.py`。无状态查询、内存与剪贴板结果都只适用于记录的有限测试场景；本报告没有宣称所有合成器、GPU、输入法或协议组合已被覆盖。
