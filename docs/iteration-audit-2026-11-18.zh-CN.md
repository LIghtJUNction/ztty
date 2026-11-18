# ztty 迭代与剩余不足

日期：2026-11-18（Asia/Taipei）  
审查基线：main `9b5497dac5402603ba0f633152fa4b07e6c11c9e`  
发布状态快照：2026-11-18 22:37（Asia/Taipei）

## 本轮结果

本轮提交了三项修复候选，并补充对应回归用例。终端状态改动位于 [PR #5](https://github.com/LIghtJUNction/ztty/pull/5)，MCP 会话身份保护位于 [PR #6](https://github.com/LIghtJUNction/ztty/pull/6)。两个 PR 保持草稿，等待开发机完成修复后的编译和运行验收。main 尚未合入这些改动，新版本尚未发布。

### 备用屏幕的清理时机

1047 模式原先在进入备用屏幕时清空内容，退出时又保留了内容。修复把清理放在退出、交换回主屏之前；文字和图片同时清除。重新进入 1047 会保留原有备用屏幕内容，主屏收到 1047 reset 时保持不变。

这解决的是全屏程序之间的状态残留。依据是 [XTerm Control Sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html) 对 1047 set/reset 的规定。提交：[2cedbb1](https://github.com/LIghtJUNction/ztty/commit/2cedbb1d7106e9b8d8c4fc5ea5b7a7468bed9178)。

### 保存和恢复光标时的字符集状态

原实现只保存当前字符集的一个值，没有保存 G0/G1 两套定义及当前选择器。程序在保存、恢复之间切换字符集时，恢复操作可能改坏另一套定义，造成普通字母被显示为画框符号。

候选现在完整保存和恢复 G0、G1、use_g1。用例覆盖 ESC 7/8、1048、1049，两种活动选择器，以及主屏和备用屏幕各自的保存槽。测试先检查恢复后的第一个字符，再发送 SI/SO，避免新输入掩盖选择器恢复错误。依据是 [VT220 Programmer Reference Manual §4.7](https://www.zx.net.nz/computers/dec/vt220/doc/vt220-rm/chapter4.html)。提交：[91ae4e8](https://github.com/LIghtJUNction/ztty/commit/91ae4e8961e5f97d4366e162ab638695b1b1144b)。

### Agent 对会话的迟到操作

会话 A 关闭后，可以用同一个名字创建 B。原来的写入、调整尺寸、控制权和关闭请求仅按名字或 URI 选择会话，缺少读取操作已有的身份约束。因此，针对 A 的迟到请求可能作用于 B。

候选为 session_write、session_resize、session_control、session_close 增加可选 expected_session。broker 在变更前核对 uri、id、created_at、broker_pid。身份变化时返回明确错误。输入所有权校验仍生效；未传 guard 的客户端继续使用已有行为。身份 guard 不负责请求去重，正确身份的重复写入仍会执行两次。

回归覆盖 stdio/HTTP 两种传输的同名重建、陈旧请求、各身份字段不匹配、短名选择器、正确身份、无 guard 兼容和跨 actor 权限。提交：[0971198](https://github.com/LIghtJUNction/ztty/commit/0971198a1e6e7a7526b5d3d6774cd6281bd16f25)。接口说明见该提交中的 [sessions.md](https://github.com/LIghtJUNction/ztty/blob/0971198a1e6e7a7526b5d3d6774cd6281bd16f25/docs/sessions.md)。

## 本轮验收状态

| 检查 | 实际结果 | 范围 |
| --- | --- | --- |
| 1047 修复前回归 | 2 个失败、1 个通过 | 两个失败分别复现进入误清、退出残留；主屏 reset 无操作用例通过 |
| 核心修复独立审查 | 完成 | 清理顺序、主副屏隔离、字符集恢复及测试覆盖 |
| MCP 修复独立审查 | 完成 | 身份检查位置、权限保留、参数校验、异步输出断言 |
| 修改的 Python 测试语法 | 两个文件 ast.parse 通过 | mcp_sessions.py、mcp_transport.py |
| 修复后 Rust 编译、格式和回归 | 尚未取得执行结果 | 开发机 ssmcp 执行通道反复超时 |
| 新增真实 MCP/PTY 回归 | 尚未取得执行结果 | 测试已提交，执行待恢复 |
| main 合并、新版本发布 | 未完成 | 本轮验收与 Windows 正式发布门槛仍待通过 |

修复前结果摘要见 [terminal-state-regressions-2026-11-18.json](measurements/terminal-state-regressions-2026-11-18.json)。仓库既有测试通过记录属于各自源提交，不作为本轮候选的通过结果。

开发机连接曾恢复到可以读取工作树和进程状态，随后执行再次超时。最后一次成功读取时，本轮终端工作树只有新增的三个 1047 回归测试，生产源码仍为基线；MCP 独立工作树保持基线且干净。后续超时请求的执行结果需要恢复连接后先确认，再同步候选并运行检查。

## 还存在哪些不足

| 优先级 | 不足 | 用户实际影响 | 证据和状态 |
| --- | --- | --- | --- |
| P0 | 正式发布未闭环 | 当前安装 latest 仍得到 v0.1.2，新源码能力尚未形成新的完整发行版 | v0.3.0 tag 的 [44309412482](https://github.com/LIghtJUNction/ztty/actions/runs/44309412482) 失败；最新 Windows 候选 [44324788860](https://github.com/LIghtJUNction/ztty/actions/runs/44324788860) 的 x64、ARM64 原生像素验收均失败 |
| P1 | 窗口缩放后的文本重排不完整 | 横向缩窄会裁掉当前屏幕右侧内容；历史搜索和复制受当前列宽限制，长日志的逻辑行不完整 | [daily-workflow.md](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/docs/daily-workflow.md)、Screen::resize；已确认实现边界 |
| P1 | Unicode 排版仍有限 | 基础 CJK 已支持；复杂 emoji、阿拉伯文等连写、较多组合附加符仍可能显示或复制不完整 | Cell 每格仅两个 combining scalar；完整 emoji ZWJ、复杂 shaping 待补。[协议范围](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/docs/protocols.md) |
| P1 | 跨平台日常工作流不齐 | Linux 的共享 broker、MCP、attach、搜索、命令面板、配置热更新未在其他平台形成同等能力 | [README 平台表](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/README.md)、[daily-workflow.md](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/docs/daily-workflow.md) |
| P1 | 性能目标和测量矩阵未完成 | 目前没有足够完整的硬件桌面数据评价相对 Kitty/Ghostty 的整体响应和内存效率 | 已披露的 fa2f2017 单窗口记录为 22.02 MiB RSS，低于 10 MiB 的目标未达；物理输入到显示延迟、GPU 内存、完整窗口/负载矩阵仍待测。这是历史源记录，并非本轮候选实测。[README](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/README.md) |
| P2 | 长期会话的超链接表不回收 | 累计 1024 个不同链接后，新链接条目停止分配，旧文本即使已淘汰也不会释放链接槽 | MAX_LINKS 与 OSC 8 追加路径；源码审查确认，端到端复现待做。[terminal.rs](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/crates/zterm/src/terminal.rs) |
| P2 | ANSI 新行模式输入端未接通 | 程序开启 LNM 20 后，屏幕端处理换行，但普通 Return 仍发送 CR，未随模式发送 CR+LF | State.newline 未传入 Modes::input，legacy Enter 固定 CR；源码与规范已核对，真实 PTY 复现待做。[zinput](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/crates/zinput/src/lib.rs) |
| P2 | Agent 输出记录缺少持久保留 | 慢客户端会遇到输出截断；broker 退出后的长任务和历史输出不能依赖布局快照恢复 | raw output 为 256 KiB 滚动保留，最多 128 条 shell command 元数据；布局恢复依赖仍存活的会话。[sessions.md](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/docs/sessions.md) |
| P2 | Linux 原生桌面和字体表现仍待完善 | 当前依赖 X11/XWayland；完整原生 Wayland、专用 GPU 渲染、真实斜体等体验仍缺 | Linux 使用 Xft/Xrender，core 保留 italic 属性但 Linux 使用配置字体；未做完整外观一致性验收。[README](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/README.md) |
| P2 | 部分终端扩展协议未实现 | 远程剪贴板、部分图片客户端及高级图像能力有明确边界 | OSC 52、Kitty file/shared-memory transport、动画、Unicode placeholders、relative placements 待补。[protocols.md](https://github.com/LIghtJUNction/ztty/blob/9b5497dac5402603ba0f633152fa4b07e6c11c9e/docs/protocols.md) |

macOS 和 Windows 已有原生实现，也有此前的原生像素验收记录；Windows 已实现 tabs/splits。基础中文、字体 fallback、静态图片、Sixel、Linux 搜索和 shell 集成都已有实现。剩余工作应围绕完整性、状态正确性、跨平台一致性及最终源提交验收推进。

## 建议迭代顺序

1. 恢复开发机执行通道，完成本轮候选的 Rust 和真实 PTY 验收，合并通过的修复。
2. 关闭 Windows 发布问题；由同一个最终源提交完成六个原生架构门槛，发布后再核对实际制品和安装版本。
3. 优先实现历史重排、缩放后的文本保真以及完整 Unicode 字符簇处理。
4. 修复长寿命会话的 OSC 8 回收与 LNM 输入语义，补充持续运行和真实终端应用回归。
5. 在真实桌面上补齐输入到可见像素、满历史、图片和复杂 TUI 的性能矩阵，再据结果决定渲染架构投入。

本轮没有增加第三方依赖。两项终端核心修改采用固定大小的状态字段和既有清理路径；MCP schema 复用发生在编译期。

## 证据自检

- 修复贡献：三个具体状态问题均对应源码路径、候选提交和回归场景。
- 文字与范围：已提交、已复现、静态审查、待运行和未发布分别标注。
- 实验强度：现有执行证据只覆盖 1047 的修复前复现及两个 Python 文件的语法检查。
- 验证完整性：修复后原生结果仍缺，PR 保持草稿。
- 设计边界：身份 guard 可选、不是授权凭据、不提供去重；历史性能数据保留原源提交与环境范围。
