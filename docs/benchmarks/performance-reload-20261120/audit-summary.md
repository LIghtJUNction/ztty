# ztty 固定性能批次独立审计

审计范围为旧版 `755c2c01c2a3eb535088a6f06d1655aaef97d931` 到集成版本 `f7e019654684ceae4b0cb64d47d651ebed99bc35`。此结论只对应原始记录中的两个 ELF；后续剪贴板修改不纳入本批性能身份。

## 结论与证据

原始样本、摘要、ELF 身份、固定 GitHub collector 源码、renderer、内存范围和清理记录核对通过，没有删除偏慢样本。A/B 有 12 个案例，三应用有 18 个案例，固定 CPU pipeline 有 36 个案例。独立复算执行了 9,614 项离线检查；它没有启动应用，也不替代原生验收。[完整审计](sandbox:/workspace/scratch/3dc04f47346d/final-evidence/audit-result-complete.json)

测得的改善集中在软件窗口输出和 Unicode 日志。无窗口 ASCII 原地更新仍有负向结果；单窗口三应用中位数也不足以证明稳定快于 Ghostty。根报告的 A/B、pipeline、三应用、五轮 Unicode/ASCII 微基准以及有效 ASCII 硬件计数器百分比均与原始数据相符。[报告复核](sandbox:/workspace/scratch/3dc04f47346d/final-evidence/audit-report-review.json)

## 固定 CPU pipeline

每轮先计算 final/before，再取三个比值的中位数。两个吞吐列各为三轮中位数，因此不能直接用两列相除来重算最后一列。broker、writer、X server 亲和性分别是 CPU 23、22、21；18 个窗口全部实际使用 software，尺寸均为 1112×800。[逐项审计](sandbox:/workspace/scratch/3dc04f47346d/final-evidence/audit-pipeline.json)

| 负载 | 模式 | 旧吞吐 MiB/s | 新吞吐 MiB/s | 配对中位变化 | 新版较快轮数 |
| --- | --- | ---: | ---: | ---: | ---: |
| ASCII 滚屏日志 | 无窗口 | 130.61 | 131.79 | +2.73% | 2/3 |
| ASCII 滚屏日志 | 软件窗口 | 99.48 | 123.02 | +19.42% | 3/3 |
| Unicode 滚屏日志 | 无窗口 | 26.65 | 39.89 | +44.68% | 3/3 |
| Unicode 滚屏日志 | 软件窗口 | 24.51 | 37.28 | +53.42% | 3/3 |
| ASCII 原地更新 | 无窗口 | 196.50 | 186.39 | **−3.68%** | 1/3 |
| ASCII 原地更新 | 软件窗口 | 138.40 | 163.99 | +17.78% | 3/3 |

每种负载的输入字节数、输入 SHA、目标光标和最终可见文本指纹在其 12 次运行中一致。collector 在计时后实际读取全可见屏幕并断言相等，才记录通过；JSON 保存预期指纹与校验结果，没有保存整个屏幕原文。2,000 行是历史容量设置，此 collector 没有导出并逐字节验证完整 scrollback。[校验方式源码](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/tools/benchmark_output_pipeline.py)

## 软件 A/B 与三应用分开解释

A/B 的全部窗口是 `auto → software`。单窗口三轮吞吐中位数为 171.73 → 257.50 MiB/s（+49.94%），10 窗口为 158.52 → 229.21 MiB/s（+44.60%）。单窗口旧版第二轮的 52.04 MiB/s 原样保留。应用 PSS 分别为 13.49 → 13.72 MiB、22.26 → 22.42 MiB，不支持本轮旧新内存下降的说法。[A/B 原始数据](sandbox:/workspace/scratch/3dc04f47346d/final-evidence/final-throughput-ab.json)

三应用处于强制 Mesa 软件 GL 环境。ztty 明确走 GPU 路径，且逐窗观测到 llvmpipe；Ghostty 日志有 OpenGL/Mesa；Kitty 未独立记录实际 driver。此组 ztty 的单窗口 PSS 为 136.22 MiB，10 窗口为 326.86 MiB，不能拿软件 A/B 的 13–23 MiB 当分母。三应用逻辑网格均为 120×32、字体请求均为 11 pt；实际像素是 ztty 1112×768、Ghostty 1080×640、Kitty 1081×641，并非相同的像素工作量。[三应用原始数据](sandbox:/workspace/scratch/3dc04f47346d/final-evidence/final-terminals.json)

单窗口吞吐中，ztty 三轮为 295.07 / 111.67 / 263.60 MiB/s，Ghostty 为 137.59 / 174.17 / 275.25 MiB/s；按同轮次仅 1/3 更快，范围重叠。10 窗口则是 ztty 145.13 / 158.94 / 150.41，Ghostty 141.81 / 140.11 / 126.77，3/3 更快，但各自中位数之比只有 +7.35%。[复算与逐轮比较](sandbox:/workspace/scratch/3dc04f47346d/final-evidence/audit-result.json)

## 测量边界

- `idle` 描述内存采样前的准备状态。之后每窗仍有 300 次 DSR 与三次约 1 MiB 的 ASCII/彩色原地更新；它不只是空闲测试，也不覆盖高多样 Unicode 吞吐。
- 10 窗口测试每次只有一个生产者输出，其余九个空闲。DSR 说明逻辑处理完成，没有物理呈现栅栏、帧丢失率、硬件 VRAM 或键盘到屏幕延迟数据。
- 应用 PSS 包含已识别的终端进程与等待中的 launcher，排除 PTY probe、私有 Xvfb 和 D-Bus。合并 PSS 加的是完整 Xvfb，而非增量；Swap 单列。
- 应用与 Xvfb 的采样 Swap 均为零，但 A/B 宿主 memory PSI some avg10 为 1.72–6.47%，所有案例的宿主换页计数都有增长。`host_contention=[]` 只说明通过入口准入条件，不表示零压力。
- idle CPU 仅采样 0.5 秒，时钟为 100 Hz。零值不证明长期零 CPU；Ghostty 的样本波动不支持耗电排名。
- pipeline 与另两组 collector 的实际窗口尺寸、输入规模和亲和性不同。只在同一矩阵内比较旧新，不能直接合并跨 collector 的绝对速率。

这些边界来自固定版本的 [benchmark_throughput.py](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/tools/benchmark_throughput.py)、[benchmark_terminals.py](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/tools/benchmark_terminals.py) 与原始 host 快照；没有追加性能实验。

## 结论与证据对应

| 判断 | 证据 | 状态 |
| --- | --- | --- |
| 本批次软件窗口吞吐改善 | A/B、pipeline 原始 bytes/time 与三轮配对结果 | 支持，限测得场景 |
| Unicode 核心收益不只来自重复短句 | diverse 五轮原始 ns、payload 和语义指纹 | 支持，仍是单机器核心实验 |
| 所有 ASCII 路径都无退化 | headless inplace 两轮负向；早期核心 −7.2%/−9.2% | 不支持 |
| 单窗口稳定快于 Ghostty | 同轮次仅 1/3 更快，范围重叠 | 不支持 |
| 真实 GPU/桌面已全面领先 | 未测物理呈现和硬件 GPU | 无证据 |

核心微基准的输入、原始速率与语义指纹已额外复核，摘要一致。[五轮原始记录](sandbox:/workspace/scratch/3dc04f47346d/final-evidence/cache-paired.json)、[核心汇总](sandbox:/workspace/scratch/3dc04f47346d/final-evidence/cache-summary.json)

## 审稿式自查

- 贡献：软件窗口与 Unicode 负载有量化改善，没有扩写为全面领先。
- 表达：renderer、PSS 范围、单位和配对统计公式分开说明。
- 实验强度：保留所有样本、异常波动和负向结果。
- 覆盖范围：真实桌面、硬件 GPU、同时输出生产者等缺口已写明。
- 方法：逻辑处理、最终文本一致和像素呈现属于不同证据层，未混为一项。
