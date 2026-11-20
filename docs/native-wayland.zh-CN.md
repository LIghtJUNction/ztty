# 原生 Wayland（实验性）

本文对应统一候选 [6053a97](https://github.com/LIghtJUNction/ztty/commit/6053a97920c29c852cd92d8b96e35f62cff92ecb)。
原生 Wayland 已使用与 X11 相同的 Linux 会话进程、PTY 管理和输入控制。
默认启动仍选 X11；构建时启用 `wayland` feature，运行时用 `--backend wayland` 选择原生窗口。

ztty 0.5.0 仍是源码候选，正式下载保持 0.4.1。下面说明当前源码的使用方式；
本地检查、原生 GUI 验收和正式发布的状态分别记录。

## 构建与启动

先取得递归子模块；已有克隆运行 `git submodule update --init --recursive`。
除 [Linux 基础依赖](../README.zh-CN.md#从源码构建)外，还需要：

```sh
# Arch Linux
sudo pacman -S --needed wayland wayland-protocols libxkbcommon

# Debian / Ubuntu
sudo apt install libwayland-dev libwayland-bin wayland-protocols libxkbcommon-dev
```

当前官方 Linux 打包配置启用 Wayland；这不改变默认运行前端。
可用独立配置文件 `wayland.conf` 试用：

```ini
font = monospace:pixelsize=16
renderer = gpu
window-padding = 8
background-opacity = 100
```

```sh
cargo build --locked --release --features wayland --bin ztty
./target/release/ztty --backend wayland --config ./wayland.conf --name work
./target/release/ztty attach --backend wayland --observe work
./target/release/ztty attach --backend wayland work
```

`renderer = auto` 和 `gpu` 都使用 EGL/GLES，`software` 会明确报错。
Mesa 软件 GL 也走 EGL/GLES，并非 Xft 后端；渲染器通过配置选择，没有 `--renderer` 参数。
未编入 Wayland feature 的二进制会提示重新构建。

## 会话与输入

| 能力 | 当前实现 |
| --- | --- |
| 多窗口与会话 | 各会话有独立 PTY；detach 后 attach 同一会话保留原 PTY |
| X11 / Wayland 混用 | 同一个 broker 按创建或附着请求选择前端，不由第一个窗口决定后续窗口类型 |
| MCP 与观察模式 | 使用既有会话身份和输入控制权检查；只读观察不会取得输入权 |
| 绘制与调整尺寸 | xdg-shell 原生窗口、EGL/GLES 完整帧、真实 PTY 尺寸调整与保留文本重排 |
| 键盘 | 系统 XKB、compose、修饰键、受控重复及已有 zinput 编码 |
| 剪贴板 | 原生 clipboard、primary selection、有界异步传输与原有 OSC 52 策略 |
| 输入法 | text-input-v3 焦点、光标矩形、可见预编辑和 UTF-8 提交 |

显示后端负责窗口、输入事件和呈现目标；会话进程继续保存 PTY、终端屏幕、历史和控制权。
同一个会话因此可以重新附着到窗口，MCP 也仍经过原来的权限检查。
每个原生窗口持有自己的 Wayland/EGL 资源；这里没有跨窗口共享字形或纹理缓存的承诺。

剪贴板单份文本上限为 4 MiB，存活 source 总量上限为 8 MiB，最多 16 个发送流。
读写工作有界，超时为 2 秒；超限或传输失败不会把半份文本注入终端。
Primary selection 需要合成器给出的有效输入 serial；程序不会伪造输入或绕过合成器规则。

预编辑只显示候选文字；提交后才把 UTF-8 送入原输入路径。
终端不拥有 shell 的编辑缓冲，因此不伪造 surrounding text，也不把删除周围文本请求
转换成任意退格。无效删除批次会拒绝，之后的正常输入仍可继续。

## 当前边界

- Wayland 明确拒绝窗口内标签页、分屏和工作区容器；使用独立会话窗口。
- 不提供 X11 子窗口嵌入。窗口置前服从合成器，尚无 xdg-activation token 流程。
- HiDPI、分数缩放及跨显示器缩放变化尚未完成验收。
- 物理 GPU 呈现延迟、显卡驱动组合和真实桌面输入法尚未实机验证。
  已有 IME 证据来自私有协议 peer，不能代替全部 fcitx/IBus 配置的测试。
- 本文的 Linux 验收不覆盖 macOS/Windows。

## 验证与复现

[共享 Wayland 报告](reports/native-wayland-shared-20261119/README.zh-CN.md)
保留 feature owner 的冻结源码、ELF、逐文件摘要和实际结果。
其生产提交为 `8b9f566`，65 项 Wayland GUI 检查使用报告中的 `7fbaf994…` ELF；
环境为私有 headless Sway 1.12、pixman 与 Mesa 软件 EGL。
IME 使用真实 input-method-v2 peer，经合成器转发到终端的 text-input-v3。

统一候选 `6053a97` 已合入该实现，并通过默认 release 构建、默认/Wayland 严格 Clippy、
完整本地检查及随后单独执行的 6 项软件 EGL 单元测试。
新整合 GUI 与六架构正式原生发布门禁仍待完成；旧 owner 的 65 项不能转记为新 ELF 的通过结果。
后续回执集中在[依赖传播维护记录](dependency-propagation.md#统一候选与验收归属)。

复现 GUI 还需要 `sway wtype grim wl-clipboard xvfb python3-pil python3-xlib`，
以及 C 编译器和 pkg-config。先构建要测试的二进制，再运行：

```sh
python3 tests/wayland_gui.py --binary target/release/ztty \
  --output artifacts/wayland-acceptance
python3 tests/wayland_services.py --sanitize \
  --output artifacts/wayland-source-budget
```

GUI 入口会构建固定协议版本的 IME peer，并创建私有运行目录、显示服务和 broker socket。
自定义 Sway 可加 `--sway /absolute/path/to/sway`，必要时显式指定 `--swaymsg`。
保存的结果必须绑定实际源码与二进制摘要；软件 EGL 的像素、协议和输入检查不构成硬件性能测量。
