# ztty

ztty 是一个用 Rust 开发的 Linux 终端，生产代码不依赖 Rust 标准库。
目标是以更低的内存占用和更快的交互响应，逐步超过 Kitty 与 Ghostty。

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

性能数据分开记录核心解析与屏幕更新、PTY 往返、完整进程内存和图形
缓冲占用。基准的机器、尺寸、样本量、分位数与复现命令见
[性能报告](docs/performance.md)。微秒级的核心处理时间需要与显示器
刷新、桌面合成等时间分开理解。

当前通过 X11/XWayland 提供 Linux 窗口；原生 Wayland、专用 GPU
渲染后端、图像协议、完整复杂字形布局和历史重排是后续工作。
自动化界面验收使用 Xvfb/X11。
“超过 Kitty/Ghostty”是项目目标，比较结论按实际完成的相同条件实验报告。
