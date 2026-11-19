# ztty 小数字键盘修复验收

日期：2026-11-19

## 修复结果

小数字键盘的 Num Lock、Shift、应用键盘模式和本地界面操作已完成修复。源码提交为 [eabc9b9](https://github.com/LIghtJUNction/ztty/commit/eabc9b9154cb652720ed26e03e89e074561c3c80) 与 [2fd82de](https://github.com/LIghtJUNction/ztty/commit/2fd82de528e0a09521c237e3feb913fd1964425d)，统一审阅入口为 [PR #17](https://github.com/LIghtJUNction/ztty/pull/17)。

| 用户操作 | 修复后的行为 | 验证依据 |
|---|---|---|
| Num Lock 开启，程序启用应用键盘模式 | 数字、小数点和运算符作为文本输入，小键盘 Enter 发送回车 | 逐键比较真实 PTY 收到的字节 |
| Num Lock 与 Shift 组合 | 按 XKB 当前布局选择数字或导航含义 | 私有 `numpad:pc` 布局下的四种状态，覆盖 0–9 和物理小数点/Delete 键 |
| 按住小键盘键，再松开 Shift | 自动重复及最终释放沿用最初按下的键身份 | 实际 X server 自动重复与 Kitty 事件字节 |
| 搜索中的小键盘 Enter、Shift+Enter | 与主键盘对应操作选择相同匹配项 | 实际窗口像素对照，正反方向结果不同 |
| 命令面板中的小键盘 Enter、上/下 | 能选择并执行放大、缩小或重置字号 | 实际字形尺寸、窗口网格和 PTY 尺寸变化 |
| 搜索或面板中的无文本按键 | 保持当前选中项 | 单独 Shift 按下/释放及 PTY 无泄漏检查 |

终端收到程序的 `ESC =` 后会进入“应用键盘模式”，即把小键盘输入编码成可供程序识别的功能序列。Num Lock 应允许用户恢复数字输入。旧版在 Num Lock 开启时按小键盘 `0`，收到的字节是 `1b 4f 70`（`ESC O p`）；修复后收到 `30`（字符 `0`）。应用显式协商的 Kitty 功能键身份、事件类型和关联文本仍被保留。

## 验证结果

| 检查 | 结果 |
|---|---:|
| zinput 单元测试 | 23 通过 |
| zinput 文档测试 | 2 通过 |
| zinput Clippy，警告视为错误 | 通过 |
| Linux release 二进制编译 | 通过 |
| X11/XTEST → 真实 PTY 输入向量 | 177 通过 |
| 字号及网格场景 | 14 通过 |
| 小键盘搜索／命令面板场景 | 4 通过 |
| 测试结束后仍存活的自有进程 | 0 |

完整预期值与实际字节见 [JSON 验收记录](measurements/keypad-routing-2026-11-19.json)。[窗口截图](assets/keypad-routing-2026-11-19.png) 已检查，文字清晰，覆盖层退出后没有残留。

测试使用独立 Xvfb、私有 broker、原始 PTY 和真实 XTEST 按键。物理小数点键根据 `KP_Delete` 所在键位定位，避免测试误用键盘映射中的另一个 `KP_Decimal` 别名。输入重置同时恢复应用键盘和光标模式，防止用例之间继承模式。

本轮验证范围为 Linux X11/XWayland 前端在私有 Xvfb 中的事件路径，以及平台无关编码器。实际硬件、输入法组合、原生 Wayland、Windows 和 macOS 仍应分别验证；本记录不将这些路径计入通过数量。

## 复现

需要 Rust 工具链、Xvfb、setxkbmap、Python Xlib 和 Pillow。在仓库根目录运行：

```sh
cargo test --locked -p zinput
cargo clippy --locked -p zinput --all-targets -- -D warnings
cargo build --locked --release --bin ztty
python3 tests/input_gui.py \
  --binary target/release/ztty \
  --source-root . \
  --output artifacts/input-gui.json \
  --screenshot artifacts/input-gui.png
```

## 开发机生效检查

更新安装文件后，已有 broker 及其窗口仍可能运行旧二进制。本次现场检查确认过 0.3.1 旧进程与 0.4.1 安装文件同时存在。保留这些会话，检查新窗口所属 broker 实际运行的文件，才能确认修复已作用于该窗口。

开发机同时进行滚动、搜索和图像修复；安装应使用包含这些改进与本次小键盘提交的统一版本。此记录证明上面的源码和测试结果，实际安装状态以开发机最终读回为准。

## 证据自检

| 检查维度 | 结论 |
|---|---|
| 问题与修复对应 | 已复现错误字节，修复后对应向量通过 |
| 表述清楚 | 区分安装文件、运行进程和源码验证 |
| 实验强度 | 包含模式组合、逐键输入、自动重复及真实界面结果 |
| 覆盖完整性 | 明确记录 Linux 路径及尚未执行的平台验证 |
| 设计兼容性 | Num Lock 仅覆盖应用键盘模式；全部 32 种 Kitty 标志组合维持已协商行为 |

协议依据：[XTerm Control Sequences](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)、[Kitty keyboard protocol](https://sw.kovidgoyal.net/kitty/keyboard-protocol/)。
