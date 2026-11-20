# ztty

[English](README.md) · [文档](docs/README.md) · [下载](https://github.com/LIghtJUNction/ztty/releases)

ztty 是面向 Linux、macOS 和 Windows 的原生终端，核心用不依赖标准库的
Rust 编写。Linux 上，同一个运行中的会话可以通过窗口、命令行或 MCP 客户端使用。

本页跟随 **0.5.0 之后尚未发布的 main 开发源码**。已发布的二进制及对应变更记录见
[下载页](https://github.com/LIghtJUNction/ztty/releases)。

## Linux 安装

Arch Linux 使用 `yay -S ztty`。下载已发布的 x86_64 或 ARM64 预编译版：

```sh
curl -fsSLO https://github.com/LIghtJUNction/ztty/releases/latest/download/install.sh
sh install.sh
ztty
```

安装器校验归档后安装到 `~/.local/bin`。下文的程序热替换尚未发布，需要构建当前源码。
在 Wayland 桌面上，默认的 X11 后端通过 XWayland 运行。

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
git clone --recurse-submodules https://github.com/LIghtJUNction/ztty.git
cd ztty
cargo build --locked --release
./target/release/ztty --font 'monospace:size=12' --geometry 100x28
./target/release/ztty --cwd /path/to/project -e /bin/bash
sh install.sh --from-binary ./target/release/ztty
```

已有克隆请先运行 `git submodule update --init --recursive`，再进行构建。

Linux/macOS 使用 `SHELL`，缺省为 `/bin/sh`；Windows 使用 `COMSPEC`，
缺省为 `cmd.exe`。用 `-e` 指定其他程序。安装更新后，已有共享进程仍运行原来的代码；
[共享进程升级与热替换](docs/sessions.md#upgrading-a-running-broker)说明如何更新及路由新会话。
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

`Ctrl+Shift+V` 粘贴文本；`Ctrl+V` 默认交给前台应用，Codex CLI 0.170.0 用它
添加剪贴板图片附件。当前开发源码会在终端文本粘贴时拒绝只有图片的剪贴板，
避免把图片变成字符。详见[图片粘贴与格式检查](docs/input-compatibility.md#clipboard-images-and-application-paste-unreleased-main-2026-11-20)。

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

## 替换运行中的程序（开发源码）

由当前源码启动的 Linux X11/XWayland 或无窗口共享进程支持程序热替换。
在要更新的会话内运行：

```sh
ztty reload
ztty reload --binary /absolute/path/to/ztty
```

`ZTTY_SOCKET` 决定目标共享进程。不指定 `--binary` 时，目标就是本次调用的 CLI
自身的可执行文件；命令不会下载更新。替换保留共享进程 PID、原 shell 和命令的 PTY、
会话身份、输入队列与输出位置、终端屏幕和滚屏历史，以及 X11 标签页和分屏。
窗口会重建，可能短暂消失。原生 Wayland、macOS 和 Windows 暂不支持此操作。

已发布的 0.5.0 旧共享进程会安全拒绝，即使用新 CLI 请求也不能原地迁移。
保留旧窗口直到工作结束，用新源码构建的共享进程开始新工作；后续可在该进程内热替换。
[热替换指南](docs/sessions.md#reload-the-running-broker)说明验证过程、临时拒绝条件、
显示恢复和首次过渡步骤。Ctrl+Shift+R 仍然只重载配置。

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
[2026-11-20 性能与热替换报告](docs/benchmarks/performance-reload-2026-11-20.md)
提供固定源码、可执行文件哈希、全部原始样本和验收范围。与保留的旧版对照，
软件渲染的 ASCII/彩色输出吞吐：单窗口从 171.7 提高到 257.5 MiB/s，
10 窗口从 158.5 提高到 229.2 MiB/s；独立 Unicode 滚屏测试的三轮成对增益
中位数为 53.4%。报告也保留无窗口 ASCII 路径的小幅负向结果。

三应用比较使用私有 Xvfb，统一强制软件 OpenGL；ztty 实际驱动为 llvmpipe。
10 个窗口保持打开，每次只测一个输出生产者。它测量 PTY/DSR 逻辑处理，
不能代表物理帧率或真实 GPU 桌面排名。

| 10 窗口共享实例 | 应用 PSS，MiB | 输出，MiB/s |
| --- | ---: | ---: |
| ztty | 326.9 | 150.4 |
| Ghostty | 809.0 | 140.1 |
| Kitty | 354.6 | 53.0 |

![Xvfb 与 llvmpipe 下的十窗口比较](docs/benchmarks/performance-reload-20261120/final-terminals.svg)

表中为三次独立运行的中位数；字体度量、像素尺寸、宿主内存压力与渲染器选择
均限制结果的适用范围，报告保留全部波动。单窗口 ztty/Ghostty 吞吐区间重叠，
不能据此声称稳定排名。软件与 GPU 路径也通过了记录中的标签切换完整帧、
同步输出保持与提交检查；物理桌面及合成器行为仍需单独验证。
<!-- terminal-benchmark:end -->

本地开发检查使用 `sh tools/check.sh`。

## 可复用库

| 库 | 在 ztty 中的职责 | 发布状态 |
| --- | --- | --- |
| [zgpui](https://github.com/LIghtJUNction/zgpui) | UI 场景与布局 | 已发布 [0.1.0](https://crates.io/crates/zgpui/0.1.0) |
| [zinput](https://github.com/LIghtJUNction/zinput) | 键盘、鼠标与粘贴编码 | 已发布 [0.1.3](https://crates.io/crates/zinput/0.1.3) |
| [zpty](https://github.com/LIghtJUNction/zpty) | PTY / ConPTY 进程管理 | 已发布 [0.2.2](https://crates.io/crates/zpty/0.2.2) |
| [zgraphics](https://github.com/LIghtJUNction/zgraphics) | 图片协议与放置 | 已发布 [0.2.0](https://crates.io/crates/zgraphics/0.2.0) |
| [zterm](https://github.com/LIghtJUNction/zterm) | 终端屏幕与协议状态 | 已发布 [0.5.1](https://crates.io/crates/zterm/0.5.1) |

上表保留各库已经发布的版本。当前 main 还通过固定 Git 子模块提交使用尚未发布的
`zterm` checkpoint 与 `zpty` PTY 接管 API；上表的已发布版本不包含这些新增接口。
构建时请使用仓库记录的子模块版本和 `Cargo.lock`。

可独立使用的 Project Z 库见[架构说明](docs/architecture.md)；
使用指南与验收记录集中在[文档索引](docs/README.md)。

MIT © LIghtJUNction
