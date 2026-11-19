# 原生 Wayland 共享会话验收

## 结果与固定来源

原生 Wayland 现在通过 Linux 的同一个 broker、SessionCore 和 PTY 管理路径运行。多个原生窗口各有自己的 PTY；分离后重新附着同一会话时保留原 PTY。MCP 与本地窗口仍使用原有控制权检查。X11 和 Wayland 可以在同一个 broker 中并存，选择前端不会被第一个窗口的类型覆盖。

生产提交：[8b9f566c15ec3fb1020c4e2d96d41138c8cd2293](https://github.com/LIghtJUNction/ztty/commit/8b9f566c15ec3fb1020c4e2d96d41138c8cd2293)。其父提交 e37d6de 已合入图形协议与 OSC52 的 1db3cfda；保留了原生基础 8fc64c25、GPU 12a8e468 和重排 05a418af 的历史。此次不改发布版本、不安装应用、不重启现有会话、不触发 CI。

冻结 ELF 为 **1,227,648 字节**，SHA-256：

```text
7fbaf9942d82c168bc08c54a472ccda8acd81c24f31d65558ebc48771a4dee74
```

[validation.json](validation.json) 保存脱敏结果，[source-sha256.json](source-sha256.json) 保存生产文件清单。构建前后文件摘要一致，且逐文件确认与上述生产提交完全相符。最后一次测试后只整理了 Python GUI 夹具末尾一个多余空行，并修正其使用私有 Xvfb 的说明；原测试摘要、提交摘要和这些非执行文本差异均保留在证据中，生产源码及 ELF 没有变化。固定的上游输入法协议 XML 保留原始字节及 MIT 许可，包括其原有尾随空格。

## 改动解决了什么

早期原生窗口使用独立 PTY 路径，不能复用已建立的会话、MCP 控制与输入所有者。本次以显示适配层连接 X11/Wayland 窗口，将原生事件送入既有 UI 策略。创建和附着请求携带前端类型，连接按显示端点及前端区分。窗口资源、帧回调和 EGL 生命周期由各窗口持有，SessionCore 不重复实现。

原生剪贴板和 primary selection 使用异步文件描述符传输。单份文本上限为 4 MiB，所有存活 source 的文本总量为 8 MiB，最多 16 个发送流；一次 pump 的读写工作有界，超时为 2 秒。手动粘贴和 OSC52 继续经过原策略，失败或超限时不注入半份文本；取消使用 generation 防止旧结果进入新请求。

容量检查曾在替换旧 source 前拒绝新分配，导致两份 4 MiB selection 占满预算后连 1 字节替换也无法成功。修复只回收当前请求要替换且没有进行中传输引用的旧 source。有 writer 引用时保留其不可变快照并拒绝超预算复制，仍严格保持总上限。真实分配与引用计数代码通过了 ASan/UBSan 回归。

原生输入法使用 text-input-v3，终端用途、焦点、光标矩形和可见预编辑都走 Wayland 协议。提交以 UTF-8 边界分块送入原输入路径。终端不拥有 shell 的编辑缓冲，因此不伪造 surrounding text，也不把 delete-surrounding 请求转换成任意退格；无效删除批次被拒绝，后续正常输入仍可继续。

## 实际验收

| 门禁 | 结果 | 实际覆盖 |
| --- | --- | --- |
| Wayland 私有 GUI | 65/65 | 原生 EGL 像素、标题、4 次尺寸变更、按键/修饰键/重复/Kitty 输入、历史与同步输出、共享 PTY/MCP 控制、剪贴板、IME、附着、关闭及断线 |
| 混合前端顺序 | 含在上述 65 项 | 两个新私有 socket 分别验证 X11→Wayland、Wayland→X11；真实可见 X11 窗与原生 toplevel、相同 broker PID、不同 PTY PID |
| X11 普通剪贴板 | 25/25 | 直接/INCR、UTF-8 边界、STRING 回退、4 MiB 限额、错误后恢复 |
| X11 GPU 输入 | 198 向量全过 | 另有 14 UI 组、14 zoom 检查和 4 keypad overlay 检查；物理小键盘与 NumLock/Shift 路径保留 |
| 原生 source 容量/生命周期 | 5 组全过 | 满预算替换、超限拒绝、活跃 writer 保留、取消后 writer 生命周期、最终记账归零；ASan/UBSan |
| 编译门禁 | 全过 | Wayland 与默认 X11 严格 Clippy、release 构建、源码冻结核对 |

这些计数按各脚本原口径列出，不把 UI 分组与其输入向量重复累加为独立测试总数。

Wayland 使用私有 headless Sway 1.12，pixman 合成及 Mesa 软件 EGL。输入法 peer 是实际 input-method-v2 客户端，经 compositor 转发到 ztty 的 text-input-v3；已验证可见预编辑不进入 PTY、中文与 emoji 提交精确一次、长 UTF-8 提交跨分块不损坏、拒绝不拥有的 shell 删除后继续输入。X11 输入回归强制 GPU 路径，报告实际驱动为 llvmpipe。它们证明协议和 GLES 绘制正确性，不代表物理显示器延迟或硬件 GPU 性能排名。 [Sway 1.9 官方源码](https://github.com/swaywm/sway/blob/1.9/sway/server.c) 已注册本次所用的 text-input-v3、input-method-v2、primary-selection、screencopy 和 data-control 等协议；但本次没有实际运行 Sway 1.9，能力核对不能代替 Ubuntu 上的运行验收。

所有自有 broker、PTY 子进程、剪贴板/IME/MCP helper 均已回收；Xvfb 显示所有权通过 PID 锁与内核 socket 身份核对，键盘映射已恢复。没有向宿主桌面注入按键，没有读取用户的真实剪贴板。原始日志、截图和运行时身份只留在私有验收目录，公开结果保留其摘要。

### 过程中发现的真实边界

Primary selection 的所有权必须使用 compositor 已发出的有效输入 serial。另一个应用已取得更新的 selection serial 后，旧 serial 的声明会被 Sway 拒绝；实际日志明确报告 superseded serial。新的真实 Shift 按下/释放后可正常复制。实现没有伪造 serial，也没有增加 data-control 权限来绕过这一规则。

两个夹具问题有单独失败记录：新 wl-copy 尚未就绪时的首个 wl-paste 可能仍连到暂停的旧 owner；现在仅重试就绪探测，成功内容仍逐字节核对。一次性 wtype 退出后私有 seat 没有键盘；IME 验收现在保留一个自有、空闲的虚拟键盘，接近真实桌面的常驻键盘状态。OSC52 查询的 BEL 结束符按实际请求保留，夹具不再错误地要求 ST。

## 使用与重现

Linux 构建仍显式开启可选特性，默认运行前端仍为 X11：

```sh
cargo build --release --features wayland --bin ztty
target/release/ztty --backend wayland --name dev
target/release/ztty attach --backend wayland --observe dev
target/release/ztty attach --backend wayland dev
```

应用使用配置键 `renderer=auto|gpu|software`，没有独立 `--renderer` 命令行参数。原生 Wayland 仅提供 EGL/GLES 路径；`renderer=auto` 和 `renderer=gpu` 可用，`renderer=software` 会明确报错。Mesa 软件 GL 可以运行该路径，但它仍是 GLES 渲染，并非 Xft 后端。

新增的 Ubuntu/Debian 构建依赖为 `libwayland-dev libwayland-bin wayland-protocols libxkbcommon-dev`；EGL/GLES、HarfBuzz、FreeType、Fontconfig 和现有 X11 依赖沿用主工程要求。GUI 验收还需要 `sway wtype grim wl-clipboard xvfb python3-pil python3-xlib`，以及 C 编译器和 pkg-config。

在仓库根目录运行：

```sh
python3 tests/wayland_gui.py --binary "$CANDIDATE" --output artifacts/wayland-acceptance
python3 tests/wayland_services.py --sanitize --output artifacts/wayland-source-budget
python3 tests/clipboard_transfer_gui.py --binary "$CANDIDATE" --output artifacts/x11-clipboard.json
python3 tests/input_gui.py --binary "$CANDIDATE" --source-root "$SOURCE_ROOT" --renderer gpu \
  --output artifacts/x11-input.json --screenshot artifacts/x11-input.png
```

Wayland GUI 入口自动构建固定协议版本的 IME peer。若使用非系统 Sway，增加 `--sway /absolute/path/to/sway`；默认从同目录查找 swaymsg，也可显式传 `--swaymsg`。GUI 夹具自己创建私有运行目录、显示服务和随机 broker socket，不需要设置宿主 DISPLAY。

## 尚未覆盖的范围

原生 Wayland 继续作为可选前端。窗口内 tab/split/workspace container 明确拒绝，应使用独立会话窗口；不提供 X11 子窗口嵌入。每个原生窗口仍持有自己的 Wayland/EGL 资源，不宣称跨窗共享字形或纹理缓存。程序请求置前仍服从 compositor，尚未提供 xdg-activation token 流程。

分数缩放、跨显示器缩放变化、物理 GPU 呈现延迟以及不同输入法实现/桌面组合尚未形成此候选的验收矩阵。此次 IME 证据来自私有协议 peer，不宣称已经测试所有 fcitx/IBus 配置。macOS/Windows 不属于本次 Linux GUI 验收。

[消费者基准合同](consumer-benchmark-contract.md) 给出实际依赖、可复用场景和精确脚本入口。该固定候选尚未消费 zgpui，没有 zgpui 消费 SHA；不能把上游 SHA 当成已集成版本，也不能从 DSR 吞吐推导 GPU 呈现性能。

## 主张与证据核对

| 主张 | 证据 | 状态 |
| --- | --- | --- |
| 同一 broker 按请求打开正确原生前端 | 双启动顺序、真实窗口、broker/PTY 身份检查 | 已支持 |
| IME 提交不损坏 UTF-8，不修改未拥有的 shell 缓冲 | 实际 input-method-v2 peer 与 PTY 字节断言 | 已支持 |
| Clipboard 满预算仍可安全替换，无在途 source 提前释放 | 真实 C 分配/引用路径的 ASan/UBSan 回归 | 已支持 |
| 显示适配层保留已覆盖的 X11 行为 | 25 clipboard 与 198 输入向量/相关 UI 门禁 | 已支持 |
| 在所有桌面或硬件性能上优于其他终端/渲染库 | 本报告没有相应对照 | 未声称 |

自审结论：贡献对应原独立 PTY 的可复现缺口；说明给出调用边界与重现命令；实验强度限于实际执行的协议、像素与输入验收；未覆盖环境已列明；资源生命周期与总预算有独立检查。没有把未部署、未测试或仅有设计的部分写成已完成。
