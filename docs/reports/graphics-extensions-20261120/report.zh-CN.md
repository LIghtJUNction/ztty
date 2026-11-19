# Kitty 图形扩展与 OSC 52 整合验收

日期：2026-11-20（Asia/Taipei）  
代码：[05e14c3](https://github.com/LIghtJUNction/ztty/commit/05e14c3c25526514f50bfeed35a463cf0d85d0bf)  
最终 Linux 二进制 SHA-256：`f8f6f218a44c2873c1246651cb474e5e324a7cdb06df8f1062f4494623d8aefe`

本次把已有图形协议分支与 GPU 渲染分支完成整合，并修复了实际像素检查发现的 GPU 占位图裁剪缺陷。Linux X11 的 GPU 和软件渲染路径分别通过 8 组图形检查、13 组剪贴板检查。未发布新版本，也未改动现有终端安装。

## 实现与修正

本次接续 `3c821d5` 与 `12a8e468` 的未完成合并。三个冲突均保留双方行为：依赖锁保留文件传输与字形分段所需依赖；事件循环同时保留图像动画和文字闪烁的唤醒时间；窗口释放同时清理 OSC 52 请求窗口和 GPU 资源。

GPU 原来把 Unicode 占位字符对应的整张图画出来，没有应用每个字符区域的裁剪。现在保留原图的缩放与采样坐标，在提交图像顶点前把字符区域与视口取交集，绘制后恢复视口裁剪。普通图像不增加这一步。修改前，邻格本应为背景，却读到红色图像像素；同一最终检查脚本在修改后的构建上通过。

同时修复 X11 OSC 52 辅助方法、Windows 剪贴板句柄类型的模块可见性，以及当前 Rust Clippy 指出的两处写法。no_std 审计器现在读取依赖实际声明的否定 feature 条件，并确认该 feature 在解析后的依赖图中未启用；没有为 libc 增加跳过项，最终 ELF 中的 Rust std 符号检查继续执行。

GUI 用例也修正了两处测试问题。Kitty 的省略坐标依赖当前左邻占位字符；因此用例先检查省略坐标的继承，再显式写入右格坐标，最后检查擦除左格不会删除右格图像。另一处按实际 python-Xlib 的事件定义，把 SelectionClear 的字段从 selection 改为 atom。[Kitty Unicode 占位符规则](https://sw.kovidgoyal.net/kitty/graphics-protocol/#unicode-placeholders)

## 验证结果

所有 GUI 检查使用自己创建的 Xvfb、PTY、窗口和剪贴板，不操作用户桌面。测试前后均读取窗口的实际渲染器属性；GPU 路径使用 Mesa llvmpipe（LLVM 23.1.1），没有静默回退。

| 检查 | 结果 | 具体覆盖 |
| --- | --- | --- |
| Rust workspace 与文档测试 | 439 通过、6 忽略 | 另有 9 项 SessionCore 实际 PTY 检查通过，测试子进程已回收 |
| 严格 Clippy、格式、release 构建 | 通过 | 锁定依赖；Clippy 启用 `-D warnings` |
| Kitty 扩展 / GPU | 8/8 | 文件/符号链接/偏移、临时文件、共享内存、非法源、压缩源、相对位置、占位字符、动画 |
| Kitty 扩展 / 软件渲染 | 8/8 | 同一组检查 |
| OSC 52 / GPU | 13/13 | UTF-8、primary/clipboard、BEL/ST、INCR、精确 1 MiB、超额拒绝、无效文本、所有权、观察者、取消与超时 |
| OSC 52 / 软件渲染 | 13/13 | 同一组检查 |
| 生产 Rust no_std 审计 | 通过 | 解析后的 feature 图、源码声明与最终 ELF 的 Rust std 符号 |
| 裸机库检查 | 通过 | `zterm` 和 `zgraphics` 的 `x86_64-unknown-none` 类型检查 |
| Windows GNU 类型检查 | 通过 | `x86_64-pc-windows-gnu`；没有原生窗口运行验收 |

1 MiB 的 OSC 52 检查分别验证写入完整原文和读取完整 Base64 回复；超额读取返回空结果，没有返回截断文本。取消用例验证输入控制权变化、用户粘贴优先和两秒查询超时后，都能继续处理新的查询。四个 GUI 报告均记录清理错误列表为空。

## 修复前后的对照

修复前对照为本地合并候选：已经解决三处合并冲突与 X11 方法可见性问题，尚未加入 GPU 占位裁剪。本报告将它与正式版本区分；二进制散列为 `d608d97ba30ab23a5db0bd5bac5bdb73b92c0475c96b157ae4f5f0d6ccedcf19`。

| 同一最终检查脚本 | 修复前候选 | 本次候选 |
| --- | --- | --- |
| 图形扩展 GPU 检查 | 7/8，占位图邻格被污染 | 8/8 |
| 应保持背景的邻格 RGBA | 实际 `[255, 0, 0, 255]` | 预期背景 `[24, 36, 48, 191]` |

这里比较的是功能正确性，没有使用耗时差声称性能提高。原始结果见 [修复前](baseline-gpu.json)、[GPU](graphics-gpu.json)、[软件渲染](graphics-software.json)。[最终占位图截图](placeholder-gpu.png) 已检查，图像只出现在预期字符格和相对位置。

## 复查命令

```sh
cargo fmt --all -- --check
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo build --release --locked
python3 tests/graphics_extensions_gui.py --binary target/release/ztty --renderer gpu --artifacts artifacts/graphics-gpu
python3 tests/graphics_extensions_gui.py --binary target/release/ztty --renderer software --artifacts artifacts/graphics-software
python3 tests/osc52_gui.py --binary target/release/ztty --renderer gpu --artifacts artifacts/osc52-gpu
python3 tests/osc52_gui.py --binary target/release/ztty --renderer software --artifacts artifacts/osc52-software
python3 tools/check_no_std.py
cargo check --target x86_64-pc-windows-gnu --locked
cargo check --target x86_64-unknown-none --locked -p zterm -p zgraphics
```

[验证清单](validation.json) 记录各命令退出状态、源码散列和二进制散列。运行使用本工作树独占的 Cargo target 目录。Windows 可见性修正只影响 Windows 源文件，审计脚本修正不进入终端二进制；已核对完成这些修正后 Linux 二进制散列不变。

## 当前边界

Kitty 文件和共享内存的 host reader 当前只接入 Linux。macOS 与 Windows 有 OSC 52 bridge 源码，但本轮只有 Windows 编译检查，没有两者的原生剪贴板运行证据。原生 Wayland 的接入与验证由后续整合完成。

本轮验证 GPU API 渲染路径在 llvmpipe 上的功能，不代表物理 GPU 性能、显示器呈现延迟或所有驱动兼容性。工作区默认忽略的六项 GPU 基础测试没有计入 439 项通过；实际 GPU 窗口与像素检查由上表的 GUI 套件提供。Kitty 动画帧协议已经覆盖；APNG 解码仍维持原有不支持边界。
