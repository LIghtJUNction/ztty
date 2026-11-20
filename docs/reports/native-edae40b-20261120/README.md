# edae40b：六架构正式原生候选验收

**固定源码 edae40b 的六个原生平台 job 均通过。** Windows 两架构的最大化/恢复失败点已实际通过；
Linux 两架构的 driver、命令重排和输入夹具修复也有本次原始报告支持。
本次运行同时完成原生窗口、PTY 和像素检查，结论不限于“能够构建”。

范围是 [Release run 44656228400](https://github.com/LIghtJUNction/ztty/actions/runs/44656228400)，
attempt 1，源码固定为
[`edae40b03be0207fd062e13bc4304c42bf0d9fc0`](https://github.com/LIghtJUNction/ztty/commit/edae40b03be0207fd062e13bc4304c42bf0d9fc0)。
2026-11-20 01:48:04–01:51:06 UTC 完成，`build_only=true`、`candidate_platform=all`，
两项 Windows diagnostics 均为 false。version 与六个原生 job 成功；Verify 与 publish 按 build-only 路径跳过。
**应用没有因此发布或安装。** 本候选消费已发布 zterm 0.5.0，不覆盖随后独立调查的 0.5.1 性能候选。

## 结果范围

表中均为每个架构自己的记录，不把跨架构重复执行相加成不同功能。

| 平台 | 本次实际检查 | 结果与边界 |
| --- | --- | --- |
| Linux x86_64、aarch64 | driver 17；working-directory 4；专项 EGL 6；17 份 GUI/协议报告共 278 个记录检查 | 全部通过；Xvfb / 软件 Mesa / 私有 headless Wayland，未证明物理 GPU、桌面 IME 或输入到显示延迟 |
| Windows x86_64、aarch64 | gate contracts 41；主原生 31；8 次冷启动各 31；workspace 22 | 全部通过；从实际 runtime ZIP 解包后运行 ConPTY 与 Win32；owned window/DC 像素检查及桌面恢复均记录成功 |
| macOS x86_64、aarch64 | gate contracts 10；Darwin PTY spawn 8 组；AppKit/PTY/图形 24 | 全部通过；ScreenCaptureKit 读取 WindowServer 窗口像素；IME/键盘专项未执行 |

各平台详情和原件摘要见 [acceptance.json](acceptance.json)。
[Jobs](jobs.json)保留每个步骤的状态，构建与原生测试没有合并成一个泛化的“通过”标记。
完整 ZIP、所有内部成员摘要和七份 job log 的保存位置与摘要列在 [evidence-index.json](evidence-index.json)。

## Windows：原失败点已通过

前一候选 6fec 的两架构都在等待 maximized pane C 时失败。
本次 edae40b 只在三 pane 几何初始化前清空本 probe 的 scrollback；
常规 render 保留历史，原 grid、pixel、foreground、PID 与 cleanup 断言保留。
[修复源码](https://github.com/LIghtJUNction/ztty/commit/edae40b03be0207fd062e13bc4304c42bf0d9fc0)
对应原独立提交 10244022。

两个 workspace 原件均记录：

- `maximize_restore_keeps_pty_identity` 通过；蓝色采样为 RGB 0/0/255，背景采样为 0/0/0，
  两个采样点都属于自有、可见窗口。
- 后续 grow/shrink、swap、窗口 resize、tab、字体缩放和自然退出检查全部进入并通过，总计 22 项。
- `pane_history_isolation` 仍通过，说明这次前置条件隔离没有删除原历史检查。
- 所有 probe 退出，临时目录删除；私有 desktop 关闭，input/thread 恢复，`job_pids_after=[]`。

[主验收和 workspace 原始 JSON](reports/)保留全部具体检查。
冷启动八份原件在完整 ZIP 中，其摘要、31 项计数及父回执的 source/binary 绑定记录在 acceptance.json。
不将未捕获的 ConPTY 原始字节回放当作旧失败的已证根因。

## Linux：修复闭环与完整范围

| 记录 | x86_64 | aarch64 | 具体证据 |
| --- | ---: | ---: | --- |
| driver 单元测试 | 17 | 17 | 原 SIGTERM cleanup 用例明确为 ok |
| daily workflow | 19 | 19 | command_reflow 的 before/after 相同；保留 20 B 与 2,872 B 两段命令输出 |
| input GUI | 14 项 / 198 向量 | 14 项 / 198 向量 | `ZTTY_TEST_PC_KEYPAD`；每个向量 ok；自有进程清理为空 |
| working-directory | 4 | 4 | 原生 PTY/CWD 断言通过 |
| 专项 EGL 单元 | 6 | 6 | 原 ignored 测试在本次被实际执行并通过 |

keypad fixture SHA-256 为
`e9f664e17a5e4886f078f0bbf35ce80f4c3878ee17256c964cc13a593d0dc113`。
wrapper 的十个源文件摘要、input 测试文件及该 fixture 都与冻结提交逐字匹配。
input 的 `source_changes` 仅为生成的 `?? dist/`；wrapper 运行前后 tracked changes 均为空。

此前本地独立的 **44 项 XkbLookupKeySym probe** 不是本 workflow 的额外 stage，
未计入本次正式结果。旧 ARM “missing PNG”也没有被当成独立产品故障重新解释。

17 份 GUI/协议报告分别为：

| 场景 | 每架构记录数 |
| --- | ---: |
| X11 wrapper：workspace / daily / switcher / Sixel / input / repaint / native UI | 17 / 19 / 6 / 10 / 14 / 11 / 24 |
| GPU text / UI / repaint | 8 / 24 / 11 |
| retained frame | 2 |
| graphics extensions：GPU / software | 8 / 8 |
| OSC52：GPU / software | 13 / 13 |
| clipboard transfer | 25 |
| shared Wayland | 65 |

Wayland 两架构各 65 项均绑定 edae40b 和各自 ELF，包含七项测试 IME peer 检查及两种混合后端启动顺序。
自有 broker、child、helper 剩余列表均为空。Linux wrapper 自身的“无 Wayland”限制只描述其七个 X11 case；
后续独立 Wayland stage 的原件在 native evidence 中，不能遗漏或混为同一测试范围。

## macOS：原生像素与签名链

两架构均记录 `pixel_readback=WindowServer window via ScreenCaptureKit`、
`ime_keyboard=not_performed`、`remaining_owned_processes=[]`。
24 项覆盖原生 PTY/窗口几何、协议响应、Kitty 图像、Unicode CWD、Sixel、resize、最小化及恢复。

GUI 检查发生在打包之前；[打包器](https://github.com/LIghtJUNction/ztty/blob/edae40b03be0207fd062e13bc4304c42bf0d9fc0/tools/package_macos.py)
复制该 Mach-O，再执行 ad-hoc codesign。
签名后 executable 的摘要变化有完整来源链：两种压缩包的五个文件逐一匹配 manifest，
包内 BUILD.json 的 `source_binary_sha256` 精确等于本次原生验收二进制摘要。

| 架构 | GUI 验收的源二进制 SHA-256 | 签名后包内二进制 SHA-256 |
| --- | --- | --- |
| macOS x86_64 | `b8ec52a9b8e61552a74a5302fa9efbfbee7af5e42dbbc427bbc46ec0c9cb6190` | `a284c47329ee0bea8bcfecd3d1fb4924a226f99aaa53e82fe6b46892398c6442` |
| macOS aarch64 | `1ac4c93ba32042a245d06ac4bae9749eecf6e5e499b586042f5e48c62983482b` | `d57c18b2b56204e89d1514e986bf95650a5c8433214ebf50694fe8006fbf0348` |

manifest 的签名状态为 `ad-hoc-verified`，`notarized=false`。
签名后包通过 Mach-O、bundle metadata、文件摘要、签名及归档往返核验；
**没有重新运行签名后 .app 的 GUI**。本记录保持这一区别，不把二者写成逐字相同的被测文件。

## 固定二进制与归档

| 被测二进制 | SHA-256 | 与打包结果的关系 |
| --- | --- | --- |
| ztty-linux-aarch64 | `0681944cc2d8360a8a1c4a3619e2348b9aa5f443a835f20f70aeeb7cd3663ad4` | 与包内可执行文件相同 |
| ztty-linux-x86_64 | `d1275e8ffbceb5ca3e4d3d501ffa33e2564a8a1a75609ed56ce7ac3270018cb5` | 与包内可执行文件相同 |
| ztty-macos-aarch64 | `1ac4c93ba32042a245d06ac4bae9749eecf6e5e499b586042f5e48c62983482b` | 签名后包另见上表 |
| ztty-macos-x86_64 | `b8ec52a9b8e61552a74a5302fa9efbfbee7af5e42dbbc427bbc46ec0c9cb6190` | 签名后包另见上表 |
| ztty-windows-aarch64 | `de4f4b7e60d2b5c182d4853c53edaae3cf115c21b7f855d3308c3f0bb38a6382` | 与包内可执行文件相同 |
| ztty-windows-x86_64 | `1d00e2aa077b5dc6129fd4b78a1efd45bf5d154fc13ffaa3c3af59a0b68c25a3` | 与包内可执行文件相同 |

六份 GitHub artifact ZIP 的下载大小及 SHA-256 均与 GitHub 提供的 digest 一致：

| Artifact | ID | ZIP B | ZIP SHA-256 |
| --- | ---: | ---: | --- |
| ztty-linux-aarch64 | 14592963478 | 1,806,741 | `4bb121ff625b9d99c64f79b8a5f644d81231af5f6541d10738256897b234c6cc` |
| ztty-linux-x86_64 | 14593402344 | 1,840,836 | `ea4fb60cd7303c39261508a4ec51949f2bffada465ba5d35257c9e486464b3b9` |
| ztty-macos-aarch64 | 14592528695 | 600,312 | `d5a04a605c538638a49c818eba6fab98bb097921898e7bf8f45bb7e0c5dca545` |
| ztty-macos-x86_64 | 14592528808 | 652,734 | `bcd085f23e34efad72a595a40e1a08f9309f58e38370d8fc045c3a15ddc9cf8e` |
| ztty-windows-aarch64 | 14592789765 | 1,165,109 | `e4f3bdf8c2a95c26aa66ee8781b1fb63e601f0e8a983f085bdd969fe3fe3d6cb` |
| ztty-windows-x86_64 | 14593397287 | 1,238,909 | `5f98d6153c9e8faa02d152c0cae36b54098eaa752b0f07c330b7bc908916c443` |

Linux native evidence tar 每架构 146 个成员，原始 JSON、日志与截图保留在对应 Actions ZIP 内。
Windows 的完整冷启动截图、macOS 的各阶段原件同样保留。
七份 job log 已通过 GitHub 插件读取；归档副本与插件解码内容的 UTF-8 SHA-256 逐字一致。

## 离线复核与当前发布边界

```sh
python3 docs/reports/native-edae40b-20261120/verify-record.py
```

该命令只核对公开证据摘要、固定 source、实际检查计数、像素/清理状态和签名来源链。
完整 artifact 的稳定 GitHub ID、digest、成员清单及本地归档回执都可追溯；
本次文档整理没有重跑任何产品测试、构建、GUI、benchmark 或 workflow。

当前可确认的是 **edae40b / zterm 0.5.0 组合的六架构原生候选验收通过**。
后续核心版本、应用源码或构建内容变化需要绑定自己的候选与验收；本记录不自动转移结论。
应用最终发布、安装、物理 GPU/桌面 IME，以及最终性能回退处理仍由后续报告给出各自证据。
