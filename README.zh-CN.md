# ztty

<!-- terminal-benchmark:start -->
![十窗口终端内存与 PTY 性能比较](docs/benchmarks/terminals-10-windows.svg)

每个终端均开启 10 个窗口、使用共享实例，实测 3 轮。[测量方法与复跑命令](docs/benchmarks/README.md) · [原始数据](docs/benchmarks/terminals-10-windows.json)

从当前源码重新构建并实测：`make benchmark-terminals`。
<!-- terminal-benchmark:end -->

**2026-11-18 输出吞吐更新：** ztty 0.2.1 在上方十窗口对比中测得 **227.58 MiB/s**。
独立的旧／新配对测试为 19.48 → 200.52 MiB/s，提升 **10.29 倍**，应用 PSS 基本持平。
负载为 Xvfb 软件渲染下的 ASCII／颜色原地更新。
[实测、实现与正确性证据](docs/benchmarks/throughput-2026-11-18.md)。

ztty 是一个用 Rust 开发的原生终端，生产代码不依赖 Rust 标准库。
0.2 版本补齐了 macOS、Windows 后端，并加入静态 Kitty 图形协议。
项目优先验证 Linux，目标是以更低的内存占用和更快的交互响应，
逐步超过 Kitty 与 Ghostty。

Linux 上的多个窗口自动复用一个会话进程、显示连接和字体缓存。
每个窗口分别保存自己的 PTY、屏幕和历史记录。共享会话与 MCP 目前仅支持 Linux。

终端的工作可以拆成三件事：把键盘操作编码后送给程序，理解程序返回的
文字和控制指令，再把变化的内容画出来。图片也遵循这条路径：程序
通过 PTY 发送图片字节，终端解码并记住图片的位置，窗口后端负责合成。
这些逻辑拆成独立的库，平台后端负责操作系统接口。

## 已经可以做什么

启动真实 shell、运行命令行程序，支持主备屏、滚动区域、UTF-8 和中日韩
宽字符、真彩色、鼠标报告、复制粘贴、输入法接口、OSC 8 超链接，以及
Kitty 增强键盘和同步输出协议。图片支持内联 PNG、RGB、RGBA，以及
透明度、裁剪、缩放、滚动、主备屏隔离和删除。界面采用深色背景、
等宽字体和留白。具体平台上的验证范围如下。

| 平台 | 原生实现 | 当前验收状态 |
| --- | --- | --- |
| Linux | X11/XWayland 窗口、Xft 文字、Xrender 图片合成、Linux PTY | Xvfb 下 12 项界面和 7 项图像检查通过；实际 Hyprland/XWayland 中照片、透明 PNG 与自动探测通过 |
| macOS | AppKit 窗口、CoreText/CoreGraphics 渲染、Darwin PTY | Intel 与 Apple Silicon 均通过 8 项原生会话/协议检查；像素读回、输入法和键盘验收待完成 |
| Windows | Win32 窗口、GDI 文字、自行合成 RGBA、ConPTY | x86_64 通过 17 项原生检查，包含真实 ConPTY、窗口像素读回和键盘恢复 |

macOS 后端不要求 X11，Windows 后端不要求 WSL。上述原生检查在
对应操作系统运行；表中明确区分已检查的项目和仍需补齐的验证。
安装包状态见 [发布记录](docs/releasing.md)，对应源码与验收范围见
[协议说明](docs/protocols.md#acceptance-coverage)。

默认显示 120×32 个字符。历史记录按需增长，字符存储最多 8 MiB；
历史达到上限后复用旧行。详细协议范围以 [英文 README](README.md)
和 [协议说明](docs/protocols.md) 为准。

## 可独立使用的库

| 库 | 当前源码版本 | 做什么 | 使用场景 |
| --- | --- | --- | --- |
| [`zvte`](https://github.com/LIghtJUNction/zvte) | 0.1.3 | 解析 ANSI/VT 字节流 | 只需要理解终端控制序列的程序 |
| [`zinput`](crates/zinput) | 0.1.0 | 编码键盘、鼠标、焦点和粘贴事件 | 图形终端、远程终端、终端客户端 |
| [`zterm`](crates/zterm) | 0.2.1 | 维护屏幕、历史、图片锚点、模式和协议回复 | 自定义终端界面、终端录制和回放 |
| [`zpty`](crates/zpty) | 0.2.0 | 管理 Linux/macOS PTY 和 Windows ConPTY | 终端、进程控制、交互式测试工具 |
| [`zgraphics`](crates/zgraphics) | 0.1.0 | 处理 Kitty 图形协议、图片解码、存储和放置 | 需要图片支持的终端或回放工具 |

这些库都有独立 Cargo 包。`zinput` 和 `zpty` 没有 Cargo 依赖；
`zterm` 复用 Project Z 的 `zinput`、`zvte`、`zgraphics` 和 `no_std`
Unicode 宽度表。`zgraphics` 使用第三方 `zune-png`、`zune-core` 和
`zune-inflate` 的 `no_std` 配置解码 PNG 与 DEFLATE。所有直接生产依赖
关闭默认特性，实际启用的依赖图由 [审计脚本](tools/check_no_std.py) 检查。

需要动态内存的地方使用 `alloc`，分配器由宿主提供。图片像素通过
`Arc` 共享，因此 `zgraphics` 要求目标支持指针宽度的原子操作。
不依赖 Rust 标准库仍然需要系统 API：Linux 使用 X11 等系统库，
macOS 使用 Apple 框架，Windows 使用 Win32 和 ConPTY。

[`zinput 0.1.0`](https://crates.io/crates/zinput/0.1.0)、
[`zgraphics 0.1.0`](https://crates.io/crates/zgraphics/0.1.0)、
[`zpty 0.2.0`](https://crates.io/crates/zpty/0.2.0) 和
[`zterm 0.2.0`](https://crates.io/crates/zterm/0.2.0) 已发布到 crates.io。
四个包均已在工作区之外验证单独及组合使用，依赖全部来自注册表，
包括真实 PTY 会话与图像放置/删除。源码提交、包校验和与制品位置见
[发布记录](docs/releasing.md)。
当前源码中的 zterm 0.2.1 尚未在本轮发布到 crates.io。

```sh
cargo add zinput@0.1.0 --no-default-features
cargo add zgraphics@0.1.0 --no-default-features
cargo add zpty@0.2.0 --no-default-features
cargo add zterm@0.2.0 --no-default-features
cargo add zvte@0.1.3 --no-default-features
```

## Linux 安装与运行

本文命令对应当前源码。安装脚本下载最新已发布标签的版本；尚未发布的改动
请通过源码构建使用。

Arch Linux 可使用 `yay -S ztty`。其他 Linux 发行版可用一键安装脚本，
自动选择 x86_64 或 ARM64 预编译包并校验 SHA256。

```sh
curl -fsSLO https://github.com/LIghtJUNction/ztty/releases/latest/download/install.sh
sh install.sh
```

脚本安装最新已发布版本，默认位置为 `~/.local/bin`。Linux 预编译版需要
glibc、X11、Xft 和 Xrender 运行库。当前源码中的新功能可能尚未进入
最新发布包。

安装 Rust 1.100.0、X11/Xft/Xrender 开发库和等宽字体后：

```sh
cargo build --locked --release
./target/release/ztty
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
```

`make install` 安装到当前用户的 `~/.local`。Linux 下 Ctrl+Shift+C 复制，
Ctrl+Shift+V 粘贴，Shift+拖拽在捕获鼠标的应用中选择文字，
Shift+PageUp/PageDown 浏览历史，Ctrl+点击打开链接。
应用捕获鼠标时，按住 Shift 可使用本地选择、滚轮历史和中键粘贴。

Linux/macOS 默认使用 `SHELL`，缺省为 `/bin/sh`；Windows 使用
`COMSPEC`，缺省为 `cmd.exe`。用 `-e` 指定其他命令。macOS 与 Windows
的打包和原生验收入口见 [发布工作流](.github/workflows/release.yml)。
Windows 包携带固定版本的 Microsoft ConPTY 运行组件，库也支持回退到
系统自带 API，细节见 [`zpty` 文档](crates/zpty/README.md)。

## 多窗口、会话与 MCP（Linux）

正常启动多个 ztty 即可共享资源，无需额外开启复用模式：

```sh
ztty --name work -e /bin/bash &
ztty --name logs -e /bin/sh &
ztty attach
ztty attach work
```

`ztty attach` 不带参数时列出全部本地和 MCP 会话。带名字时打开该会话的
窗口，已有窗口则将它置前；命令会等待窗口关闭后返回。示例中的 `&`
让启动命令在后台等待，当前 shell 可以继续使用。

本地会话地址为 `ztty://local/work`。MCP 创建的会话按来源命名：

| 来源 | 地址示例 |
| --- | --- |
| stdio | `ztty://mcp/stdio/work` |
| HTTP | `ztty://mcp/http/127.0.0.1:8732/work` |

同一个短名字可以出现在不同来源下；重名时使用完整地址。名字支持 1–64 个
ASCII 字母、数字、点、下划线或连字符，省略名字时自动生成 `session-N`。

```sh
# 默认 stdio，启动时不打开窗口
ztty mcp

# Streamable HTTP，地址为 http://127.0.0.1:8732/mcp
ztty mcp --transport http --host 127.0.0.1 --port 8732

ztty attach ztty://mcp/stdio/work
```

六个 MCP 工具分别创建、列出、读取、输入、调整尺寸和关闭会话。MCP 会话
在连接退出后继续保留，关闭观察窗口只会脱离显示；程序退出后仍可查看输出
和退出码，通过 `session_close` 释放会话。普通本地窗口关闭时会结束其会话。

HTTP 支持配置主机和端口，端口 `0` 自动选择空闲端口。监听非回环地址时
需要通过 `ZTTY_MCP_TOKEN` 或 `--token` 配置令牌。每次输入最多 64 KiB；
读取按字节游标分页，单页可多返回最多 3 字节以补全 UTF-8 字符。

完整的客户端配置、工具参数和生命周期见 [会话与 MCP](docs/sessions.md)。

## 在终端里显示图片

在 ztty 中运行现成工具：

```sh
kitten icat --scale-up photo.jpg
kitten icat transparent.png
```

JPEG 由工具先转换成协议支持的像素数据，再通过 PTY 发送。第一版
自动探测会选择 `stream` 内联传输，也可以显式指定
`--transfer-mode=stream`。第一版覆盖能力查询、zlib 压缩、分片、图片 ID、
放置、裁剪、缩放与删除。文件路径传输、共享内存和动画留待后续。

Linux 图像验收通过真实 PTY 发送数据，再读取窗口像素检查颜色、
透明边缘、裁剪和删除后的画面；还检查滚动、窗口缩放、主备屏切换及
坏输入后的恢复。实际 `kitten icat` 0.50 已通过照片和透明 PNG 检查。
图片及传输有独立内存上限，详见 [图像协议范围](docs/protocols.md#kitty-graphics)。

![真实 Hyprland/XWayland 中显示照片和透明 PNG](docs/assets/kitty-graphics-hyprland.png)

## 外观

Linux 默认使用 `~/.config/ztty/ztty.conf`，macOS 默认使用
`~/Library/Application Support/ztty/ztty.conf`；这两个平台显式设置
`XDG_CONFIG_HOME` 时使用其下的 `ztty/ztty.conf`。Windows 使用
`%APPDATA%\ztty\ztty.conf`。
也可以通过 `ztty --config PATH` 指定配置文件。
配置在下次启动时生效，`--font` 优先于文件中的字体。

```ini
font = monospace:size=11
window-padding = 16
foreground = #e1e7f0
background = #161a23
background-opacity = 92
background-blur = true
cursor-shape = underline
cursor-blink = true
cursor-blink-interval = 500
cursor-color = #b1ccff
selection-foreground = #f0f4fa
selection-background = #394969
palette = 1=#e06c75
```

Linux 透明度用百分比表示，仅作用于默认背景。模糊由桌面合成器实现，
`background-blur` 控制兼容 KWin 的 X11 请求，Hyprland 使用自己的
窗口规则。设为 `background-opacity = 100` 可让 Linux 窗口完全不透明。
macOS 实现使用原生视觉效果视图提供模糊，Windows 当前使用不透明
GDI 画布；三平台外观一致性仍待原生验收。

光标支持方块 `block`、下划线 `underline` 和竖线 `bar`，闪烁间隔
以毫秒为单位。终端应用可以通过 DECSCUSR 和 DEC 私有模式 12
控制当前光标。完整选项和范围见 [英文说明](README.md#appearance)。

## 性能与当前范围

同机配对测量中，8 个空闲窗口的 ztty 总 PSS 从 47.021 MiB 降至
15.366 MiB，减少 67.3%；统计包含共享进程和 8 个等待命令返回的启动进程。
从 1 个增加到 8 个窗口时，每新增窗口的平均 PSS 从 5.198 MiB 降至
0.599 MiB。单窗口 PSS 则增加了 0.539 MiB。

每个窗口的 X server pixmap 仍占 3,273,728 字节，单独计算；shell 和它运行
的程序也另计。完整表格、测量条件及原始数据见
[多窗口内存报告](docs/performance.md#multiple-window-memory)。
核心解析、PTY 往返与实际屏幕显示延迟分别记录。

当前通过 X11/XWayland 提供 Linux 窗口。原生 Wayland、专用 GPU
渲染后端、Sixel、Kitty 文件/共享内存传输与动画、Unicode 图片占位符、
相对放置、OSC 52、完整复杂字形布局和历史重排是后续工作。
“超过 Kitty/Ghostty”是项目目标，比较结论按实际完成的相同条件实验报告。

## 验证

```sh
sh tools/check.sh
cargo test --workspace --doc
python3 tests/gui_smoke.py --binary ./target/release/ztty
python3 tests/graphics_smoke.py --binary ./target/release/ztty
python3 tests/shared_gui_smoke.py --binary ./target/release/ztty
python3 tests/repaint_gui.py --binary ./target/release/ztty
python3 tests/test_throughput_benchmark.py
python3 tests/mcp_transport.py --binary ./target/release/ztty
python3 tests/mcp_sessions.py --binary ./target/release/ztty
# 可选：真实 Fcitx 5、Pinyin 插件和独立 D-Bus 会话。
python3 tests/gui_smoke.py --fcitx --binary ./target/release/ztty
```

界面和图像测试创建独立 Xvfb 会话，需要 Xvfb、python-xlib 和 Pillow；
图像测试还需要 `kitten`。结果和截图写入 `artifacts/`。Fcitx 检查验证
普通按键与中文提交进入 PTY，保留为单独的可选验收。

进一步阅读：[架构](docs/architecture.md)、[协议范围](docs/protocols.md)、
[性能](docs/performance.md)、[发布](docs/releasing.md)。
