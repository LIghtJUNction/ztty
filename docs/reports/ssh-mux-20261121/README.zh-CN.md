# ztty 0.6.0：SSH 持久会话与终端多路复用

本轮增加 Linux 上的 `ztty ssh` 和 `ztty mux`。远端 broker 持有 PTY、子进程、屏幕和历史；SSH 只传输输入与视图。因此关闭本地窗口或断开 SSH 后，远端 shell 仍然运行，再次连接同名会话可以恢复原进程。完整命令见 [SSH 使用指南](../../ssh-sessions.md)。

本地验收基于主分支 `094085e90eda4df1e696bbc16283527445ee5a8d` 加本轮变更，已保留该主分支更新的依赖锁定值。各源码文件的 SHA-256、命令和退出状态见 [gates.json](gates.json)。验收二进制启用 `wayland`，使用 Rust 1.100.0 的 release 配置，SHA-256 为：

```text
8dc097df858240827d3d8deb2cabc04274d150e686a27c9387d398c4af958aa4
```

## 行为与边界

同一个 SSH 连接可以承载多个独立 PTY。原生新标签页、左右/上下分屏和新窗口继承连接配置，创建新的远端 shell；创建命令只在指定会话第一次出现时执行。只读观察者不发送输入，也不改变写入者的 PTY 尺寸。输入控制权采用独占租约，接管需要明确指定 `--takeover`。

本轮对应 Rex 一类产品的持久 SSH 会话能力，使用 ztty 自身的会话协议。两端均需 Linux 和 ztty 0.6.0+；不提供 Rex Server 协议兼容。原生标签和分屏使用 X11/XWayland，独立窗口支持原生 Wayland。SSH 视图传输文字、颜色、字形组合、链接、光标、输入模式和有界历史；不传输 Kitty/Sixel 图片、shell 命令标记或软换行元数据。

SSH 认证、主机密钥检查和配置交给系统 OpenSSH。连接复用使用私有目录内的 ControlMaster socket。PTY 不依赖 SSH 进程的生命周期，但主机重启或终止 broker 会结束这些进程。

## 性能改变及测量范围

空闲时，broker 挂起 revision watcher，不复制输出环形缓冲区，也不遍历整个屏幕。有变化时最多每 16 ms 生成一帧，并只把变化的行写入 SSH。输入和输出均有界：客户端等待当前输入块被 PTY 接受，输出只保留一个待发送帧。

### 可复现的字节数比较

[ssh_frame_benchmark](../../../examples/ssh_frame_benchmark.rs) 建立 120×40 文字屏幕，连续改变一个状态行 1,000 次。它比较每次重绘完整视图与保留前帧后发送变化行的 ANSI 字节数，结果见 [frame-benchmark.json](frame-benchmark.json)。

| 指标 | 结果 |
| --- | ---: |
| 首次完整视图 | 6,539 字节 |
| 相同画面再次渲染 | 0 字节 |
| 1,000 次完整重绘 | 6,569,000 字节 |
| 1,000 次变化行更新 | 343,000 字节 |
| 该场景减少的 ANSI 字节 | 94.779% |

这个比较针对相同内容的两种发送方式，不是与 Rex、tmux 或旧版 ztty 的整体性能比较，也不包含 SSH 加密、压缩或 TCP 开销。全屏连续变化时不能期待相同节省比例。

### 实际 SSH 路径

[ssh-mux.json](ssh-mux.json) 使用临时密钥、固定主机密钥和仅监听 localhost 的真实 sshd。以下均为一次验收样本，不能外推为公网延迟或长期 CPU 占用：

| 指标 | 结果 |
| --- | ---: |
| 第一个远端会话连接 | 197.753 ms |
| 复用连接创建第二个会话 | 43.290 ms |
| 两个会话的 SSH 公钥认证次数 | 1 |
| 断开传输后重连原进程 | 152.977 ms |
| 该会话重连视图输出 | 3,080 字节 |
| SSH 测试中 350 ms 空闲窗口的进程 CPU tick 增量 | 0 |
| 该样本的新输出唤醒 watcher | 0.789 ms |
| 该样本的客户端可见输出 | 11.053 ms |

## 输入、退出与恢复

输入确认必须表示字节已经被 PTY 接受。仅确认“进入内存队列”会使大段粘贴后立即脱离时的剩余输入被释放操作清掉。本轮为 mux 写入增加最长 5 秒的异步排空确认，并将等待绑定到会话身份、actor 和控制权 epoch。超时或控制权变化明确失败，普通 MCP 写入保留原有语义。

背压检查让远端程序进入 raw 模式后暂停读取 500 ms，再连续发送 **524,425 字节并立即 Ctrl+]**。本地与真实 SSH 两条路径都逐字节保存了完整 payload；客户端正常脱离，原 PTY 和进程仍保留。另检查了 NUL、非 UTF-8 字节、跨读取边界的 UTF-8，以及 Kitty 键盘协议中的脱离组合键。

短命令检查先关闭标准输入输出，再延迟退出，确认客户端会等待真实退出状态。初次连接与重连均恢复最终画面并返回 **42**，且创建命令只执行一次。

## 验收结果

| 范围 | 结果与证据 |
| --- | --- |
| Rust workspace | 265 项通过；8 项需专用原生环境的测试按其声明忽略；[gates.json](gates.json) |
| Clippy、release 构建、no_std 检查 | 全部通过；[gates.json](gates.json) |
| 真实 SSH 与本地 mux | 18 项通过；[ssh-mux.json](ssh-mux.json) |
| 软件渲染原生 SSH 界面 | 6 项通过；[ssh-gui-software.json](ssh-gui-software.json) |
| GPU 渲染原生 SSH 界面 | 6 项通过；[ssh-gui-gpu.json](ssh-gui-gpu.json) |
| MCP 会话和控制权 | 通过；[mcp-sessions.json](mcp-sessions.json) |
| MCP 延期读取、并发和取消 | 13 项通过；[mcp-wait.json](mcp-wait.json) |
| 热重载 | 同 PID、PTY、会话、布局和已挂起读取保留；[reload.json](reload.json) |

GUI 测试使用独立 Xvfb，通过真实 XTEST 输入验证远端 shell，创建标签、分屏和窗口，再关闭全部视图并重连原进程。截图就绪同时检查远端和本地解析状态、执行后的标题、完整文字，以及两个原生窗格内指定行的彩色像素；避免将命令回显误认为执行结果。软件和 GPU 截图均经过人工视觉检查，双栏中文和彩色文字完整。GPU 测试使用 llvmpipe，不构成物理 GPU 性能测量。

所有 SSH/GUI 夹具的进程清理记录均为空错误，未留下其拥有的进程。图片和可执行文件不放入 Git；JSON 保留截图及二进制哈希。正式发布仍以标签对应的完整跨平台 Release 工作流为准，安装另行核对实际文件和版本。
