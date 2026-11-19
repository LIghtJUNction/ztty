# 原生 Wayland（实验性）

Linux 可以通过 `--backend wayland` 打开独立的原生 Wayland 窗口。这个路径使用 xdg-shell、系统 XKB 和现有 EGL/GLES 渲染器；默认启动仍使用 X11/XWayland 的共享会话后端。

## 构建与启动

Wayland 支持是显式启用的 Cargo feature。它没有增加第三方 Rust 依赖；生产 Rust 代码继续使用 `no_std`。需要系统的 `wayland-client`、`wayland-egl`、`wayland-cursor`、`xkbcommon` 开发文件，以及 `wayland-protocols` 和 `wayland-scanner`。原有的 EGL、GLES、FreeType、Fontconfig、HarfBuzz 依赖仍然需要。

```sh
cargo build --release --features wayland
./target/release/ztty --backend wayland --config ./wayland.conf
```

一个最小配置文件：

```ini
font=monospace:pixelsize=16
renderer=gpu
window-padding=8
background-opacity=100
```

`--geometry`、`--scrollback`、`--font`、`--cwd` 和 `-e COMMAND [ARG...]` 可用于该路径。子进程在指定目录启动，`PWD` 与实际目录一致，退出状态传回调用者。没有 Wayland feature 的构建会明确提示重新构建；不会悄悄转到另一个后端。

## 本轮实现范围

| 能力 | 状态与边界 |
| --- | --- |
| 原生窗口 | xdg-shell 创建、尺寸配置确认、标题、关闭；不连接 X 服务器 |
| 绘制 | 复用完整帧 GPU 渲染器，包括终端字符、颜色和现有图像模型；窗口可由硬件或 Mesa 软件 GLES 驱动 |
| 键盘 | 系统 XKB 布局、UTF-8、compose、修饰键、按下/释放和受控重复；编码使用现有 zinput，要求 wl_seat 版本至少为 5 |
| 滚动与缩放 | Shift+PageUp/PageDown 浏览历史；窗口尺寸更新真实 PTY，复用已保留文本的回流 |
| 鼠标 | 已接入 wl_pointer、终端鼠标上报和滚动；本轮没有物理鼠标或完整指针协议验收 |
| 帧调度 | 仅在完整画面准备好后 swap；通过 frame callback 限制排队；同步输出超过 250 ms 可结束停滞帧 |
| 生命周期 | 合成器断连报告错误，关闭已创建的 PTY 会话并有界等待自己的子进程 |
| 共享会话与 MCP | 此路径不进入 X11 broker；标签、分屏、attach/observe、会话名称与共享输入控制尚未接入 |
| 输入法和剪贴板 | 尚未接入 Wayland text-input/输入法、系统剪贴板、primary selection；XKB 的 Unicode 输入不等于输入法验收 |
| 扩展与界面 | 尚未接入自定义按键、文本插件、原生 UI 场景或搜索/命令面板；配置了自定义按键或文本插件时会明确拒绝启动 |
| 显示缩放与装饰 | 尚未接入 fractional scale/HiDPI、客户端装饰、模糊协议；使用合成器提供的窗口布局 |

实验路径的边界独立于默认后端的功能。没有必要为试用 Wayland 改动现有 X11 会话配置；建议使用独立配置文件，避免无声忽略自定义扩展或按键。

## 实现机制与系统边界

`native/wayland_bridge.c` 只处理平台对象和事件。它拥有 `wl_display`、registry、xdg-shell surface/toplevel、seat/keyboard/pointer、`wl_egl_window` 和光标主题。协议代码由系统的 xdg-shell XML 在构建目录中生成，不把生成代码或协议定义复制成新的实现。

`src/platform/wayland.rs` 处理终端策略。它复用 `SessionCore` 管理 PTY、`zterm` 解析和回流、`zinput` 编码以及共享 `gpu::Renderer` 绘制。新后端在初始化 EGL/字体驱动前创建子进程，避免在驱动可能启动线程之后调用 `forkpty`。析构先释放渲染器和 EGL，再销毁原生 Wayland 对象。

平台事件和 PTY 使用同一次 `poll` 等待。C 边界遵守 `wl_display_prepare_read` 后必须配对 `read_events` 或 `cancel_read` 的规则，避免 EGL 已消费事件后再次阻塞分发。每轮最多读取 64 KiB PTY 输出，随后处理输入和窗口事件；这只是工作量上限，不是吞吐提升声明。

输入事件队列固定为 512 项，XKB keymap 上限 16 MiB，PTY 输入队列上限 1 MiB，单帧沿用渲染器的 8192 边长和 16 Megapixel 上限。超限、连接错误和资源创建失败向调用者返回错误。关闭时只处理本进程创建的 PTY 会话，不扫描或停止其他终端。

后续若统一到 zsys，可分别抽取下面的边界，不需要把终端模型搬到系统层：

| 边界 | 当前系统接口 |
| --- | --- |
| 连接与窗口 | libwayland-client、xdg-shell、registry 生命周期 |
| 原生呈现目标 | libwayland-egl 的 create/resize/destroy，指针交给公共 EGL 渲染器 |
| 键盘 | libxkbcommon keymap/state/compose、键盘重复时间 |
| 指针与光标 | wl_pointer、libwayland-cursor、wl_shm |
| 等待 | poll、CLOCK_MONOTONIC、Wayland prepare/read/cancel |
| 终端子进程 | 现有 zpty；不属于 Wayland C 边界 |

## 验证方法

实际 GUI 测试启动独立的 headless Sway，使用独立 `XDG_RUNTIME_DIR` 和 Wayland socket，禁用 XWayland，不访问物理输入设备。测试通过 wtype 的 virtual-keyboard 协议输入，并通过 grim 读取合成后的屏幕像素。每次新建虚拟键盘后先等待 100 ms 完成 seat 能力通知和 `wl_keyboard` 订阅；这段设备建立时间不作为终端输入延迟测量。

```sh
cargo clippy --features wayland --bin ztty -- -D warnings
cargo check --workspace --no-default-features
python3 tests/wayland_gui.py \
  --binary target/release/ztty \
  --sway /path/to/sway \
  --output artifacts/wayland-validation/gui
```

测试保留窗口截图、合成器日志、子进程输入字节、PTY 尺寸和 `validation.json`。结果绑定二进制 SHA-256；失败或异常不会写成通过。测试使用软件 Mesa EGL，不能证明物理显示器的延迟、显卡驱动兼容性、输入法完成度或比其他终端更快。

协议和事件循环依据：[Wayland Client API](https://wayland.freedesktop.org/docs/html/apb.html)；构建使用系统 `wayland-protocols/stable/xdg-shell/xdg-shell.xml`。
