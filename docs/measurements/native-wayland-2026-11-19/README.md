# 原生 Wayland 验收记录

2026-11-19 UTC。实验后端在独立 headless Sway 中通过 **23/23 GUI 检查**及 **4/4 CLI 错误入口检查**。这些结果说明单窗口原生 Wayland 路径可以完成创建窗口、真实 PTY 输入输出、重绘、调整尺寸和退出；它们不代表默认 X11 后端的全部功能已经迁移。

## 可核对的身份

| 项目 | 值 |
| --- | --- |
| 生产源码提交 | `4d9027099d76fd7a80414496e09fea531cd8849e` |
| 测试时源码状态 | clean，见 `source_worktree_dirty: false` |
| 二进制 SHA-256 | `8c96835d857fcc4b6e147a9a2429784a5be98259d12d3d0a62124fd78f062a14` |
| 独占编译目录 | `target-wayland-resume` |
| 编译器 | rustc 1.100.0 |
| 合成器 | Sway 1.12，headless backend，1280×720 |
| 绘制环境 | `WLR_RENDERER=pixman`、`LIBGL_ALWAYS_SOFTWARE=1`；未查询具体 GLES renderer 字符串 |

[environment.json](environment.json) 保存生产文件哈希、Git tree、系统库版本与命令。[validation.json](validation.json) 保存每项 GUI 断言及实际输入字节。[cli.json](cli.json) 保存错误入口命令、退出码与 stderr。报告收录不会改变所记录的产品二进制。

## 通过的检查

| 组别 | 数量 | 实际观测 |
| --- | ---: | --- |
| 创建与环境 | 2 | 原生 app_id 可见，PTY 子进程存活，PWD 与工作目录一致，TERM/TERM_PROGRAM 正确 |
| EGL 呈现 | 1 | 合成后的截图中出现 4000 个指定红色像素 |
| 键盘 | 8 | ASCII、中文 UTF-8、Ctrl、方向键、Alt、长按重复、释放后停止、Kitty 按下与释放字节 |
| 标题 | 1 | OSC 标题更新到合成器窗口树 |
| 尺寸与内容 | 2 | 4 个 configure 尺寸改变真实 PTY 的行列；颜色块在缩放后仍存在 |
| 历史 | 2 | Shift+PageUp 改变视图，PageDown 恢复原实时画面 |
| 同步重绘 | 2 | 同步区间保持旧的完整帧，结束后绿色完整帧可见 |
| 生命周期 | 5 | 合成器请求关闭退出 0；回收子进程；子进程状态 37 传回；合成器断连退出 1 并回收子进程 |

CLI 检查覆盖非法后端名、Wayland 不支持的共享会话名称、连接不存在的 Wayland socket，以及连接失败前不创建命令子进程。最终 `owned_children_remaining` 为空。

严格编译检查也通过：

```sh
CARGO_TARGET_DIR=target-wayland-resume CARGO_BUILD_JOBS=2 \
  cargo build --release --features wayland
CARGO_TARGET_DIR=target-wayland-resume CARGO_BUILD_JOBS=2 \
  cargo clippy --features wayland --bin ztty -- -D warnings
CARGO_TARGET_DIR=target-wayland-resume CARGO_BUILD_JOBS=2 \
  cargo check --workspace --no-default-features
```

对应日志：[build.log](build.log)、[committed-build.log](committed-build.log)、[clippy.log](clippy.log)、[default-check.log](default-check.log)。提交后再次构建的二进制哈希与首次成功构建一致。clippy 的等待时间包含共享 Rust 编译锁排队，不用于比较构建速度。

## 复现 GUI 验收

需要 Sway/swaymsg、wtype、grim、Python 和 Pillow。开发机未预装 Sway，因此本次只在忽略的 artifacts 目录中解包已验证签名的 Arch Sway 包，没有安装或替换系统组件。

```sh
python3 tests/wayland_gui.py \
  --binary target-wayland-resume/release/ztty \
  --sway artifacts/wayland-test-tools/root/usr/bin/sway \
  --output artifacts/wayland-validation/gui-committed
```

脚本自行创建权限为 0700 的临时 runtime 与独立 Wayland socket，禁用 XWayland，清除继承的 DISPLAY/SWAYSOCK，不使用物理输入设备。它仅关闭自己创建的合成器、终端和已核对 PID/启动时间的子进程。

每次 wtype 调用新建一个虚拟键盘。本次早期测试曾在新设备建立时漏掉首键；验收脚本增加 100 ms 的 seat 能力通知和键盘订阅建立等待后再注入。这个等待属于测试设备初始化，不是终端输入延迟。最终源码提交上的完整重跑通过。

## Ubuntu 24.04 头文件兼容

从 Ubuntu 镜像解包 `libwayland-dev_1.22.0-2.1build1_amd64.deb`，使用其中 `WAYLAND_VERSION=1.22.0` 的头文件，以 `cc -std=c11 -O2 -fPIC -Wall -Wextra -Werror` 编译 C bridge 与生成的 xdg-shell protocol，均退出 0；[完整命令和包 SHA-256](ubuntu-1.22-headers.json)。测试没有安装这些包，也没有更换本机系统库。它验证旧头文件的源码兼容性，不是 Ubuntu 完整构建或运行验收。

bridge 绑定 wl_seat v5、wl_compositor 至多 v4、xdg-shell v1。pointer listener 只用 frame/source/stop/discrete 等既有字段，没有使用 axis_relative_direction、axis_value120 等新字段，因而无需增加兼容宏。

Wayland 增量依赖如下；原有 X11、字体和 EGL/GLES 依赖仍然需要，因为两个后端在同一可执行文件内。

| 平台 | 构建增量 | 运行增量 |
| --- | --- | --- |
| Ubuntu 24.04 | `libwayland-dev`、`wayland-protocols`、`libxkbcommon-dev`；libwayland-dev 依赖 libwayland-bin，提供 wayland-scanner | `libwayland-client0`、`libwayland-egl1`、`libwayland-cursor0`、`libxkbcommon0` |
| Arch | `wayland`、`libxkbcommon`；makedepends 为 `wayland-protocols`、`pkgconf` | `wayland`、`libxkbcommon` |

包信息来源：[Ubuntu libwayland-dev](https://packages.ubuntu.com/noble/amd64/libwayland-dev)、[Arch wayland](https://archlinux.org/packages/extra/x86_64/wayland/)、[Arch libxkbcommon](https://archlinux.org/packages/extra/x86_64/libxkbcommon/)。实际二进制的直接动态依赖保存在 [needed-libraries.txt](needed-libraries.txt)。

## 截图与未测边界

[实际原生窗口（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)

[缩放后（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)、[实时历史（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)、[向上滚动（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)、[同步前（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)、[同步中（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)、[同步后（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view) 保存了相同测试产生的完整截图。内容全部为合成测试数据。

本轮没有验收物理显示器、硬件 GPU、触控板或完整鼠标协议、真实输入法、系统剪贴板、HiDPI、分数缩放、多窗口/标签/分屏、共享 broker/MCP、自定义按键或文本插件。复杂文字排版和图形协议沿用公共渲染器，但本组 Wayland 检查不能单独证明其完整正确性。功能差异见 [原生 Wayland 使用与边界](../../native-wayland.zh-CN.md)。

23 项检查的总耗时和同步截图时间只描述测试执行条件，不是帧率、输入延迟或吞吐 benchmark；本记录不作性能领先结论。
