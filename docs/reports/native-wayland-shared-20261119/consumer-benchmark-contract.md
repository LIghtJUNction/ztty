# ztty 渲染与输入消费者基准合同

## 当前依赖事实

本记录随原生 Wayland 共享会话补丁交付。该候选尚未消费 zgpui，因此**没有 zgpui 消费 SHA**。不能用 zgpui 上游 HEAD 代替不存在的集成证据。具体生产源码和二进制绑定见同目录验收报告；这里不发布新的性能排名。

当前 Cargo workspace 中，`crates/zpty` 是本地 path 依赖，版本为 0.2.1；`crates/zgraphics` 仍为 workspace 成员，由终端核心消费。发布整合可以调整包元数据，但这不证明已切换到独立仓库或新渲染库。当前没有 `.gitmodules`，渲染直接调用本仓库 `ztty::gpu::{Display, Renderer, GridStyle}`。

| 层 | 当前实际工作 | 对照时必须保持的内容 |
| --- | --- | --- |
| zpty / Linux broker | PTY 创建、读写、窗口大小、生命周期、控制权 | 同一字节流、终端大小、输出量；不能跳过 PTY 来冒充完整终端 |
| zterm / SessionCore | 转义序列解析、网格、历史、重排、grapheme | 相同历史限额、最终网格与复制文本 |
| zgraphics | 图像解码、位置、动画与协议状态 | 相同像素、裁剪、动画帧与内存预算 |
| ztty::gpu | EGL/GLES、HarfBuzz/FreeType、字形 atlas、纹理、完整帧呈现 | 相同字体文件、DPI、fallback、着色器、alpha、GPU 工作 |
| X11 / Wayland 窗口 | 原生事件、帧回调、焦点、剪贴板与 IME | 真正映射的窗口、真实协议输入、相同分辨率与缩放 |

## 可复用场景与入口

下列脚本已存在。参数中的 `CANDIDATE`、`BEFORE`、`AFTER` 是独立冻结的 ELF 路径；执行前必须记录其 SHA-256 和对应源码，不允许指向正在重建的 target 文件。

| 场景 | 现有入口 | 证据边界 |
| --- | --- | --- |
| 原生 Wayland、共享 PTY、双前端启动顺序、真实输入法 | `tests/wayland_gui.py` | 私有 Sway 截图、PTY 字节和进程身份；正确性验收 |
| 文本、emoji、组合字符、斜体、alpha、闪烁、字体重载 | `tests/text_gpu_gui.py` | 私有 Xvfb 像素与真实复制；正确性验收 |
| Kitty 图像、动画和裁剪 | `tests/graphics_extensions_gui.py` | 真实窗口图像；正确性验收 |
| 物理小键盘、修饰键、Kitty 键盘、搜索/面板输入 | `tests/input_gui.py` | XTEST 到 PTY 的实际字节，含物理键码验证 |
| 1/10 窗口、历史、图像、共享与独立进程 | `tools/benchmark_terminals.py` | 终端资源与场景记录；不是 zgpui/gpui 库级对照 |
| 成对旧/新终端吞吐 | `tools/benchmark_throughput.py` | 固定终端工作负载；不把处理完成当作屏幕呈现完成 |
| ASCII/颜色/Unicode 日志、进度条、原位更新 | `tools/benchmark_output_pipeline.py` | DSR 回包界定逻辑处理完成；不能证明 GPU 或用户看到画面的延迟 |

在仓库根目录执行的现有 CLI：

```sh
python3 tests/wayland_gui.py --binary "$CANDIDATE" --output artifacts/consumer-wayland
python3 tests/text_gpu_gui.py --binary "$CANDIDATE" --renderer gpu \
  --output artifacts/consumer-text.json --screenshots artifacts/consumer-text
python3 tests/graphics_extensions_gui.py --binary "$CANDIDATE" --renderer gpu \
  --artifacts artifacts/consumer-graphics
python3 tests/input_gui.py --binary "$CANDIDATE" --source-root "$SOURCE_ROOT" \
  --renderer gpu --output artifacts/consumer-input.json --screenshot artifacts/consumer-input.png
python3 tools/benchmark_throughput.py --before "$BEFORE" --after "$AFTER" \
  --counts 1,10 --rounds 3 --json artifacts/consumer-throughput.json
python3 tools/benchmark_output_pipeline.py --binary "$CANDIDATE" \
  --binary-source "$SOURCE_COMMIT" --modes headless,window --mib 8 --samples 3 \
  --broker-cpu "$BROKER_CPU" --writer-cpu "$WRITER_CPU" --xserver-cpu "$XSERVER_CPU" \
  --output artifacts/consumer-output.json
python3 tools/benchmark_terminals.py --ztty "$CANDIDATE" --apps ztty,kitty,ghostty \
  --counts 1,10 --modes shared,independent --workloads idle,history,images \
  --repetitions 3 --json artifacts/consumer-terminals.json \
  --svg artifacts/consumer-terminals.svg --no-readme
```

`--renderer` 是这些测试脚本的参数；应用自身使用配置键 `renderer=auto|gpu|software`。Wayland GUI 脚本自动构建固定协议版本的输入法 peer。它使用私有 headless Sway、pixman 合成和 Mesa 软件 GL，用来检查真实协议及绘制正确性，**不能据此宣称硬件 GPU 比 gpui 更快**。

## zgpui / gpui 对照必须遵守的合同

对照应在同一机器、同一 GPU/驱动、同一字体及图片、同一窗口尺寸和缩放下执行。先检查结果像素、最终文本和输入字节等价，再比较冷启动、暖缓存帧耗时、输入到呈现的延迟、空闲 CPU、CPU PSS、GPU 内存与长时间创建/销毁后的资源数量。冷热缓存应分别报告，运行次序采用配对交替，保存原始样本与背景负载。

GPU 完成和真正呈现必须有对应时间边界。已有 `tests/egl_present_trace.c` 可观察实际 EGL swap 调用，但 swap 被调用并不等于 GPU 已完成或显示器已呈现；DSR 回复也不具有这个含义。没有合适的呈现时间证据时，只报告已测到的提交/逻辑处理时间。

不得通过关闭 GPU、少画字符、丢动画帧、减少图片、删除 fallback/IME、降低分辨率或把功能缺失计作速度优势。现有脚本是消费者场景与正确性依据，尚不是 zgpui/gpui 的完整适配器。是否“全面超越”必须逐项由同任务实测决定，不能预先写入结论。
