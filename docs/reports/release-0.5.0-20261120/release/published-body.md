# ztty 0.5.0

正式标签 `v0.5.0`，源码固定为 [c4f8f48](https://github.com/LIghtJUNction/ztty/commit/c4f8f48c9598284a589e609f25242b49ceabfea8)。

这一版整理了终端宿主与独立库的边界：终端状态、图形协议、PTY、输入和 UI 布局分别维护，长平台文件按职责拆分，首页保留安装、常用操作与证据入口。
源码构建请使用 `git clone --recurse-submodules`；已有 clone 执行 `git submodule update --init --recursive`。

## 主要变化

- Linux 支持 X11 GPU、扩展图形协议、原生 UI scene 与共享会话。MCP、Attach 和多窗口继续使用共享 broker。
- 实验性 Wayland 通过 `--backend wayland` 显式开启，整合共享会话、剪贴板及受测 IME 协议路径。默认仍为 X11；Wayland 的 tab/split/workspace 仍拒绝。
- macOS/Windows 保持本地原生窗口范围；macOS 包采用 ad-hoc 签名，未公证。zgpui 提供跨平台 UI 核心、布局与 renderer 契约，窗口和绘制由宿主实现。
- X11 Auto 在 `LIBGL_ALWAYS_SOFTWARE` **恰好为 `1`** 时提前走已有 software 路径，避免先初始化 GPU 再回退。显式 GPU、变量未设或其他值保持原规则。

依赖采用已发布的 **zgpui 0.1.0、zgraphics 0.2.0、zinput 0.1.3、zpty 0.2.2、zterm 0.5.1、ztty-plugin-api 0.1.0**。
插件 API 精确消费 zplugin 0.3.1；registry 实包、VCS 与锁文件验证见[消费记录](https://github.com/LIghtJUNction/ztty/blob/d4c78a271f3c87aca4c179dc5eb848f3c7f381a0/docs/reports/host-9f5a77a-20261120/README.md)。

## 性能与验证

Unicode 核心处理仍有历史差距：采用的 0.5.1 在同组测量中比 0.5.0 提高 35.03%，但只达到旧版的 28.56%。
未采纳的批处理实验、9f 宿主吞吐/PSS 回归及原始失败都保留，见[核心测量](https://github.com/LIghtJUNction/ztty/blob/8cde51bf1f74af4119e651ea954b15b2e7254a79/docs/benchmarks/resume-2026-11-20.md)。

修复后的同负载宿主测量中，1/10 窗口应用 PSS 为 12.83/21.42 MiB，先前约 100 MiB 的额外软件回退成本未再出现。
10 窗口吞吐仍比本轮旧基线低 21.82%；全部样本、波动区间与 renderer 范围见[9855 检查点](https://github.com/LIghtJUNction/ztty/blob/c37385a7437ad8823ab36e9743aec122043236d9/docs/reports/host-9855da5-20261120/README.md)。
这里的 PTY/DSR 时延衡量逻辑处理完成，不是物理键盘到屏幕的延迟。

六架构原生验收与发行制品见[正式工作流](https://github.com/LIghtJUNction/ztty/actions/runs/44668398577)和[Release 资产](https://github.com/LIghtJUNction/ztty/releases/tag/v0.5.0)。
完整原始验收日志（包括空日志）保存在 [ztty-0.5.0-native-evidence.zip](https://github.com/LIghtJUNction/ztty/releases/download/v0.5.0/ztty-0.5.0-native-evidence.zip)。

<details>
<summary>保留的原始生成说明与验收资产提示</summary>

Raw native evidence, including empty logs, is retained in ztty-0.5.0-native-evidence.zip.

## What's Changed
* Fix numeric keypad modes, XKB layers and local controls by @LIghtJUNction in https://github.com/LIghtJUNction/ztty/pull/17
* Integrate published Project Z libraries and split host responsibilities by @LIghtJUNction in https://github.com/LIghtJUNction/ztty/pull/18


**Full Changelog**: https://github.com/LIghtJUNction/ztty/compare/v0.4.1...v0.5.0

</details>
