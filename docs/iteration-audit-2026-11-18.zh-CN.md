# ztty 迭代与剩余不足

更新日期：2026-11-18（Asia/Taipei）  
初始审查基线：main `9b5497dac5402603ba0f633152fa4b07e6c11c9e`  
本轮整合验收提交：`c497fba0cb8bcfe53bacb9f4142a5eb73902f263`

## 本轮结果

开发机执行通道已恢复。本轮的终端状态修复 [PR #5](https://github.com/LIghtJUNction/ztty/pull/5) 和 MCP 会话身份保护 [PR #6](https://github.com/LIghtJUNction/ztty/pull/6) 已完成本机验收并合入 main。并行推进的窗口高度变化后自动换行修复 [PR #7](https://github.com/LIghtJUNction/ztty/pull/7) 也已合入。随后在独立工作树对包含这三个 PR 的同一源提交重新验收：280 项 Rust 测试、6 项文档测试、8 项 SessionCore 真实 PTY 检查、全工作区格式和严格 Clippy 检查全部通过。

正式新版本尚未发布。截至下述固定运行，Windows x64 候选已通过原生验收，Windows ARM64 仍在缩放后的图片恢复检查失败。已发布的 latest 仍为 [v0.1.2](https://github.com/LIghtJUNction/ztty/releases/tag/v0.1.2)。

### 备用屏幕的清理时机

1047 模式原先在进入备用屏幕时清空内容，退出时又保留了内容。修复把清理放在退出、交换回主屏之前；文字和图片同时清除。重新进入 1047 会保留原有备用屏幕内容，主屏收到 1047 reset 时保持不变。

这解决的是全屏程序之间的状态残留。依据是 [XTerm Control Sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html) 对 1047 set/reset 的规定。实现提交：[2cedbb1](https://github.com/LIghtJUNction/ztty/commit/2cedbb1d7106e9b8d8c4fc5ea5b7a7468bed9178)。

### 保存和恢复光标时的字符集状态

原实现只保存当前字符集的一个值，没有保存 G0/G1 两套定义及当前选择器。程序在保存、恢复之间切换字符集时，恢复操作可能改坏另一套定义，造成普通字母被显示为画框符号。

现在完整保存和恢复 G0、G1、use_g1。回归覆盖 ESC 7/8、1048、1049，两种活动选择器，以及主屏和备用屏幕各自的保存槽。测试先检查恢复后的第一个字符，再发送 SI/SO，避免新输入掩盖选择器恢复错误。依据是 [VT220 Programmer Reference Manual §4.7](https://www.zx.net.nz/computers/dec/vt220/doc/vt220-rm/chapter4.html)。实现提交：[91ae4e8](https://github.com/LIghtJUNction/ztty/commit/91ae4e8961e5f97d4366e162ab638695b1b1144b)。

### Agent 对会话的迟到操作

会话 A 关闭后，可以用同一个名字创建 B。原来的写入、调整尺寸、控制权和关闭请求仅按名字或 URI 选择会话，缺少读取操作已有的身份约束。因此，针对 A 的迟到请求可能作用于 B。

现在 session_write、session_resize、session_control、session_close 接受可选 expected_session。broker 在变更前核对 uri、id、created_at、broker_pid。身份变化时返回明确错误。输入所有权校验仍生效；未传 guard 的客户端继续使用已有行为。调用方需要主动携带 guard 才能获得这项保护；guard 不负责请求去重，正确身份的重复写入仍会执行两次。

stdio/HTTP 真实 PTY 回归均已通过：同名重建之后，旧身份的写入、resize、释放控制、接管和关闭被拒绝；新会话的 PID、尺寸、所有权及输出保持一致。还验证了逐项身份字段不匹配、短名选择器、正确身份、无 guard 兼容和跨 actor 权限。已验证提交：[c0a6aba](https://github.com/LIghtJUNction/ztty/commit/c0a6aba9579dd4849a0aad04c9c8ce9578cbe10f)；接口说明：[sessions.md](https://github.com/LIghtJUNction/ztty/blob/c0a6aba9579dd4849a0aad04c9c8ce9578cbe10f/docs/sessions.md)。

### 窗口高度变化后的待换行状态

并行修复 [PR #7](https://github.com/LIghtJUNction/ztty/pull/7) 保留高度变化时已有的 pending autowrap 状态，避免下一次输入覆盖行尾。该改动已包含在本轮整合验收中。它解决的是自动换行状态丢失，完整的历史文本重排仍有后述边界。

## 本轮验收结果

| 检查 | 实际结果 | 精确范围 |
| --- | --- | --- |
| 1047 修复前回归 | 2 个失败、1 个通过 | 复现进入误清、退出残留；主屏 reset 无操作通过 |
| 字符集修复前回归 | 2 个失败 | 复现字符集定义/选择恢复错误和主副屏保存槽错误 |
| 终端修复候选 | 93 项单元测试、3 项文档测试通过 | 格式和严格 Clippy 也通过；文件哈希对应 d0dac9d |
| MCP 新回归运行旧二进制 | 按预期失败 | 首个 guarded write 返回 -32602 / Unknown tool argument；证明旧版不支持该参数 |
| MCP 修复候选 Rust 验收 | 271 项 Rust 测试、6 项文档测试通过 | SessionCore 内部另外执行 8 项真实 PTY 检查，子进程全部回收 |
| MCP 实际传输与权限 | transport、sessions、security 全通过 | 新增身份保护场景覆盖 stdio 和 HTTP |
| MCP 等待与流式响应 | waiting 13/13；SSE 7/7 | 包括取消、并发、断开、边界、UTF-8、身份变化和资源清理 |
| MCP 构建与 no_std | release 构建、严格 Clippy、格式、no_std 通过 | 检查 11 个生产 crates；候选 release 未发现 Rust std 符号 |
| 合并后同一源提交验收 | 280 项 Rust 测试、6 项文档测试、8 项 SessionCore PTY 检查通过 | c497fba，包含 PR #5/#6/#7；全 workspace 格式与严格 Clippy 通过 |
| main 合并 | 已完成 | PR #5/#6/#7 的 head 均已核验为 c497fba 的祖先 |
| 正式新版本发布 | 未完成 | Windows ARM64 原生图片恢复门槛尚未通过 |

证据文件：

- [终端修复前后结果](measurements/terminal-state-regressions-2026-11-18.json)
- [MCP 身份保护验收](measurements/session-mutation-identity-2026-11-18.json)
- [合并后完整工作区验收](measurements/merged-main-verification-2026-11-18.json)

MCP release 二进制 SHA-256 为 `3c037b67bee176b427539a7c69184acae026009284e9179ac2791846da6b615c`，对应 MCP 候选源码。合并后工作区检查另绑定 c497fba；候选二进制的运行结果不冒充合并后重新构建的二进制结果。本轮按开发机工作规则进行本地验收，没有手动启动日常 CI。

## 发布仍卡在哪里

最新核对的 Windows 候选为 [44330414544](https://github.com/LIghtJUNction/ztty/actions/runs/44330414544)，源提交 [07851a3](https://github.com/LIghtJUNction/ztty/commit/07851a38374f5794e1f0013de0821d0be76ae32b)。参数为 BUILD_ONLY=true、CANDIDATE_PLATFORM=windows，绘制诊断关闭。这是 Windows 候选构建，不是全平台正式发布。

| 平台 | 观察到的原生结果 |
| --- | --- |
| Windows x64 | 8/8 轮冷启动完成，每轮 31 项原始检查、10 次快速 resize、10 次连续 resize 均通过；workspace 19 项通过 |
| Windows ARM64 | 前 5 轮完成，第 6/8 轮在 resize_restores_visible_image 失败；期望红色 [255,0,0]，实际背景色 [20,20,20]；剩余轮次与 workspace 未完成 |

更早的诊断运行显示，失败后同一进程的 CPU buffer 与 source DIB 在失败点为红色，窗口 target 为背景；但 paint-time 探针取了另一处坐标。因此，不能据此确定错误发生在 BitBlt 当时还是绘制结束之后。新的 WM_NCCALCSIZE 候选也仍未解决 ARM64 失败。需要让绘制时探针与原断言使用同一坐标，再收集因果证据。

正式 v0.3.0 的 [44309412482](https://github.com/LIghtJUNction/ztty/actions/runs/44309412482) 也未完成发布。一个平台候选成功，不能替代同一最终源提交的六个原生架构验收。

## 还存在哪些不足

| 优先级 | 不足 | 用户实际影响 | 证据和状态 |
| --- | --- | --- | --- |
| P0 | 正式发布未闭环 | 安装 latest 仍得到 v0.1.2，本轮新修复尚未进入完整发行版 | 上述正式运行失败；最新 Windows 候选 x64 通过、ARM64 第 6 轮图片恢复失败 |
| P1 | 窗口缩放后的文本重排不完整 | 横向缩窄会裁掉当前屏幕右侧内容；历史搜索和复制受当前列宽限制，长日志的逻辑行不完整 | [daily-workflow.md](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/docs/daily-workflow.md)、Screen::resize；高度变化的 pending-wrap 修复不等于完整 reflow |
| P1 | Unicode 排版仍有限 | 基础 CJK 已支持；复杂 emoji、阿拉伯文等连写、较多组合附加符仍可能显示或复制不完整 | Cell 每格仅两个 combining scalar；完整 emoji ZWJ、复杂 shaping 待补。[协议范围](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/docs/protocols.md) |
| P1 | 跨平台日常工作流不齐 | Linux 的共享 broker、MCP、attach、搜索、命令面板、配置热更新未在其他平台形成同等能力 | [README 平台表](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/README.md)、[daily-workflow.md](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/docs/daily-workflow.md) |
| P1 | 性能目标和测量矩阵未完成 | 目前没有足够完整的硬件桌面数据评价相对 Kitty/Ghostty 的整体响应和内存效率 | 历史 fa2f2017 单窗口记录为 22.02 MiB RSS，低于 10 MiB 的目标未达；物理输入到显示延迟、GPU 内存、完整窗口/负载矩阵仍待测。这不是本轮候选实测。[README](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/README.md) |
| P2 | 长期会话的超链接表不回收 | 累计 1024 个不同链接后，新链接条目停止分配，旧文本即使已淘汰也不会释放链接槽 | MAX_LINKS 与 OSC 8 追加路径；源码审查确认，端到端复现待做。[terminal.rs](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/crates/zterm/src/terminal.rs) |
| P2 | ANSI 新行模式输入端未接通 | 程序开启 LNM 20 后，屏幕端处理换行，但普通 Return 仍发送 CR，未随模式发送 CR+LF | State.newline 未传入 Modes::input，legacy Enter 固定 CR；源码与规范已核对，真实 PTY 复现待做。[zinput](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/crates/zinput/src/lib.rs) |
| P2 | Agent 输出记录缺少持久保留 | 慢客户端会遇到输出截断；broker 退出后的长任务和历史输出不能依赖布局快照恢复 | raw output 为 256 KiB 滚动保留，最多 128 条 shell command 元数据；布局恢复依赖仍存活的会话。[sessions.md](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/docs/sessions.md) |
| P2 | Linux 原生桌面和字体表现仍待完善 | 当前依赖 X11/XWayland；完整原生 Wayland、专用 GPU 渲染、真实斜体等体验仍缺 | Linux 使用 Xft/Xrender，core 保留 italic 属性但 Linux 使用配置字体；未做完整外观一致性验收。[README](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/README.md) |
| P2 | 部分终端扩展协议未实现 | 远程剪贴板、部分图片客户端及高级图像能力有明确边界 | OSC 52、Kitty file/shared-memory transport、动画、Unicode placeholders、relative placements 待补。[protocols.md](https://github.com/LIghtJUNction/ztty/blob/c497fba0cb8bcfe53bacb9f4142a5eb73902f263/docs/protocols.md) |

macOS 和 Windows 已有原生实现，也有此前的原生像素验收记录；Windows 已实现 tabs/splits。基础中文、字体 fallback、静态图片、Sixel、Linux 搜索和 shell 集成都已有实现。后续工作应围绕状态正确性、文本保真、跨平台一致性及最终源提交验收推进。

## 建议迭代顺序

1. 解决 Windows ARM64 缩放后图片丢失；先统一失败点与绘制时诊断点，再根据同一像素的前后记录修复。
2. 使用同一个最终源提交通过六个原生架构门槛，正式发布后核对制品、校验和与实际安装版本。
3. 优先实现历史重排、缩窄后文本保真以及完整 Unicode 字符簇处理。
4. 修复长寿命会话的 OSC 8 回收与 LNM 输入语义，补充持续运行和真实终端应用回归。
5. 在真实桌面上补齐输入到可见像素、满历史、图片和复杂 TUI 的性能矩阵，再据结果决定渲染架构投入。

本轮三项修复没有增加第三方依赖。终端核心修改采用固定大小的状态字段和既有清理路径；MCP schema 复用发生在编译期。
