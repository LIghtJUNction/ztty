# 9f5a77a 宿主性能检查点

**这次测量完成了三组采集，也确认了需要继续处理的回归。** 与固定的 0.4.1 基线相比，
9f5 的 Auto 路径在 1 / 10 窗口下，逻辑输出吞吐中位数分别下降 24.88% / 29.45%；
应用 PSS 从 13.45 / 18.98 MiB 增至 123.79 / 148.41 MiB。
这些是此候选、此负载和此机器的结果；本页没有把它们改写成最终发行版本的表现。

本检查点固定应用生产源码
[9f5a77a1299093d0ba609bfafa2cec3b0ba931dd](https://github.com/LIghtJUNction/ztty/commit/9f5a77a1299093d0ba609bfafa2cec3b0ba931dd)，
内含已发布 zterm 0.5.1 / a2074ba。
采集器固定为
[3cace1d18446d1cc63dc84d635955e5a2b213a0b](https://github.com/LIghtJUNction/ztty/commit/3cace1d18446d1cc63dc84d635955e5a2b213a0b)。
[生产输入对照](production-equivalence.json)记录这两个 Git 对象在应用源码、native/build、Cargo 与 crates 范围没有差异。
[功能报告](../../reports/host-9f5a77a-20261120/README.md)的 472+6+365 与这里的性能观测分别成立。

## 样本与身份

2026-11-20 02:52:36–02:55:33 UTC，三阶段依次执行，没有阶段内自动重试。
[cohort 回执](attempt2/cohort-receipt.json)记录三阶段 exit 0：
paired 33.493 s、output 6.478 s、terminals 136.775 s。
这些时长是整段采集的耗时，不是单次终端吞吐的计时区间。

| 阶段 | 实际范围 | 保存的计时数据 |
| --- | --- | --- |
| paired | 新旧 ztty × 1/10 窗口 × 3 轮，共 12 runs | 198 个输出 burst；19,800 个 DSR 往返值 |
| output | 3 种负载 × headless/software window，共 6 组 | 18 个约 8 MiB 输出样本；另有 1,200 个 idle DSR 值 |
| terminals | ztty/Ghostty/Kitty × 1/10 窗口 × 3 轮，共 18 runs | 297 个输出 burst；29,700 个 DSR 往返值 |

[SAMPLES.md](SAMPLES.md)逐 run 列出未舍入的主要结果；[burst-samples.csv](burst-samples.csv)
保存全部 513 个输出 burst 的顺序、字节数、原始耗时和速率。
完整 JSON 还保留所有往返值、进程与内存快照、压力、退出、清理和错误字段。

| 制品 | 版本 / SHA-256 |
| --- | --- |
| 旧 ztty，source 155fea51aa138b0503ea1d80be4252aacb2c61a7 | 0.4.1；9f58c1a679ccca62cffd09d9991fbbb9c35fff4254f7d64dfc5eedaa6a2bb704 |
| 新 ztty，source 9f5a77a | 0.5.0；c7935e9e76476a9cd7d0186f23f5806e3e2e2fc556a494b0e3741a2c893b3807 |
| Ghostty | 1.4.0+omarchy.1；ef28bf3a0f6ead69014da86ab0c9a2ef30fcf3bf297a6a27c4c18dfc80a56a62 |
| Kitty | 0.50.0；e3aec26698a05b4c024a672e92365649a8fdeff8e884e432f345b14d54e20dd7 |

## 这里测量的是什么

生产者写入 PTY 后，等待终端返回光标位置报告（DSR/CPR）。它说明字节已被逻辑处理到指定位置；
计时终点不是显示器呈现，也不包含物理键盘到屏幕的延迟。
paired 与 terminals 每窗口先预热，再测三个 1,050,937 B burst；先取窗口中位数，再取 run 和三轮中位数。
同时只激活一个 PTY 生产者，因此 10 窗口结果不是十个并发写入者的总带宽。

PSS 将共享内存页按共享进程分摊。应用 PSS 汇总被标记的终端进程及其 helper/等待 launcher，
排除测试 probe 子树和独立 Xvfb/D-Bus；X server 在单独列中保留。
每 run 在固定 UTF-8/颜色画面与字体预热后取三次快照。进程是新建的，系统页缓存与字体缓存没有清空。

机器为 i9-12900HX、24 个逻辑 CPU、Linux 7.3.4-arch1-1，governor 为 powersave。
采集入口实际继承 nice -4 与 0–23 affinity；paired/terminals 没有套用核心微基准的单 CPU 23 约束。
output 单独把 broker/writer/Xserver 绑到 CPU 23/22/21。
执行前后记录的 leaf 没有 cpu.max 字段，有该字段的父级均为 `max 100000`；不能把构建时的 1 CPU/768 MiB 限额移植到这里。
各阶段 host guard 均通过，原始 load、可用内存、swap/PSI 仍保留；机器上的无关工作没有被人为终止。

## 一、新旧配对：Auto 路径的回归

旧版不支持 renderer 配置，因此使用其 application-default，实际 renderer 身份不可读取。
新版请求 Auto，实际窗口属性为 software，但 driver 字段已记录 llvmpipe。
这组观测可以确认资源与吞吐变化；仅凭 driver 字符串不能把全部增量归因到某一组件。

| 窗口 | 旧版 MiB/s，中位数 [范围] | 新版 MiB/s，中位数 [范围] | 中位数之比 | 配对比值中位数 |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 296.687 [274.567, 310.440] | 222.880 [183.026, 236.915] | 0.751232 | 0.717950 |
| 10 | 286.295 [271.539, 301.560] | 201.982 [199.549, 213.611] | 0.705502 | 0.705502 |

“中位数之比”是新旧各自三轮中位数的比值；“配对比值中位数”先对同轮新旧相除再取中位数。
两者都保留，避免用不同统计量隐藏波动。

| 窗口 | 旧 → 新应用 PSS MiB | 旧 → 新 DSR p50 µs | 旧 → 新 DSR p95 µs |
| ---: | ---: | ---: | ---: |
| 1 | 13.45 → 123.79 | 13.6950 → 8.3540 | 16.464 → 20.832 |
| 10 | 18.98 → 148.41 | 13.0850 → 15.0075 | 24.485 → 25.022 |

新候选 1 / 10 窗口应用 PSS 分别增加约 110.35 / 129.43 MiB。
这份检查点暴露的问题由宿主渲染路径继续调查；后续修复与复验应使用新的源码和独立样本。
原始 [paired.json.gz](attempt2/throughput/paired.json.gz)与首次失败均保留。

### 随后的独立内存诊断

[diagnostic.json](diagnostic/diagnostic.json)在同一 9f ELF、collector 6885caf 上各运行一次 Auto 与 software。
应用 PSS 分别为 112,327,680 B 与 12,172,288 B，相差 100,155,392 B（95.52 MiB）。
Auto 最终仍为 software，但自有进程 maps 中保留 LLVM/Gallium/NVIDIA vendor 映射；
明确 software 没有这些映射。前后两个时点的 maps、smaps_rollup、按文件聚合与原脚本全部保留，
source/ELF 一致，剩余自有 PID 为空。

这支持继续调查 GPU 探测后回退的资源成本，尚不把全部差额归给某一个动态库。
诊断只各一 run，运行在另一个受限 unit；其偶然产生的计时没有加入上述三轮、513 行 CSV 或吞吐结论。
修复与新的完整采集使用独立源码和记录。

## 二、输出管线：headless 与明确 software window

六组都使用新 9f5 ELF，每组 3 个约 8 MiB 样本。window 模式在计时前后校验实际 renderer 为 software；
headless 没有终端窗口。两种模式都保留完整可见文本检查。
它们没有旧版对照，不能用于量化版本改进；也不与核心处理函数的 MiB/s 合并。

| 负载/模式 | 三次 MiB/s（原顺序） | 中位数 MiB/s | idle DSR p50 / p95 µs |
| --- | --- | ---: | ---: |
| ascii-log-headless | 132.132, 133.176, 132.976 | 132.976 | 17.295 / 20.848 |
| ascii-log-window | 90.660, 101.645, 103.265 | 101.645 | 32.716 / 34.310 |
| inplace-ascii-headless | 183.908, 192.068, 193.384 | 192.068 | 17.579 / 18.933 |
| inplace-ascii-window | 118.704, 143.160, 140.517 | 140.517 | 31.233 / 34.622 |
| unicode-log-headless | 25.970, 26.675, 26.520 | 26.520 | 16.775 / 18.760 |
| unicode-log-window | 24.416, 24.455, 24.183 | 24.416 | 20.317 / 24.982 |

同组全部三个耗时/速率及 idle DSR 原值在 [output 原件](attempt2/output/)。
CPU 时间按内核 10 ms tick 计数，原始值保留，不把它当作纳秒级 CPU 测量精度。

## 三、三应用矩阵：独立的显式 GPU 设置

该矩阵只覆盖共享模式、固定 idle 画面、1/10 窗口；没有测试 history、images、hidden 或独立进程模式。
ztty 明确请求 GPU，实际走 llvmpipe 软件 OpenGL；这与上面的 Auto→software 配对不是同一条路径。
Ghostty 日志只给出 Mesa/OpenGL 4.6，Kitty 没有实际 driver 记录。
因此不能宣称三者使用同一物理 GPU/同一 driver，更不能把此机器的一小组结果推广成通用排名。

| 应用 | 窗口 | 应用 PSS MiB | 应用 + 完整 X server PSS MiB | MiB/s，中位数 [范围] | DSR p50 / p95 µs |
| --- | ---: | ---: | ---: | ---: | ---: |
| ghostty | 1 | 319.72 | 404.54 | 152.880 [132.529, 251.217] | 60.0450 / 644.210 |
| kitty | 1 | 179.94 | 263.14 | 56.875 [52.597, 60.882] | 4946.8665 / 9628.259 |
| ztty | 1 | 140.09 | 225.59 | 136.008 [96.380, 165.607] | 10.5415 / 11.708 |
| ghostty | 10 | 817.45 | 928.17 | 191.813 [125.694, 198.678] | 33.1970 / 118.409 |
| kitty | 10 | 359.42 | 455.90 | 55.460 [49.545, 58.976] | 5085.0900 / 7917.933 |
| ztty | 10 | 330.67 | 445.64 | 116.453 [106.338, 144.857] | 18.7125 / 82.806 |

三应用各有 33 次窗口运行。ztty 的 33 次具有完整 32×120 文本检查；
Ghostty 33 次和 Kitty 33 次保留 CPR 与 payload hash，没有完整屏幕文本 oracle。
加上 paired 的 66 次 ztty 窗口运行，本报告总共 99 次完整文本检查、66 次 CPR/hash 范围检查。
不能把 99 写成三应用矩阵单独的 ztty 数量。

完整 [terminals.json.gz](attempt2/terminals/terminals.json.gz)保存三轮和区间；
[原始图](attempt2/terminals/terminals.svg)与[规模图](attempt2/terminals/terminals-scaling.svg)沿用该矩阵的数据。
应用与专用 X server 的 sampled swap 均记录为 0；resident PSS 与 swap 分列，未相加成唯一物理内存总量。

## 首次失败与采集器修正

[attempt1](attempt1/cohort-receipt.json)在 throughput 阶段 exit 1，后两阶段没有启动。
固定旧版拒绝新增的 `renderer = auto` 配置，随后 probe hello 等待超时。
原 paired 记录包含一次失败的启动尝试，**没有完成的计时样本，新 ELF 尚未启动**。
它不是 0 MiB/s 样本，也不是新二进制的性能结论。

[诊断记录](collector/legacy-baseline-compatibility.json)保留 stderr、原始 SHA 与兼容开关
`--before-application-default-renderer`。attempt2 显式让旧版用应用默认配置，
仍检查原 source 与 ELF；新版继续要求 Auto 身份。旧失败文件与旧采集器源码没有覆盖。
诊断文件中“no retry performed”属于其写入时点，随后的 attempt2 由自己的 launch/cohort 单独记录。

## 证据与离线复核

| 原件 | 原 SHA-256 |
| --- | --- |
| attempt2/cohort-receipt.json | b82436a9124402d4edf601b136945ba7accbf7d9cc93c8b6d9a7bcc82a867de9 |
| attempt2/throughput/paired.json.gz | 9a6be71703a344ab669218d96fb8cbbefea5b0dd8f66b4781edcd9bc5d52907f |
| attempt2/output/receipt.json | a7c9bd550dc398806062a83cbc13609f7bb8b83b4290151e94c69096d0c24b13 |
| attempt2/terminals/terminals.json.gz | 1bb5cde43e8b47887729d8c4fa890b3b699c86287994e5d78c5857d84aa02953 |
| attempt1/throughput/paired.json | cd41cf005586bbba8124b6322b2dded6406e22b3b9b3246100dc06cded576f81 |
| collector/run-final-performance.py | 2a0e6f52d43404c8d2764d87ea14a71aea7e0d15c544bdf192f5b585ce961a2c |
| diagnostic/diagnostic.json | 87e009fe4ecf3c4364b599857014a737c2a14feb01c02951486a499f1e636a2e |

```sh
python3 docs/benchmarks/host-9f5a77a-20261120/verify-record.py
```

Python 3.11+ 脚本只读公开附件，核对所有 SHA、来源关系、失败时点、513 个输出 burst 的算术、
DSR 原值与中位数、身份/清理/guard、表格及 CSV；不会启动终端、编译或重新测量。
完整入口命令在每组 receipt 与 launch 的 command 数组中；固定采集器在 [collector](collector/)。
重现实验需准备这些固定源码与二进制，并把 WORKTREE 占位换成对应机器的路径。
外部应用、字体、显示服务与主机压力也需按原件记录，不能只复制一个吞吐数字。

[redaction.json](redaction.json)逐文件保存原 SHA、公开 SHA 和路径变化字段。
只替换私有绝对路径前缀；数字、顺序、失败、负载与资源原值保留。
两个大 JSON 使用无损 gzip，压缩文件和解压内容分别核验。
原始 runs 的规范化摘要在脱敏前校验；公开副本另存规范化摘要，未冒称两者相同。

这份宿主检查点与[此前的核心微基准](../resume-2026-11-20.md)使用不同负载和计时边界。
核心 a207 的部分恢复、9f 的功能通过与这里的资源/吞吐回归需要同时保留。
