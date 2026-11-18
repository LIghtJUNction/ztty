# ztty 与 Kitty、Ghostty 的差距

核对日期：2026-11-18（Asia/Taipei）。ztty 功能快照：[`19274145`](https://github.com/LIghtJUNction/ztty/commit/19274145e37a591d800f638fe21c2717730b6431)。对照资料为当天读取的项目官方文档；性能数据单独标明历史源码和测试环境。

## 结论

ztty 已具备共享会话、分屏、搜索、配置热更新和 MCP 的基础。最需要补齐的部分是复杂文字与历史重排、Linux 原生显示与输入、图片协议完整度，以及 macOS/Windows 与 Linux 的功能一致性。现有证据支持特定场景中的低内存优势，整体功能和性能仍有明确差距。

## 日常使用差距

| 方面 | ztty 的已实现能力与边界 | Kitty / Ghostty 的对应能力 | 下一步的实际目标 |
| --- | --- | --- | --- |
| Linux 显示 | X11/XWayland、Xft、Xrender；原生 Wayland、专用 GPU renderer 未完成 | Kitty 使用 GPU；Ghostty 在 Linux 使用 OpenGL、macOS 使用 Metal，支持平台原生界面 | 原生 Wayland、多屏缩放、呈现延迟与 GPU 内存验收 |
| 复杂文字 | 支持宽字符；每格存两个组合码点，完整 shaping、emoji ZWJ、历史 reflow 未完成；缩窄会裁剪旧行 | 两者支持字体连字；Ghostty 明确支持 grapheme clustering 与多码点 emoji | 建立逻辑行和 grapheme 模型，让光标、选择、搜索、图形锚点在重排后保持正确 |
| 输入法 | Linux 有 XIM/Fcitx 路径；IBus、原生 Wayland text-input 与 macOS 输入法仍需完整验收 | 对手已有原生平台输入路径；Ghostty 有 CJK、dead keys 与多种输入法的测试记录 | 覆盖预编辑、候选框、布局切换、死键与组合键 |
| 图片 | 静态 Kitty RGB/RGBA/PNG、直接 PTY 传输及 Sixel；文件/共享内存传输、动画、Unicode placeholder、relative placement 未完成 | Kitty 定义完整图形协议；Ghostty 1.4.0 已公布补全动画和相对放置 | 优先补实际 TUI 依赖的传输和布局能力，再做动画；继续保持资源上限 |
| 剪贴板与远程使用 | 有本地 clipboard/primary 与 INCR 接收；OSC 52 尚未实现 | 对手已有远程剪贴板和 SSH 配套能力 | 带明确读写权限的 OSC 52，以及 SSH/terminfo 自动配置 |
| 搜索与工作区 | Linux 有跨软换行字面搜索、会话切换、palette、tabs/splits、布局保存恢复；查询限1024字节；macOS/Windows 尚无相同搜索/palette界面 | Kitty 有布局、sessions、scrollback pager/kittens；Ghostty 自1.3.0起有 macOS/GTK 原生搜索和独立搜索线程 | 补齐跨平台界面；为大历史搜索设可测交互预算 |
| Shell 集成 | Bash/Zsh/Fish/Nushell 显式启用；OSC133/7、命令导航、输出提取已有；区域会随历史裁剪失效 | Kitty、Ghostty 有自动注入和日常 shell/SSH 配套；Ghostty 支持提示符点击移动光标等行为 | 降低手工配置量，同时保留用户 startup/hook 行为 |
| 平台 | Linux、macOS、Windows 均有原生实现；broker、attach、MCP 目前仅 Linux；正式发布仍需精确源码的原生验收 | Kitty 覆盖 Linux/macOS/BSD；Ghostty 官方覆盖 macOS/Linux，Windows 仍列为计划 | 让三端输入、搜索、会话与发布质量一致 |
| 自动化 | MCP stdio/HTTP、分页与等待、会话身份、只读观察和输入所有权是现有重点 | Kitty 已有 `kitten @` 和 socket 控制；Ghostty macOS 已有 AppleScript | 深化可靠的会话生命周期、可恢复读取和明确所有权，避免只追求工具数量 |

ztty 的事实依据是该快照的 [README](https://github.com/LIghtJUNction/ztty/blob/19274145e37a591d800f638fe21c2717730b6431/README.md)、[协议范围](https://github.com/LIghtJUNction/ztty/blob/19274145e37a591d800f638fe21c2717730b6431/docs/protocols.md)、[日常工作流](https://github.com/LIghtJUNction/ztty/blob/19274145e37a591d800f638fe21c2717730b6431/docs/daily-workflow.md)、[会话说明](https://github.com/LIghtJUNction/ztty/blob/19274145e37a591d800f638fe21c2717730b6431/docs/sessions.md) 和 [屏幕模型](https://github.com/LIghtJUNction/ztty/blob/19274145e37a591d800f638fe21c2717730b6431/crates/zterm/src/screen.rs)。表中的优先级与目标是工程判断。

## 性能证据

下表来自仓库已有的[受控空闲对比](https://github.com/LIghtJUNction/ztty/blob/19274145e37a591d800f638fe21c2717730b6431/docs/benchmarks/guarded-idle-1-10-2026-11-18.md)。测试使用历史 ztty `fa2f2017`（0.2.0）、Ghostty 1.4.0-arch1 和 Kitty 0.50.0，在 Linux Xvfb 软件显示、共享实例模式下，每格执行三次新进程测量。

| 指标 | ztty | Ghostty | Kitty |
| --- | ---: | ---: | ---: |
| 单窗口应用进程 RSS | 22.02 MiB | 340.38 MiB | 209.52 MiB |
| 10窗口应用进程 PSS | 20.31 MiB | 817.51 MiB | 360.05 MiB |
| 同次10窗口场景的单生产者输出处理 | 21.92 MiB/s | 138.50 MiB/s | 48.83 MiB/s |

这次测试中，ztty 的内存用量较低，输出吞吐低于两者。后续已有独立的吞吐优化实验；这些实验的源码和条件不同，需要重新做统一三方测量才能更新排名。

RSS 是各进程驻留页面计数；PSS 按映射进程分摊共享页面。单窗口 RSS 小于10 MiB 的目标仍未达到。该历史对比没有测原生 GPU/Wayland、GPU/驱动内存、第一帧实际呈现、按键到屏幕延迟、多生产者同时输出或 macOS/Windows 性能。协议查询的应答速度也不等于屏幕已经显示了那一帧。

## 开发顺序

1. **可靠性和交付闭合。** 优先修复实际输入、resize、选择、搜索与主/备用屏幕问题，并统一源码、制品和实际启动路径。本轮已合并的 [PR7](https://github.com/LIghtJUNction/ztty/pull/7) 与 [PR8](https://github.com/LIghtJUNction/ztty/pull/8) 分别处理高度变化覆盖行尾、Shift标点快捷键不生效。
2. **文本模型。** 将完整逻辑行 reflow、grapheme、光标和选择坐标一起设计，避免每次增加文字能力都独立修补坐标。
3. **原生桌面体验。** 推进 Wayland 与输入法矩阵，再用测量决定 GPU renderer 的实现次序和性能目标。
4. **高频远程工作流。** 补 OSC52、SSH/terminfo 与低侵入 shell 自动集成。
5. **平台一致性与 AI 接口。** 复用公共核心补齐 macOS/Windows 的交互能力；继续加强 MCP 的身份、读取和控制语义。新的智能UI协议需要独立定义组件、更新、事件和图片边界，并给出可复现性能结果。

## 官方对照来源

- [Kitty 概览](https://sw.kovidgoyal.net/kitty/)：GPU、字体、窗口布局与平台。
- [Kitty 图形协议](https://sw.kovidgoyal.net/kitty/graphics-protocol/)：传输、动画、占位符与相对放置。
- [Kitty 远程控制](https://sw.kovidgoyal.net/kitty/remote-control/)：脚本、socket、窗口与输入控制。
- [Ghostty 功能](https://ghostty.org/docs/features)：GPU、grapheme、平台与原生组件。
- [Ghostty 1.4.0](https://ghostty.org/docs/install/release-notes/1-4-0)：2026-10-27发布，新增完整图形协议能力。
- [Ghostty 1.3.0](https://ghostty.org/docs/install/release-notes/1-3-0)：原生搜索、搜索线程、滚动条与提示符交互。
- [Ghostty shell 集成](https://ghostty.org/docs/features/shell-integration)：自动注入与命令区域交互。
- [Ghostty AppleScript](https://ghostty.org/docs/features/applescript)：macOS 自动化。

## 证据核对

已实现功能对应固定源码；对手能力对应官方文档；性能数值对应同一份历史报告。开发优先级属于判断，未完成能力与未测指标均明确列出。本文不把历史内存数据、新的吞吐实验和未测输入延迟合并成整体排名。
