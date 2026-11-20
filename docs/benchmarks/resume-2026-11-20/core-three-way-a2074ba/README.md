# a2074ba：独立三方核心采样

这组九次运行中，候选的 Unicode 吞吐为 **26.101 MiB/s**，比同组已发布版的 **19.330 MiB/s**
高 **35.03%**，但只有旧版 **91.404 MiB/s** 的 **28.56%**。ASCII 则低于同组已发布版 **5.27%**。
结论仍是部分恢复，不能写成性能回退已经解决。

候选是未发布的 zterm 0.5.1，固定在 [a2074bac4c030cfd7c0b2514c852a89dcf54648f](https://github.com/LIghtJUNction/zterm/commit/a2074bac4c030cfd7c0b2514c852a89dcf54648f)。
本组独立于 [85f51c 九次采样](../core-three-way-0.5.1/comparison.json)和[最初六次采样](../core-paired-6053.json)。
不同窗口的基线水平有变化，不从本组和 85f 的数值相减来推导第二项修改的独立收益。

范围仍为解析器与屏幕模型，不含 PTY、窗口、渲染、物理输入延迟或完整应用排名。
[comparison.json](comparison.json)保存全部值、顺序、失败字段和 host guard；
[receipt.json](receipt.json)是执行者回执，[measurements.csv](measurements.csv)提供逐项完整精度数据。

## 本组中位数

A 为旧 zterm 0.4.0，B 为已发布 0.5.0，C 为 a2074ba 候选。每侧有三个进程输出，
表格取其各场景中位数，变化按中位数之比计算；没有跨组拼样本或给出统计显著性。
吞吐单位为 MiB/s；C/A 表示候选达到旧版的比例。

| 场景 | A | B | C | C 相对 B | C / A |
| --- | ---: | ---: | ---: | ---: | ---: |
| ASCII | 243.684 | 215.103 | 203.772 | -5.27% | 83.62% |
| 满历史 ASCII | 140.261 | 112.670 | 114.955 | +2.03% | 81.96% |
| SGR | 133.699 | 112.745 | 122.179 | +8.37% | 91.38% |
| Unicode | 91.404 | 19.330 | 26.101 | +35.03% | 28.56% |

## 全部九次运行

顺序为 A/B/C、B/C/A、C/A/B。九次均 exit 0，stderr 为空，前后压力检查通过，
记录为 `ok=true, status=complete`。进程耗时表保留六位小数，JSON/CSV 保存完整数值。

| 顺序 | 轮次 / 核心 | ASCII | 满历史 ASCII | SGR | Unicode | 进程耗时 s |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| 1 | 1 / A | 305.046 | 169.575 | 173.396 | 111.586 | 2.346019 |
| 2 | 1 / B | 270.457 | 154.178 | 141.343 | 21.103 | 5.514912 |
| 3 | 1 / C | 269.172 | 138.657 | 130.094 | 22.232 | 5.423461 |
| 4 | 2 / B | 198.848 | 92.959 | 91.305 | 19.330 | 6.979460 |
| 5 | 2 / C | 203.772 | 114.955 | 118.767 | 28.405 | 4.861206 |
| 6 | 2 / A | 237.555 | 140.261 | 106.351 | 86.099 | 3.757649 |
| 7 | 3 / C | 201.814 | 108.114 | 122.179 | 26.101 | 5.067307 |
| 8 | 3 / A | 243.684 | 136.290 | 133.699 | 91.404 | 3.347446 |
| 9 | 3 / B | 215.103 | 112.670 | 112.745 | 17.719 | 6.506340 |

屏幕尺寸每次均为 120×32。内存和分配计数没有隐去：候选 Cell 与初始堆仍与 0.5.0 相同。
这些是计数分配器与核心结构的量，不是进程 RSS 或完整应用内存。

| 顺序 | 轮次 / 核心 | Cell B | 初始核心堆 B | 吞吐稳态分配 |
| --- | --- | ---: | ---: | ---: |
| 1 | 1 / A | 28 | 107932 | 0 |
| 2 | 1 / B | 32 | 123292 | 0 |
| 3 | 1 / C | 32 | 123292 | 0 |
| 4 | 2 / B | 32 | 123292 | 0 |
| 5 | 2 / C | 32 | 123292 | 0 |
| 6 | 2 / A | 28 | 107932 | 0 |
| 7 | 3 / C | 32 | 123292 | 0 |
| 8 | 3 / A | 28 | 107932 | 0 |
| 9 | 3 / B | 32 | 123292 | 0 |

内部操作延迟单位为 ns，三元组依次是各进程自己的 p50/p95/p99，没有合并成新的总体分位数。
它们不是按键到屏幕呈现的延迟。

| 顺序 | 轮次 / 核心 | edit p50 / p95 / p99 | edit 分配 | burst p50 / p95 / p99 | burst 分配 |
| --- | --- | ---: | ---: | ---: | ---: |
| 1 | 1 / A | 56 / 60 / 62 | 0 | 2527 / 2754 / 2880 | 0 |
| 2 | 1 / B | 65 / 74 / 77 | 0 | 2926 / 3730 / 3867 | 0 |
| 3 | 1 / C | 87 / 90 / 92 | 0 | 3979 / 4503 / 4914 | 0 |
| 4 | 2 / B | 66 / 71 / 75 | 0 | 4333 / 4716 / 4861 | 0 |
| 5 | 2 / C | 72 / 76 / 79 | 0 | 3355 / 3862 / 4220 | 0 |
| 6 | 2 / A | 74 / 78 / 116 | 0 | 3129 / 3574 / 3982 | 0 |
| 7 | 3 / C | 73 / 78 / 78 | 0 | 3579 / 4144 / 4525 | 0 |
| 8 | 3 / A | 96 / 99 / 104 | 0 | 4514 / 12914 / 14102 | 0 |
| 9 | 3 / B | 83 / 89 / 97 | 0 | 3995 / 4331 / 4533 | 0 |

## 固定来源与方法

| 身份 | A | B | C |
| --- | --- | --- | --- |
| 源码 | ztty [155fea51](https://github.com/LIghtJUNction/ztty/commit/155fea51aa138b0503ea1d80be4252aacb2c61a7) | zterm [13d5e2c7](https://github.com/LIghtJUNction/zterm/commit/13d5e2c74571d35cccd920b1204798211b70c79f) | zterm [a2074bac](https://github.com/LIghtJUNction/zterm/commit/a2074bac4c030cfd7c0b2514c852a89dcf54648f) |
| 二进制 SHA-256 | `4b3f5d75d409cb697d46a41937e87d2123a59b67127dd2178c1e115423ee0d5d` | `7c645edd3c7fe9feae80386c93be856003b1b938f8ad20b5f47dd0898b64d652` | `e6721b2bc2eece65993521695dbb9f2819cef68902ea8e510aaa02157d350bc6` |

三方使用同一 benchmark 示例，SHA-256 为
`8551f815d1ca276154df9f2050a9039f0e9cd225da363137338d3c820ecc3624`。
工作负载、4096 次预热、每场景 7 批及“进程内中位数，再取三个进程中位数”的口径与
[前两组报告](../../resume-2026-11-20.md)相同；没有输出内部七批原值。
三方均使用 Rust 1.100.0 / LLVM 23.1.5、opt-level 3、fat LTO、1 codegen unit、
panic abort、strip debuginfo。

[候选构建记录](candidate-build.json)和[冻结回执](freeze.json)绑定源码、release 二进制、采集器及构建来源。
构建记录中的 `benchmarks_executed=false` 只描述构建阶段；本轮正式九次采样另见 comparison.json。
GDB/profile 属于诊断，停顿栈样本不是 CPU 时间份额；没有用调试构建或诊断时长替代本组 release 二进制吞吐。

采集从 **2026-11-20 01:25:23.701994 UTC** 开始，根上下文固定为
[36b367ff](https://github.com/LIghtJUNction/ztty/commit/36b367ffda4cea9b3b3dfbf38dc6375567912323)。
[native 启动记录](native-launch.json)和运行前后快照均记录 nice **−4**，三方通过 taskset 固定 CPU **23**。
原九次样本、原六次样本及此前 85f 二进制的身份保留检查均为 true。

## CPU 控制器与主机压力的证据时点

comparison.json 的运行开始/结束两个快照记录祖先链，所有实际读到的 `cpu.max` 都是
`max 100000`；叶节点没有记录 `cpu.max`。这不是连续采样，也没有把缺失文件补成某个数值。

[cgroup-readback.json](cgroup-readback.json)是**运行结束后的单独读回**，明确标记
`read_after_run_only=true`：叶节点只委派 memory/pids，未委派 cpu；父节点 `cpu.max=max 100000`，
systemd scope 报告 CPU quota infinity。它解释事后看到的控制器配置，不能冒充运行期间的额外采样。
两类证据分别保留，原 comparison.json 没有追加这些事后字段。

九次运行前后共 18 个主机快照中，load1 范围为 2.15966796875–3.455078125（24 个逻辑 CPU），MemAvailable 最低为 5048811520 B。完整 PSI、swap 和各次压力字段保留在原样本中；这些离散快照不证明两次读数之间没有波动。

压力门槛来自固定源码 [benchmark_terminals.py](https://github.com/LIghtJUNction/ztty/blob/36b367ffda4cea9b3b3dfbf38dc6375567912323/tools/benchmark_terminals.py)：
MemAvailable 至少为 max(2 GiB, RAM 的 10%)，load1 不高于可用逻辑 CPU 数的一半。
PSI 是记录项；准入通过不等于系统空闲。本组没有 deferred/contended，仍完整保留相关状态和每次 guard 输出。

## 公开复核与再次测量

```sh
python3 docs/benchmarks/resume-2026-11-20/core-three-way-a2074ba/verify-record.py
```

该脚本只检查公开文件摘要、九次顺序、stdout/stderr、七项汇总和 CSV，
并核对前六次与 85f 整组证据、此前报告字节没有改变。它不编译或运行 benchmark。

[run-three-way-borrowed.py](run-three-way-borrowed.py)是本次实际采集器原文，SHA-256 为
`9eaec60bdc52e969a5de5af1cd786865fb7e887465866a54ccd2c128ff892d73`。
它依赖原工作树布局、冻结二进制和未经脱敏的原始身份断言；[native-launch.json](native-launch.json)记录实际参数。
再次测量需要按固定来源和构建记录准备三方二进制，用新的目录、实际摘要和执行环境保存新样本，
不覆盖这里的历史记录，也不假定重新构建后的二进制拥有相同摘要。

原 comparison.json SHA-256：
`765ffc3b74c2191795ed0b587de27a7164a339d5271678663e9ecbe3570abc43`。
公开副本 SHA-256：`c8b92507ddb0a4679507a6ceb312ba23598826ae75457526a21c5ec719b5d75a`。

只替换必要的私有绝对工作目录前缀。comparison 恰有九个 command 路径改变；
构建/启动/冻结记录中同类目录值或以绝对路径为键的字段按同一规则处理。
[redaction.json](redaction.json)逐项记录原 SHA、公开 SHA、改变字段与原字段摘要。
数字字面量、键顺序、空白、失败信息和 host guard 不重写。
原 collector、receipt、cgroup 事后读回、各 stdout/stderr 均保持原字节。
