# 2026-11-19：普通日志输出吞吐优化

本轮针对实际 PTY 输出链路，而不是只重复此前的局部滚动微基准。在固定的 Linux/X11 测试中，普通日志吞吐提高约 **14.0%**，彩色日志提高约 **19.8%**，包含中日韩文字和组合字符的日志提高约 **10.0%**。持续原地覆盖的两个控制场景没有获得同样收益：进度输出中位数下降约 **1.3%**，整屏原地更新下降约 **1.7%**。完整样本与范围保留在下文；不能把日志场景的提升推广到所有终端工作负载。

生产改动对应 [94aa9b55709af44cf61ea4b362b06cd37ac20f51](https://github.com/LIghtJUNction/ztty/commit/94aa9b55709af44cf61ea4b362b06cd37ac20f51)，基线为 [db4253054dfad84fb1d5af64efd3d9efc4e048d1](https://github.com/LIghtJUNction/ztty/commit/db4253054dfad84fb1d5af64efd3d9efc4e048d1)。本记录描述已提交、已验证的独立性能候选；没有在这一验证阶段替换用户运行中的终端、安装新包或发布 Release。

## 为什么先改空白行初始化

大量日志经过 PTY、broker、VT 解析器，再更新屏幕和历史记录。屏幕滚动时，ztty 会保留最上面一行、旋转屏幕行号，然后把新出现的底行清成空白。一个 `Cell` 保存字符、组合字符、颜色、宽度、标志和链接，大小为 28 字节；原先逐格 `fill` 的机器代码会为每列重复发出多次标量写入。

在自有隔离 broker 中，gprofng 用户态 CPU 采样显示，ASCII 日志的解析与状态更新占约 **64.6%**，`scroll_up` 连同子调用占约 **43.7%**，其中历史行保存连同复制占约 **18.9%**。Unicode 日志中，解析与状态更新占约 **81.2%**，`put_char` 自身占约 **42.4%**，滚动连同子调用占约 **31.5%**。这些包含关系不能相加；采样占比也不是端到端墙钟时间占比。两组采样都把主要问题指向屏幕状态更新，而非字体绘制。[采样记录](measurements/output-throughput-2026-11-19/profile-summary.json)

新实现只修改 [`Screen::clear_row`](../crates/zterm/src/screen.rs)：短行沿用原来的填充；较长的行先初始化 8 个格子，再把已初始化的前缀分批复制到后面的空位。`split_at_mut` 保证源和目的区间不重叠，每轮复制量不超过已初始化前缀。这样可以使用现有的块复制实现，保留整行所有属性，不需要新增堆分配、空白行缓存或平台专用 SIMD 代码。历史行仍完整保存，PTY 读取配额和事件循环没有调整。

## 同条件前后对照

测试使用 i9-12900HX、Linux 7.3.4-arch1-1、Rust 1.100.0。broker 固定到 CPU 23，写入端固定到 CPU 22，私有 Xvfb 固定到 CPU 21；CPU governor 为 `powersave`。窗口为 120×32 格，历史容量为 2,000 行，字体、光标闪烁和透明度设置保持一致。

每个场景先输出一遍相同内容以填满历史并预热，再测三次约 8 MiB 的输出。共运行五组 A/B，组间交替 AB、BA 顺序。每次计时结束前等待精确的光标位置应答，随后核对最终可见文本和光标；有窗口模式还核对实际 X11 窗口已映射。全部 100 个场景运行都通过这些检查，自有测试进程全部回收。

**计时边界是 PTY 写入到逻辑处理完成的协议应答。** X11 绘制同时运行，但应答不是显示帧栅栏，更不是物理屏幕的显示完成时间。无窗口模式用于观察绘制之外的路径；两个模式都使用同样的本地会话与输出保留策略。

表中为各次运行中位数的中位数，方括号是五个运行中位数的最小值与最大值，单位 MiB/s；该范围不是置信区间。

| 场景 | 模式 | 基线 MiB/s [范围] | 候选 MiB/s [范围] | 变化 |
|---|---|---:|---:|---:|
| 普通 ASCII 日志 | 无窗口 | 122.37 [115.80, 123.80] | 138.93 [131.18, 144.28] | +13.5% |
| 普通 ASCII 日志 | X11 窗口 | 116.47 [111.77, 118.84] | 132.77 [125.04, 135.94] | +14.0% |
| 彩色日志 | 无窗口 | 122.01 [113.04, 124.03] | 137.73 [127.00, 142.28] | +12.9% |
| 彩色日志 | X11 窗口 | 106.79 [106.22, 116.75] | 127.95 [118.81, 135.34] | +19.8% |
| Unicode 日志 | 无窗口 | 60.05 [55.18, 65.32] | 69.86 [61.07, 70.91] | +16.3% |
| Unicode 日志 | X11 窗口 | 58.95 [51.28, 61.53] | 64.83 [29.65, 68.36] | +10.0% |
| 原地进度输出 | 无窗口 | 250.02 [215.47, 257.98] | 254.60 [242.84, 259.03] | +1.8% |
| 原地进度输出 | X11 窗口 | 247.87 [186.88, 253.41] | 244.63 [216.78, 253.86] | -1.3% |
| 整屏原地更新 | 无窗口 | 197.74 [181.31, 203.46] | 193.83 [174.55, 198.24] | -2.0% |
| 整屏原地更新 | X11 窗口 | 191.01 [152.54, 193.06] | 187.77 [182.44, 189.63] | -1.7% |

[完整计时样本](measurements/output-throughput-2026-11-19/pipeline-raw.json) · [汇总与各组中位数](measurements/output-throughput-2026-11-19/pipeline-summary.json) · [采集器](../tools/benchmark_output_pipeline.py) · [交替执行脚本](measurements/output-throughput-2026-11-19/run_ab.py)

有窗口 Unicode 场景的一组候选中位数为 **29.65 MiB/s**，明显低于其他组，保留在范围及原始样本中，没有删除。测试暂停了本轮其他构建，但没有把开发机变成独占基准机；这些结果包含实际系统调度和频率变化。

原地覆盖场景的负差异也保留了下来。只读汇编检查确认，候选把 `clear_row` 从内联代码变成独立函数，`scroll_up` 的代码缩短；两版 `Parser::advance` 均为 0x15d3 字节，规范化重定位地址后逐指令一致。这排除了该解析循环新增指令，但不足以证明约 2% 差异的具体原因。本轮结论是日志滚动路径变快，不是所有输出路径都变快。[汇编核查](measurements/output-throughput-2026-11-19/layout-review.json)

## 交互响应与正确性

每次输出计时后另采 200 个空闲 DSR 往返样本。有窗口的五种场景中，将各次运行的 p95 再取五组中位数，基线落在 **21.27–22.61 µs**，候选落在 **21.50–22.39 µs**。这是应用与终端之间的协议响应，不是物理键盘到屏幕的延迟；全部样本保留在正式采集 JSON。

两版分别运行相同的真实 PTY、XTEST 和 X11 像素回归，各 **11/11** 项通过。检查覆盖另一窗口持续输出时的键盘回显和 broker 输入、输出结束后的最终帧、合并脏行、同步输出、Expose、滚动历史与返回实时视图。候选在该次回归中，洪流期间的 XTEST 回显约 **1.38 ms** 可见，RPC 回显约 **1.15 ms** 可见；这些单次观察用于检查响应没有被洪流阻塞，不用于宣称交互延迟改善。两个版本的 350 ms 空闲窗口都没有新增上下文切换。[基线回归](measurements/output-throughput-2026-11-19/repaint-baseline.json) · [候选回归](measurements/output-throughput-2026-11-19/repaint-candidate.json)

核心测试 **114 项**、文档测试 **3 项**、真实 SessionCore PTY 检查 **9 项**全部通过；`cargo fmt --all -- --check`、zterm 全目标 Clippy `-D warnings` 和 release 构建也通过。新增回归覆盖 1–257 列、三种 backing-ring 偏移，逐格检查完整字符属性，并检查相邻行和软换行边界未被改变。[验证记录](measurements/output-throughput-2026-11-19/validation.json)

本轮性能和 GUI 实测范围为 Linux x86_64 与隔离 Xvfb。改动位于共享的 `no_std` 核心，但没有据此宣称 Windows 或 macOS 已完成性能验证。

## 复现与源码绑定

基线 ELF SHA-256 为 `2eda135a24ae4507f6e9ba751fd7b2ddf042e38d1e60cca759a6fdf558d0a7d7`，候选为 `8f35b08477877a798cc2d44ff4431731bdddd418c9c3c72097493c68cc3d2d0c`。每次采集都通过私有 socket 的 `SO_PEERCRED` 核实 broker 身份，再核对 `/proc/<pid>/exe` 的内容哈希，防止测试误连到已安装版本或用户旧会话。

单版复测可在仓库根目录运行：

```sh
CARGO_BUILD_JOBS=1 cargo build --release
python3 tools/benchmark_output_pipeline.py \
  --binary target/release/ztty \
  --binary-source "$(git rev-parse HEAD)" \
  --output artifacts/output-throughput-check.json \
  --mib 8 --samples 3
```

交替脚本使用 `artifacts/output-throughput/baseline/ztty` 作为固定基线；从上述基线提交另建工作树、构建并复制该 ELF 后，可运行：

```sh
python3 docs/measurements/output-throughput-2026-11-19/run_ab.py \
  --candidate target/release/ztty \
  --candidate-source "$(git rev-parse HEAD)" \
  --output artifacts/output-throughput-repeat \
  --pairs 5 --mib 8 --samples 3
```

公开 JSON 保留全部正式计时值和源绑定，仅去掉开发机绝对路径、临时进程标识和重复标准错误输出。原始文件与采样实验保存在开发机 `/home/lightjunction/Documents/GITHUB/ztty/artifacts/output-throughput-20261119-evidence/`，目录内包含 SHA-256 清单，便于复核与后续归档。
