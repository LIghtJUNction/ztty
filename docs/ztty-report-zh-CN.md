# ztty 开发与发布记录

日期：2026-11-18，Asia/Taipei。

**已完成可运行的 Linux 终端初版，拆出三个可复用的 Rust 库；其中两个已发布到 crates.io，第三个已通过打包验证，正式发布因开发连接中断而未完成。**

项目仓库：[LIghtJUNction/ztty](https://github.com/LIghtJUNction/ztty)。
生产代码提交：[9333047](https://github.com/LIghtJUNction/ztty/commit/9333047c0867756a73d71e29524ddef3d9f21544)。

## 组件与发布状态

| 组件 | 职责 | 状态 |
| --- | --- | --- |
| `zvte 0.1.3` | 把 ANSI/VT 字节流解析成终端操作 | 已采用用户现有的库，关闭默认特性 |
| [`zinput 0.1.0`](https://crates.io/crates/zinput/0.1.0) | 编码键盘、鼠标、焦点、粘贴事件 | 已由 LIghtJUNction 发布，独立使用验证通过 |
| [`zpty 0.1.0`](https://crates.io/crates/zpty/0.1.0) | 创建和管理 Linux 伪终端、非阻塞 I/O、子进程生命周期 | 已由 LIghtJUNction 发布，真实 PTY 验证通过 |
| `zterm 0.1.0` | 屏幕、历史、模式、协议回复和变化记录 | 源码与测试已提交；发布 dry-run 通过；crates.io 尚无该版本 |

三个新库都有各自的 Cargo 清单、MIT 许可证、README 和公共 API。
`zinput` 与 `zpty` 没有 Cargo 依赖。`zterm` 复用 `zinput`、`zvte`
和关闭默认特性的 `unicode-width`。它们可以脱离图形前端使用。

```sh
cargo add zvte@0.1.3 --no-default-features
cargo add zinput@0.1.0 --no-default-features
cargo add zpty@0.1.0 --no-default-features
```

公开包经过三组独立验证：只依赖 `zinput`、只依赖 `zpty`、同时依赖两者。
验证项目使用精确 crates.io 版本，没有本地路径依赖；库代码使用 `no_std`，
检查与运行全部通过。包内 VCS 记录均指向上述生产代码提交，校验和与
crates.io 一致。[发布记录与原始验证结果](https://github.com/LIghtJUNction/ztty/blob/main/docs/releasing.md)。

`zterm` 的 `cargo publish --dry-run --locked -p zterm` 已以退出码 0
完成，依赖从注册表解析并独立编译成功。dry-run 不会上传；正式发布不能
据此宣称完成。已有开发机发布任务的最终结果尚未取回，重试前应先核对
该任务和注册表状态，避免重复操作。

## 终端如何工作

终端把键盘操作编码后送给运行中的程序，接收程序输出的文字与控制指令，
更新屏幕状态，再画出发生变化的区域。代码按数据所有权拆分：`zpty`
管理进程与文件描述符，`zterm` 管理屏幕，`zinput` 只负责事件编码。

屏幕格子在连续数组中存储，每格 28 字节，没有单独的堆指针。整屏滚动
通过改变行索引实现，避免搬动整块屏幕。历史按需增长，字符存储上限为
8 MiB；达到上限后复用旧行。备用屏幕按需创建。

前端使用单线程事件循环，空闲时在 `poll` 中等待；绘制仅处理发生变化的行。
PTY 写队列保存已写入的位置，遇到部分写入或背压时继续等待，不丢弃剩余数据。
同步输出保留上一帧，并在缺少结束标记时用 150 ms 超时恢复。

生产 Rust 代码及依赖不使用 Rust `std`，动态存储使用 `alloc`，由应用提供
分配器。Linux 窗口和字体通过系统 C ABI 使用 libc/libutil、X11、Xft 及
编译器 unwinder。因此，`no_std` 不等于没有操作系统库，也不能直接代表
整个窗口只消耗核心模型的内存。测试和基准程序可以使用 `std`，不进入
生产程序的 Rust 依赖链。

## 已实现的功能

- 启动真实 shell；主备屏、光标、滚动区域、插入/删除/擦除和历史浏览。
- 流式 UTF-8、中日韩宽字符、每格两个组合字符、16/256 色和真彩色。
- Kitty 键盘增强标志 1/2/4/8/16，按下/重复/释放、修饰键、关联文本；主备屏模式栈分离。
- SGR/传统鼠标、焦点事件、括号粘贴、同步输出。
- OSC 标题、工作目录元数据、OSC 8 链接、颜色查询、DSR、DA、DECRQM 和有限 XTGETTCAP。
- X11/Xft 窗口、字体回退、选择、剪贴板/主选择、INCR 接收和 XIM 接口。

Ctrl+Shift+C/V 复制粘贴，Shift+PageUp/PageDown 浏览历史，Ctrl+点击打开
OSC 8 链接。应用捕获鼠标时，按住 Shift 使用本地选择、滚轮和中键粘贴。
链接只在用户明确点击后打开，且通过 argv 传递，不拼接 shell 命令。

## 验证结果

| 检查 | 结果 |
| --- | --- |
| 库单元测试 | 40 项通过 |
| README 示例测试 | 3 项通过 |
| Clippy，警告视为错误 | 通过 |
| Linux release 构建 | 通过 |
| 生产依赖及二进制 `no_std` 审计 | 通过，未发现 Rust `std` 符号 |
| PTY 真实会话 | 读写、尺寸、控制终端、退出、子进程归属验证通过 |
| Xvfb/X11 GUI 验收 | 11 项通过，0 项失败 |
| 已发布包的独立消费者 | 单包与组合使用全部通过 |
| `zterm` 发布前打包验证 | 通过，未实际上传 |
| 裸机目标额外检查 | 目标库不可用，未通过该项验证 |

GUI 验收包括键盘、Kitty 增强与重复事件、窗口尺寸、中文选择、剪贴板、
粘贴控制字符清理、背压下的大量粘贴、鼠标、手势归属、链接打开和同步帧。
大粘贴实际送达 139,276 字节，其中 12 字节为括号粘贴分隔符。

真实 IBus/Fcitx 会话、专门构造的 INCR 分块传输和实际 XWayland 桌面
尚未完成验收。接口已实现与这些环境已实测，是两个不同结论。

## 性能数据及边界

以下是最终生产源码在另一台执行环境的记录：Intel Xeon Platinum 8573C、
9 个可见逻辑 CPU、Linux 6.18.57、Rust 1.100.0，release 构建，120×32 屏幕。
它不是 i9-12900HX 开发机的最终整机结果，不用于跨机器计算优化收益。

| 核心指标 | 实测 |
| --- | ---: |
| 初始核心堆内存 | 107,676 B，约 105.15 KiB |
| ASCII 吞吐 | 202.116 MiB/s |
| 满历史缓冲 ASCII 吞吐 | 126.492 MiB/s |
| SGR 工作负载 | 128.566 MiB/s |
| 混合 Unicode 工作负载 | 76.143 MiB/s |
| 1 KiB 文本更新 p50 / p95 / p99 | 3.019 / 7.177 / 14.441 µs |
| 已测稳态循环的堆分配次数 | 0 |

延迟样本每次合并计时 16 次 feed，再除以 16，共 10,000 个样本；因此是
批次均摊的核心处理时间。吞吐取 7 轮的中位数。测量包含解析和屏幕状态
更新，不包含 PTY、字体渲染、合成器或显示器扫描输出。

核心初始内存不含代码、栈、分配器元数据、字体、共享库和 X server 图形
缓冲。整窗 RSS/PSS、PTY 往返和 Kitty 对照脚本已写好，但最终运行结果
未取回，因此不填入数字，也不声称已经全面超过 Kitty/Ghostty。Ghostty
在所检查的开发环境中未安装。[完整方法与原始数据](https://github.com/LIghtJUNction/ztty/blob/main/docs/performance.md)。

## 运行与后续验收

开发机项目目录：`/home/lightjunction/Documents/GITHUB/ztty`。

```sh
cargo build --locked --release
./target/release/ztty
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
```

依赖 Linux、Rust 1.100.0、C 链接器、X11/Xft 开发库和等宽字体。
`make install` 可安装到当前用户的 `~/.local`。

当前交付采用 X11/XWayland。原生 Wayland、专用 GPU 渲染、Sixel/Kitty
图像协议、OSC 52、复杂文字塑形、完整 emoji ZWJ 和历史重排尚未实现。
后续应先完成 `zterm` 正式发布与整窗测量，再按相同硬件、字体、窗口、
内容和渲染条件，与 Kitty/Ghostty 比较中位数、长尾延迟和总内存。

实际界面截图保存在仓库
[`docs/assets/ztty-preview.png`](https://github.com/LIghtJUNction/ztty/blob/main/docs/assets/ztty-preview.png)。
