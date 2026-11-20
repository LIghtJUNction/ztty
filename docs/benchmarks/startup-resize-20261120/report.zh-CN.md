# ztty 启动百分号与窗口重排修复

日期：2026-11-20（Asia/Taipei）

## 结果与复现范围

本轮修复独立 X11 窗口处理排队输出和尺寸变更的顺序。回归使用私有 Xvfb、软件渲染、DejaVu Sans Mono 13 号字体及自有 PTY/broker：子进程先按 120 列写出 zsh 风格的行尾标记，窗口再缩到 110 列。基线在 padding=0/28 下均留下独立 `%`，候选均正确擦除。六项回归由 **4 项通过、2 项失败** 变为 **6 项全部通过**，两次运行均无残留自有子进程。[基线记录](before.json)、[候选记录](after.json)。

基线：`9c0d8c3a4418dc684428daefd78ec77ae3de121e`。候选：`cbea202aac882b774b3d8ffe6afce784a43a1224`。候选二进制 SHA-256：`3b410373c0c162686caec46e0b3ad95834b389181b2b3be449cbf0f48758115d`。源码包版本仍为 **0.5.2**，本次没有创建新正式发行版；桌面安装不属于这份源码验证记录。

## `%` 表示什么

zsh 默认以 `PROMPT_SP` 保护未换行的输出，用反显、加粗的 `%` 标记普通用户的未完成行；`PROMPT_EOL_MARK` 可定义这个符号。因此，真实未换行输出后的 `%` 属于正常行为，空白行残留则需检查终端处理顺序。[官方选项说明](https://zsh.sourceforge.io/Doc/Release/Options.html#Prompting)、[参数说明](https://zsh.sourceforge.io/Doc/Release/Parameters.html)。

## 原因与改动

初始 padding 计算是正确的：窗口像素宽度为“请求列数 × 单元格宽度 + 两侧 padding”，PTY 和终端模型也使用同一请求列数。风险出现在映射之后：子进程可以先按旧列数写完提示符，而主循环先处理窗口管理器发来的 `ConfigureNotify`、改变网格，再读取先前排队的输出。按 120 列生成的空格和回车在 110 列网格上执行时会先换行，留下第一行的 `%`。[窗口初始化](https://github.com/LIghtJUNction/ztty/blob/cbea202aac882b774b3d8ffe6afce784a43a1224/src/x11/window.rs)、[会话创建](https://github.com/LIghtJUNction/ztty/blob/cbea202aac882b774b3d8ffe6afce784a43a1224/src/session/requests.rs)、[事件与输出循环](https://github.com/LIghtJUNction/ztty/blob/cbea202aac882b774b3d8ffe6afce784a43a1224/src/session.rs)。

候选在本地可输入 session 收到未过期、确实改变像素尺寸的配置事件时，先调用已有 `Session::pump`，使用旧网格处理已排队输出，再执行原有 resize。该调用沿用每次最多 4 次读取、每块 16 KiB 的预算，最多处理 64 KiB；同时保留已有滚动位置、回复队列和子进程状态维护。只读视图、非本地拥有的 session、同尺寸和过期事件不会触发这次提前读取。[修复源码](https://github.com/LIghtJUNction/ztty/blob/cbea202aac882b774b3d8ffe6afce784a43a1224/src/session.rs)。

## 从失败到通过的证据

回归使用可观察的顺序屏障：确认自有 broker 身份后暂停它；等待子进程确认完整旧宽度输出已进入 PTY；向自有窗口请求缩窄并等待 XSync；最后恢复 broker。测试不靠任意延时制造排队顺序。每组还检查相同宽度的空行擦除，以及真实未换行内容的正常标记。[回归脚本](https://github.com/LIghtJUNction/ztty/blob/cbea202aac882b774b3d8ffe6afce784a43a1224/tests/resize_prompt_gui.py)。

| Padding | 场景 | 基线 | 候选 | 第一行：基线 → 候选 |
| --- | --- | --- | --- | --- |
| 0 px | 相同列数的空行标记 | 通过 | 通过 | `""` → `""` |
| 0 px | 确有未换行内容 | 通过 | 通过 | `"x%"` → `"x%"` |
| 0 px | 旧宽输出排队后窗口缩窄 | 失败（预期重现） | 通过 | `"%"` → `""` |
| 28 px | 相同列数的空行标记 | 通过 | 通过 | `""` → `""` |
| 28 px | 确有未换行内容 | 通过 | 通过 | `"x%"` → `"x%"` |
| 28 px | 旧宽输出排队后窗口缩窄 | 失败（预期重现） | 通过 | `"%"` → `""` |

![修复前：受控窗口重排后留下百分号](before/padding-28-resize-queued-marker.png)

![修复后：同一受控场景清除了百分号](after/padding-28-resize-queued-marker.png)

全部受控截图见 [before](before/) 与 [after](after/)。另完成八项套件标记检查，并在 padding=0/28 下各启动一次真实 oh-my-ztty shell：两次均无独立 `%`，PTY 与模型尺寸一致。这是正常启动兼容性检查；修复的因果证据来自受控回归。涉及个人配置的套件原始 JSON 和截图仅留本地，公开 [验证摘要](validation.json)。

## 构建与正确性验证

| 验证 | 结果与证据 |
| --- | --- |
| 工作区测试，启用 wayland feature | 193 项通过；[日志](workspace-tests.log) |
| SessionCore | 10 项检查通过，测试子进程全部回收；包含于工作区日志 |
| 确定性 GUI 回归 | 六项全部通过；[候选日志](after.log)、[基线预期失败日志](before.log) |
| Clippy | workspace、all-targets、`-D warnings` 通过；[日志](clippy.log) |
| release 与 no_std 边界 | 构建和检查通过；[构建](release-build.log)、[检查](no-std.log) |
| 格式和补丁 | `cargo fmt --check` 与 `git diff --check` 通过 |

基线 GUI 回归返回 1 是预先规定的失败复现；候选验证命令均返回 0。JSON 中保留了这些退出码和耗时。公开日志和 JSON 仅替换本机绝对路径，测量数值、布尔结果及失败记录均未改动。

## 复现方法与边界

在基线与候选提交、各自固定的子模块和锁文件下构建 release 二进制，并保留为两个不同文件。两者使用候选提交中的同一份回归脚本和测试辅助代码：

```sh
CARGO_BUILD_JOBS=2 cargo build --locked --offline --release \
  --features wayland --bin ztty

python3 tests/resize_prompt_gui.py --binary artifacts/baseline-ztty \
  --output artifacts/startup-resize/before.json \
  --screenshots artifacts/startup-resize/before

python3 tests/resize_prompt_gui.py --binary target/release/ztty \
  --output artifacts/startup-resize/after.json \
  --screenshots artifacts/startup-resize/after
```

运行需要已缓存的构建依赖、原生字体、Xvfb、Python Xlib 与 Pillow。基线命令预期返回 1，候选预期返回 0；脚本管理其私有显示服务、专属 socket 和自有进程。

结论限于独立 X11 窗口的这类短输出竞争。workspace 父窗口布局、Wayland 行为、超过读取预算的积压和子进程持续并发写入没有在本轮证明全部解决。此次受控竞争能产生与用户描述相同的残留，但没有重建用户当时完整的事件时间线。

## 证据与范围自检

| 检查问题 | 回答与依据 |
| --- | --- |
| 改动解决了什么？ | 旧宽度排队输出被新宽度解析的问题；两组基线失败与相同候选场景通过对应。 |
| 表述是否混淆正常标记与错误残留？ | 保留真实未换行内容对照，并给出 zsh 官方语义。 |
| 实验能否重复控制顺序？ | 使用进程状态、写入确认和 XSync 屏障；保留原始失败与退出码。 |
| 评估覆盖是否说清？ | 覆盖两种 padding、六项固定回归、额外套件检查与构建；其他布局/平台另行验证。 |
| 方法是否有界？ | 复用已有 4 × 16 KiB 读取预算，不宣称 resize 与所有并发输出原子同步。 |
