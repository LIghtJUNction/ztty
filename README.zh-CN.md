# ztty

ztty 是一个用 Rust 开发的 Linux 终端，生产代码不依赖 Rust 标准库。
多个窗口自动复用一个会话进程、显示连接和字体缓存，每个窗口分别保存自己的
PTY、屏幕和历史记录。

终端的工作可以拆成三件事：把键盘操作编码后送给程序，理解程序返回的
文字和控制指令，再把变化的字符画出来。ztty 围绕这三条路径组织代码，
空闲时等待事件，输出时只更新发生变化的行。

## 已经可以做什么

启动真实 shell、运行命令行程序，支持主备屏、滚动区域、UTF-8 和中日韩
宽字符、真彩色、鼠标报告、复制粘贴、输入法接口、OSC 8 超链接，以及
Kitty 增强键盘和同步输出协议。界面采用深色背景、等宽字体和留白。

默认显示 120×32 个字符。历史记录按需增长，字符存储最多 8 MiB；
历史达到上限后复用旧行。详细协议范围以 [英文 README](README.md)
和 [协议说明](docs/protocols.md) 为准。

## 可独立使用的库

| 库 | 做什么 | 使用场景 |
| --- | --- | --- |
| `zvte` | 解析 ANSI/VT 字节流 | 只需要理解终端控制序列的程序 |
| `zinput` | 编码键盘、鼠标、焦点和粘贴事件 | 图形终端、远程终端、终端客户端 |
| `zterm` | 维护屏幕、历史、模式和协议回复 | 自定义终端界面、终端录制和回放 |
| `zpty` | 创建和管理 Linux 伪终端会话 | 终端、进程控制、交互式测试工具 |

这些库都有独立 Cargo 包。`zinput` 和 `zpty` 没有 Cargo 依赖；
`zterm` 复用 `zinput`、你开发的 `zvte` 和 `no_std` 的 Unicode 宽度表。
需要动态内存的地方使用 `alloc`，分配器由宿主提供。

[`zinput 0.1.0`](https://crates.io/crates/zinput/0.1.0)、
[`zpty 0.1.0`](https://crates.io/crates/zpty/0.1.0) 和
[`zterm 0.1.0`](https://crates.io/crates/zterm/0.1.0) 已发布到 crates.io。
三个包均已在工作区之外验证单独及组合使用，依赖全部来自注册表，
包括真实 PTY 会话在内的检查全部通过。

```sh
cargo add zinput@0.1.0 --no-default-features
cargo add zpty@0.1.0 --no-default-features
cargo add zterm@0.1.0 --no-default-features
cargo add zvte@0.1.3 --no-default-features
```

## 安装

本文命令对应当前源码。安装脚本下载最新已发布标签的版本；尚未发布的改动
请通过源码构建使用。

Arch Linux 可使用 `yay -S ztty`。其他 Linux 发行版可用一键安装脚本，
自动选择 x86_64 或 ARM64 预编译包并校验 SHA256。

```sh
curl -fsSLO https://github.com/LIghtJUNction/ztty/releases/latest/download/install.sh
sh install.sh
```

脚本默认安装到 `~/.local/bin`。预编译版需要 glibc、X11、Xft 和 Xrender
运行库。macOS（Intel / Apple Silicon）与 Windows（x64 / ARM64）的
原生后端暂列为后续工作。

## 运行

安装 Rust 1.100.0、X11/Xft/Xrender 开发库和等宽字体后：

```sh
cargo build --locked --release
./target/release/ztty
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
```

`make install` 安装到当前用户的 `~/.local`。Ctrl+Shift+C 复制，
Ctrl+Shift+V 粘贴，Shift+拖拽在捕获鼠标的应用中选择文字，
Shift+PageUp/PageDown 浏览历史，Ctrl+点击打开链接。
应用捕获鼠标时，按住 Shift 可使用本地选择、滚轮历史和中键粘贴。

## 多窗口、会话与 MCP

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

## 外观

编辑 `~/.config/ztty/ztty.conf`，设置 `XDG_CONFIG_HOME` 时使用其下的
`ztty/ztty.conf`。也可以通过 `ztty --config PATH` 指定配置文件。
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

透明度用百分比表示，仅作用于默认背景。模糊由桌面合成器实现，
`background-blur` 控制兼容 KWin 的 X11 请求，Hyprland 使用自己的
窗口规则。设为 `background-opacity = 100` 可让窗口完全不透明。

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

当前通过 X11/XWayland 提供 Linux 窗口；原生 Wayland、专用 GPU
渲染后端、图像协议、完整复杂字形布局和历史重排是后续工作。
自动化界面验收使用 Xvfb/X11。
当前结果覆盖报告中列出的工作负载，完整历史、复杂应用和硬件图形路径需要
各自的测量。
