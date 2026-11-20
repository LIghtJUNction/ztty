# ztty

[English](README.md) · [文档](docs/README.md) · [下载](https://github.com/LIghtJUNction/ztty/releases)

ztty 是面向 Linux、macOS 和 Windows 的原生终端，核心用不依赖标准库的
Rust 编写。Linux 上，同一个运行中的会话可以通过窗口、命令行或 MCP 客户端使用。

ztty 当前正式下载版本仍为 **0.4.1**。本页描述 **0.5.0 源码候选版**，
平台范围对应 `iter/resume-20261120` 的
[6053a97](https://github.com/LIghtJUNction/ztty/commit/6053a97920c29c852cd92d8b96e35f62cff92ecb) 检查点。
应用正式发布和设备安装尚未完成。

## Linux 安装

Arch Linux 使用 `yay -S ztty`。下载已发布的 x86_64 或 ARM64 预编译版：

```sh
curl -fsSLO https://github.com/LIghtJUNction/ztty/releases/latest/download/install.sh
sh install.sh
ztty
```

安装器校验归档后安装到 `~/.local/bin`。0.4.1 Linux 版需要 glibc、X11、
Xft 和 Xrender；Wayland 桌面通过 XWayland 运行。

## 从源码构建

需要 Rust **1.100.0**、C 编译器和等宽字体。当前 Linux 源码的系统依赖：

```sh
# Arch Linux
sudo pacman -S --needed base-devel pkgconf libx11 libxft libxrender \
  fontconfig freetype2 harfbuzz libglvnd ttf-dejavu

# Debian / Ubuntu
sudo apt install build-essential pkg-config libx11-dev libxft-dev libxrender-dev \
  libfontconfig1-dev libfreetype-dev libharfbuzz-dev libegl1-mesa-dev \
  libgles2-mesa-dev fonts-dejavu-core
```

```sh
git clone --recurse-submodules --branch iter/resume-20261120 https://github.com/LIghtJUNction/ztty.git
cd ztty
cargo build --locked --release
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
./target/release/ztty --cwd /path/to/project -e /bin/bash
sh install.sh --from-binary ./target/release/ztty
```

已有克隆请先运行 `git submodule update --init --recursive`，再进行构建。

Linux/macOS 使用 `SHELL`，缺省为 `/bin/sh`；Windows 使用 `COMSPEC`，
缺省为 `cmd.exe`。用 `-e` 指定其他程序。安装更新会保留现有会话，
[共享进程升级规则](docs/sessions.md#upgrading-a-running-broker)说明新旧会话的路由。
macOS 和 Windows 的原生构建、打包入口见[发布说明](docs/releasing.md)。

## 核心能力

- VT 控制序列、真彩色、鼠标报告、括号粘贴和 Kitty 增强键盘协议。
- 完整字符簇存储、历史重排，以及跨自动换行的文字复制。
- Kitty 图片和 Sixel；Linux 增加动画、Unicode 占位符和相对位置。
- Linux X11 Kitty 文件、共享内存传输，以及有界的 OSC 52 剪贴板访问。
- Linux 共享会话；X11 标签页、分屏、历史搜索和命令面板。
- Linux X11 GPU 文字整形、Xft 回退、配置热重载和 XIM 输入法恢复。
- 可选的 [Shell 集成](shell/README.md)和[可信文本扩展](docs/text-plugins.zh-CN.md)。

Linux X11 默认使用 `renderer = auto`：可用时选择硬件 EGL，否则使用 Xft。
[配置指南](docs/configuration.md)和[协议说明](docs/protocols.md)列出支持的模式与范围。

## Linux X11 常用操作

| 操作 | 快捷键 |
| --- | --- |
| 复制 / 粘贴 | Ctrl+Shift+C / V |
| 在捕获鼠标的 TUI 中选择文字 | Shift+拖拽 |
| 搜索历史 / 打开命令面板 | Ctrl+Shift+F / P |
| 切换会话 | Ctrl+Shift+S |
| 新建标签页 / 向右分屏 | Ctrl+Shift+T / E |
| 下一个 / 上一个标签页 | Ctrl+Tab / Ctrl+Shift+Tab |
| 放大 / 缩小 / 恢复字体 | Ctrl+Plus / Minus / 0 |
| 浏览历史 | Shift+PageUp / PageDown 或滚轮 |
| 打开链接 | Ctrl+点击 |
| 重载配置 | Ctrl+Shift+R |

双击选词，三击选行。浏览历史时，点击 **Back to live ↓** 回到最新输出。
[日常操作](docs/daily-workflow.md)、[工作区](docs/workspaces.md)和
[输入兼容性](docs/input-compatibility.md)说明自定义绑定、小键盘和其他操作。

## 共享会话与 MCP（Linux）

Linux X11 与原生 Wayland 窗口共用同一个会话进程；每个会话保留自己的
PTY、屏幕和历史。X11 窗口还共用显示连接与字体缓存。

```sh
ztty --name work -e /bin/bash &
ztty attach                     # 列出会话
ztty attach work                # 打开窗口或将已有窗口置前
ztty attach --observe work      # 只读观察
ztty mcp                        # stdio MCP
ztty mcp --transport http --host 127.0.0.1 --port 8732
```

MCP 支持创建会话、读取输出、输入、调整尺寸、关闭 PTY 和管理输入控制权。
MCP 创建的会话在连接断开后保留，关闭附加视图也会保留它的 PTY。
HTTP 监听非回环地址时需要令牌。
客户端配置、会话身份校验、JSON/SSE 响应与命令输出读取见[会话指南](docs/sessions.md)；
ssmcp 等工具的接入见[观察模式](docs/ssmcp-observer.md)。

## 外观

Linux 默认配置为 `~/.config/ztty/ztty.conf`，也可用 `--config PATH` 指定文件。

```ini
font = monospace:size=11
window-padding = 16
background = #161a23
foreground = #e1e7f0
background-opacity = 92
renderer = auto
```

Linux X11 会在原窗口中应用有效配置。[配置指南](docs/configuration.md)
包含各平台路径、颜色、光标、快捷键和渲染器选项。

## 平台范围

| 平台 | 当前源码 |
| --- | --- |
| Linux X11/XWayland | GPU 或 Xft；共享会话、标签页、分屏、Attach、MCP |
| Linux 原生 Wayland | 显式启用 EGL/GLES；共享会话、Attach/MCP、剪贴板与 text-input 输入法 |
| macOS | 原生 AppKit/CoreText 窗口与 Darwin PTY；本地会话 |
| Windows | 原生 Win32/GDI 与 ConPTY；本地标签页和分屏 |

macOS 无需 X11，Windows 无需 WSL。Windows 支持字体与工作区快捷键；
macOS 复制粘贴使用 Command+C/V 或 Ctrl+Shift+C/V。
共享会话、Attach 与 MCP 当前属于 Linux 功能。
各平台实际测试范围见[验收说明](docs/protocols.md#acceptance-coverage)。

试用实验性原生 Wayland 后端：

```sh
cargo build --locked --release --features wayland
./target/release/ztty --backend wayland --name work
```

官方 Linux 包的构建会启用 Wayland 支持，运行时仍默认使用 X11。
Wayland 会拒绝标签页、分屏和工作区容器。[Wayland 配置](docs/configuration.md#native-wayland-experimental)
说明额外依赖，以及尚未实机验证的物理 GPU、桌面输入法与显示缩放。

## 性能与开发

<!-- terminal-benchmark:start -->
新的测量结果待补入[本轮基准报告](docs/benchmarks/resume-2026-11-20.md)。
[复现方法](docs/benchmarks/README.md)说明如何运行同条件对比；
[较早的性能记录](docs/performance.md)保留对应的测量环境。
<!-- terminal-benchmark:end -->

本地开发检查使用 `sh tools/check.sh`。

## 可复用库

| 库 | 在 ztty 中的职责 | 发布状态 |
| --- | --- | --- |
| [zgpui](https://github.com/LIghtJUNction/zgpui) | UI 场景与布局 | 已发布 [0.1.0](https://crates.io/crates/zgpui/0.1.0) |
| [zinput](https://github.com/LIghtJUNction/zinput) | 键盘、鼠标与粘贴编码 | 已发布 [0.1.3](https://crates.io/crates/zinput/0.1.3) |
| [zpty](https://github.com/LIghtJUNction/zpty) | PTY / ConPTY 进程管理 | 已发布 [0.2.2](https://crates.io/crates/zpty/0.2.2) |
| [zgraphics](https://github.com/LIghtJUNction/zgraphics) | 图片协议与放置 | 已发布 [0.2.0](https://crates.io/crates/zgraphics/0.2.0) |
| [zterm](https://github.com/LIghtJUNction/zterm) | 终端屏幕与协议状态 | 已发布 [0.5.0](https://crates.io/crates/zterm/0.5.0) |

可独立使用的 Project Z 库见[架构说明](docs/architecture.md)；
使用指南与验收记录集中在[文档索引](docs/README.md)。

MIT © LIghtJUNction
