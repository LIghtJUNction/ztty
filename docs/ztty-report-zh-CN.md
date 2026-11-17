# ztty 开发与发布记录

日期：2026-11-18，Asia/Taipei。

**ztty 的 Linux 终端初版、三个可复用库的正式发布，以及本轮公开包和软件 X11 测量均已完成。** `zinput`、`zpty`、`zterm` 现在都能从 crates.io 安装，并已验证在独立项目中单独和组合使用。

项目仓库：[LIghtJUNction/ztty](https://github.com/LIghtJUNction/ztty)。生产 Rust 实现来自 [9333047](https://github.com/LIghtJUNction/ztty/commit/9333047c0867756a73d71e29524ddef3d9f21544)；后续提交补充测试、发布记录和测量工具，没有改变这版生产代码。

## 组件与发布

| 组件 | 职责 | 状态 |
| --- | --- | --- |
| `zvte 0.1.3` | 把 ANSI/VT 字节流解析成终端操作 | 复用用户现有的库，关闭默认特性 |
| [`zinput 0.1.0`](https://crates.io/crates/zinput/0.1.0) | 键盘、鼠标、焦点、粘贴事件编码 | 已发布，独立使用验证通过 |
| [`zpty 0.1.0`](https://crates.io/crates/zpty/0.1.0) | Linux PTY、非阻塞 I/O、子进程生命周期 | 已发布，真实 PTY 验证通过 |
| [`zterm 0.1.0`](https://crates.io/crates/zterm/0.1.0) | 屏幕、历史、模式、协议回复和变化记录 | 已发布，单包及组合验证通过 |

三个新库都有独立 Cargo 包、MIT 许可证、README 和公共 API。`zinput` 与 `zpty` 没有 Cargo 依赖；`zterm` 复用 `zinput`、`zvte` 和关闭默认特性的 `unicode-width`。

```sh
cargo add zvte@0.1.3 --no-default-features
cargo add zinput@0.1.0 --no-default-features
cargo add zpty@0.1.0 --no-default-features
cargo add zterm@0.1.0 --no-default-features
```

注册表回读确认三个包均由 LIghtJUNction 发布，版本为 0.1.0，未被撤回。`zterm` 于 2026-11-18 01:45:40（Asia/Taipei）发布，Cargo 已确认可从注册表取得。

独立验证项目使用精确 crates.io 版本，没有本地路径依赖。三个单包消费者和一个组合消费者均完成元数据检查、`no_std` 库检查及实际运行，覆盖屏幕状态、协议回复、键盘编码和真实 PTY。`zinput`、`zpty` 的包内源码提交为 `9333047`，`zterm` 为 `3e2f032`；两次提交之间的生产 Rust 源码一致。校验和与源提交均保存在[发布记录](https://github.com/LIghtJUNction/ztty/blob/main/docs/releasing.md)。

## 实现方式

终端接收键盘操作，把它编码后送给程序；接收程序输出的文字和控制指令，更新屏幕，再画出变化。代码按数据所有权拆分：`zpty` 管进程与文件描述符，`zterm` 管屏幕，`zinput` 负责事件编码。

字符格使用连续数组，每格 28 字节，没有单独的堆指针。整屏滚动改变行索引，避免搬动整块屏幕；历史按需增长，字符存储上限为 8 MiB，达到上限后复用旧行。备用屏幕按需分配。

前端使用单线程事件循环，空闲时在 `poll` 中等待，绘制只处理变化的行。PTY 写队列保留已写位置，遇到部分写入或背压时等待继续。同步输出保留上一帧，并用 150 ms 超时处理缺少结束标记的情况。

生产 Rust 代码和依赖使用 `no_std`；动态存储使用 `alloc`，分配器由应用提供。Linux 窗口和字体通过系统 C ABI 使用 libc/libutil、X11、Xft 及编译器 unwinder。测试和基准程序使用的 `std` 不进入生产程序的 Rust 依赖链。系统字体、共享库、图形缓冲仍然占内存，因此核心模型的大小不能代替整个窗口的内存。

## 已实现的功能

- 真实 shell、主备屏、光标、滚动区域、插入/删除/擦除、历史浏览。
- 流式 UTF-8、中日韩宽字符、每格两个组合字符、16/256 色和真彩色。
- Kitty 键盘标志 1/2/4/8/16，按下/重复/释放、修饰键和关联文本，主备屏模式栈分离。
- SGR/传统鼠标、焦点事件、括号粘贴、同步输出。
- OSC 标题、工作目录元数据、OSC 8 链接、颜色查询、DSR、DA、DECRQM 和有限 XTGETTCAP。
- X11/Xft 窗口、字体回退、选择、剪贴板/主选择、INCR 接收及 XIM 接口。

Ctrl+Shift+C/V 复制粘贴，Shift+PageUp/PageDown 浏览历史，Ctrl+点击打开链接。应用捕获鼠标时，按住 Shift 使用本地选择、滚轮和中键粘贴。链接只在明确点击后打开，通过 argv 传递。

## 验证结果

| 检查 | 结果 |
| --- | --- |
| 库单元测试 | 40 项通过 |
| README 示例测试 | 3 项通过 |
| Clippy 严格检查、Linux release 构建 | 通过 |
| 生产依赖和二进制 `no_std` 审计 | 通过，未发现 Rust `std` 符号 |
| PTY 真实会话 | 读写、尺寸、控制终端、退出、子进程归属通过 |
| Xvfb/X11 GUI 验收 | 11 项通过，0 项失败 |
| PTY 协议检查 | 12 项通过 |
| 公开包独立消费者 | 三个单包及三包组合全部通过 |
| 本轮软件 X11 测量 | 12 轮写屏/查询、6 轮纯查询全部通过 |

大粘贴验收完整送达 139,276 字节，含 12 字节括号粘贴分隔符。测量结束后没有残留的测试终端或子进程。

真实 IBus/Fcitx、实际 XWayland 会话和专门构造的 INCR 分块传输仍需验收。裸机目标的附加编译检查未执行成功；Linux `no_std` 的源文件、依赖和已链接二进制检查已通过。原始验收记录保存在[性能与验证报告](https://github.com/LIghtJUNction/ztty/blob/main/docs/performance.md)。

## 开发机核心性能

环境：i9-12900HX、24 个逻辑 CPU、Linux 7.3.4-arch1-1、Rust 1.100.0、release 构建，120×32 字符。

| 指标 | 实测 |
| --- | ---: |
| 初始核心堆内存 | 107,676 B，约 105.15 KiB |
| ASCII 吞吐 | 232.927 MiB/s |
| 满历史缓冲 ASCII 吞吐 | 177.049 MiB/s |
| SGR 工作负载 | 125.111 MiB/s |
| 混合 Unicode 吞吐 | 97.722 MiB/s |
| 1 KiB 更新 p50 / p95 / p99 | 3.138 / 4.119 / 4.495 µs |
| 已测稳态处理循环的堆分配 | 0 次 |

延迟样本每次合并计时 16 次 feed 再取均值，共 10,000 个样本；吞吐取 7 轮中位数。这里测量解析和屏幕状态更新，不含 PTY 和绘制。另一台 Xeon 环境的旧记录单独保留，不混合计算或作为优化收益。

## 整窗内存与 PTY 响应

本轮在开发机上使用独立 Xvfb/X11，每组 3 次，轮换测量顺序；设置 `LIBGL_ALWAYS_SOFTWARE=1`。两者请求相同的 120×32 字符网格、monospace 11pt 和固定文本页面，历史未填满。实际窗口为 ztty 1104×728、Kitty 1081×609；字体文件和每轮负载均保留在原始结果中。

下表为每组三次的中位数，单位 MiB：

| 配置 | 进程 RSS | 进程 PSS |
| --- | ---: | ---: |
| ztty | 18.195 | 8.653 |
| Kitty 0.50.0，默认时序 | 155.168 | 124.787 |
| Kitty，仅 `input_delay=0` | 155.340 | 125.315 |
| Kitty，低延迟时序 | 155.504 | 114.102 |

RSS 包含进程驻留页面，PSS 按比例分摊共享页面，两者不能相加。ztty 另有约 3.066 MiB 的 X server pixmap；它与 X server 增量、像素估算等账户也不能重复累加。Kitty 的 XRes pixmap 计数为零不代表其图形内存为零。软件 GL 的内存分布与正常硬件 GPU 环境不同，不能把本表直接推广到真实桌面。

持续写同一字符并查询光标位置时，ztty 往返 p50 为 **16.705 µs**，p95 为 **29.913 µs**，p99 为 **79.184 µs**；Kitty 三种时序配置的 p50 约 6.49–6.97 ms。每轮预热 100 次后测 1,000 次，报告取三轮各分位数的中位数。

去掉每轮写屏，仅查询光标位置后，结果为：

| 配置 | p50 | p95 | p99 |
| --- | ---: | ---: | ---: |
| ztty | 16.934 µs | 32.905 µs | 89.426 µs |
| Kitty，`input_delay=0` | 47.320 µs | 98.040 µs | 168.922 µs |

Kitty 的结果随写屏负载变化很大，说明该测量对绘制与主循环调度敏感；这不能被简化为纯解析器速度的倍数差。查询回复只确认终端已经理解逻辑状态，不代表新像素已经显示。这里没有测物理键盘到显示器的延迟，也没有测 Ghostty。

低延迟配置使用 `input_delay=0`、`repaint_delay=2`、`sync_to_monitor=no`，依据 [Kitty 官方性能说明](https://sw.kovidgoyal.net/kitty/performance/)，仅对测试进程生效。完整配置、样本口径、每轮结果和脚本校验和均在[性能报告及原始数据](https://github.com/LIghtJUNction/ztty/blob/main/docs/performance.md)中。

## 运行与后续范围

开发机项目目录：`/home/lightjunction/Documents/GITHUB/ztty`。

```sh
cargo build --locked --release
./target/release/ztty
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
```

依赖 Linux、Rust 1.100.0、C 链接器、X11/Xft 开发库和等宽字体。`make install` 可安装到当前用户的 `~/.local`。

当前提供 X11/XWayland 前端。原生 Wayland、专用 GPU 渲染、Sixel/Kitty 图像、OSC 52、复杂文字塑形、完整 emoji ZWJ 和历史重排仍未实现。下一阶段的验收应覆盖真实桌面、输入法、满历史/备用屏内存、持续 TUI 输出，以及与 Ghostty 的同条件比较。

[实际界面截图](https://github.com/LIghtJUNction/ztty/blob/main/docs/assets/ztty-preview.png)。
