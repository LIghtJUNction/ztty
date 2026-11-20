# 未采纳的批处理候选：28d5 九次测量与六对复核

**最终采用 a207 的 scalar 优化，未采用 28d5 批处理增量。**
同窗口六对比较中，batch / a207 的吞吐中位数比为 ASCII 0.9474、满历史 0.9136、SGR 0.9741、Unicode 0.9620；
Unicode 只有 2/6 对高于 a207。这些样本没有支持额外批处理带来稳定增量，因而选择范围更小的 a207。
这是一项采纳决定，不是对所有机器或工作负载的普遍快慢、统计显著性结论。

范围只有解析器与屏幕模型，不含 PTY、渲染、完整应用排名或物理输入延迟。
此前 6、85f 9、a207 9 次原件保持；本附录再保存 28d5 九次与 focused 十二次，
总计 33 + 12 次历史样本分别分析，不混池、不跨窗口相减推导单项收益。

## 固定身份与采用范围

| 身份 | 完整来源 | 正式吞吐二进制 SHA-256 |
| --- | --- | --- |
| a207 / borrowed | a2074bac4c030cfd7c0b2514c852a89dcf54648f | e6721b2bc2eece65993521695dbb9f2819cef68902ea8e510aaa02157d350bc6 |
| 28d5 / print_batch | 28d5d266375ff67562e16652a3eef10bd36b4756 | 0ad515013c182f9edcd4ffac152f0646659507e7676679504ee6ba80cfd6e2d5 |
| published 0.5.0 | 13d5e2c74571d35cccd920b1204798211b70c79f | 7c645edd3c7fe9feae80386c93be856003b1b938f8ad20b5f47dd0898b64d652 |
| legacy 0.4 | 155fea51aa138b0503ea1d80be4252aacb2c61a7 | 4b3f5d75d409cb697d46a41937e87d2123a59b67127dd2178c1e115423ee0d5d |

采集器所用根源码固定 da8b697d6ca3b3948810ee632e3f5d90cbd60913，前后源码、二进制与历史文件身份检查全部通过。
两种实验 manifest 都叫 0.5.1；相同版本文字不代表相同源码。
后续正式发布选用 a207：[zterm v0.5.1](https://github.com/LIghtJUNction/zterm/releases/tag/v0.5.1)
于 2026-11-20T02:18:36Z 发布，归档 78,051 B，
SHA-256 fc48bcaf9892a77d17ab235316448c3e235b0dccd00e5963b542586585c26c91。
28d5 保留实验来源，没有进入这份发布；宿主应用验收、发布、安装另行绑定。

## 三方九次测量

2026-11-20T02:07:51.635368+00:00 开始，ABC / BCA / CAB 三轮；
A=legacy、B=published、C=28d5，每种版本占每个顺序位置一次。
本组 28d5 Unicode 较 published 高 52.49%，却只为旧版的31.58%；ASCII较published低12.81%。
窗口没有直接测 a207，不能据此与上一窗口相减推导批处理的独立收益。

| 指标 | legacy | published | 28d5 | batch/published | batch/legacy |
| --- | --- | --- | --- | --- | --- |
| ASCII MiB/s | 274.406 | 247.662 | 215.927 | 0.8719 | 0.7869 |
| 满历史 MiB/s | 135.231 | 147.8 | 146.613 | 0.992 | 1.0842 |
| SGR MiB/s | 138.776 | 145.637 | 137.364 | 0.9432 | 0.9898 |
| Unicode MiB/s | 99.15 | 20.536 | 31.316 | 1.5249 | 0.3158 |
| Cell B | 28 | 32 | 32 | 1 | 1.1429 |
| 初始 heap B | 107932 | 123292 | 123292 | 1 | 1.1423 |
| 稳态 allocations | 0 | 0 | 0 | — | — |

## 同窗口六对复核

2026-11-20T02:14:02.523626+00:00 开始，只测 a207 与 28d5，AB / BA 交替六对。
下表比值是两个中位数相除；逐对比值另列，二者不混称同一个统计量。

| 指标 | a207中位数 | 28d5中位数 | batch/a207 |
| --- | --- | --- | --- |
| ASCII MiB/s | 259.7875 | 246.123 | 0.9474 |
| 满历史 MiB/s | 152.5695 | 139.38 | 0.9136 |
| SGR MiB/s | 148.187 | 144.3445 | 0.9741 |
| Unicode MiB/s | 32.586 | 31.347 | 0.962 |
| Cell B | 32 | 32 | 1 |
| 初始 heap B | 123292 | 123292 | 1 |
| 稳态 allocations | 0 | 0 | — |

| 对次 | 实际顺序 | ASCII比 | 满历史比 | SGR比 | Unicode比 |
| --- | --- | --- | --- | --- | --- |
| 1 | borrowed → print_batch | 0.7729 | 0.9175 | 0.9854 | 0.9963 |
| 2 | print_batch → borrowed | 0.9797 | 0.7989 | 0.9455 | 0.8113 |
| 3 | borrowed → print_batch | 0.8847 | 0.8992 | 0.9806 | 0.9855 |
| 4 | print_batch → borrowed | 0.9077 | 0.9041 | 0.9092 | 0.9453 |
| 5 | borrowed → print_batch | 0.9984 | 0.9928 | 0.9405 | 1.0149 |
| 6 | print_batch → borrowed | 1.1356 | 1.0993 | 1.0339 | 1.0681 |

全部样本没有丢弃最慢值。[逐次完整表](samples.md)和[CSV](measurements.csv)给出四项吞吐、
Cell/heap/稳态分配、edit与burst的p50/p95/p99及分配、elapsed和实际采集的child CPU时间。
[三方 raw](core-three-way-28d5d26/comparison.json)、
[focused raw](core-focused-a207-28d5/comparison.json)保留全部顺序、失败和主机压力字段。

a207 自己的[三方 cohort](../core-three-way-a2074ba/README.md)是91.404 /19.330 /26.101 MiB/s：
较published高35.03%，仍只是旧版的28.56%。**不能把28d5的+52.49%写成a207的结果，也不能说回退已解决。**

## 执行条件与准入边界

两侧 Rust 1.100.0、opt-level3、fat LTO、codegen-units1、panic abort、strip debuginfo一致。
共同example SHA-256为8551f815d1ca276154df9f2050a9039f0e9cd225da363137338d3c820ecc3624。
每进程taskset CPU23，native exec实际nice=-4；未在受限build unit内计时，不能套用构建时1CPU/768MiB限额。
执行前后context中实际存在的祖先cpu.max均为max100000，leaf未提供该字段；缺失不伪造为读取到无限额。
这两组有运行前后快照；早一组a207的独立事后cgroup读回仍只属于那个历史附件。

| 窗口 | load1范围 /24 CPUs | 最小MemAvailable B | 系统swap used B范围 |
| --- | --- | --- | --- |
| 三方9次 | 2.2178–2.6333 | 3327692800 | 20296486912–20730273792 |
| 六对12次 | 2.3193–3.3853 | 5120548864 | 20587130880–20801871872 |

21个子进程均exit0，stdout解析成功，每次之后的contention/reasons列表为空。
系统原本已有swap使用，raw保留完整PSI、load、内存和系统swap；本次未采集吞吐进程swap峰值，不宣称“零swap”。
focused额外记录child user/system秒数；三方未采集，表格留空或写未采集，不填0。

执行者确认两次各启动一次，没有前置guard延期、失败尝试或隐式重跑；
三方job尾号387、focused尾号392均exit0。
两份[三方](core-three-way-28d5d26/launch-receipt.json)/
[focused](core-focused-a207-28d5/launch-receipt.json)launch回执都明确recorded_after_completion=true，
是完成后据实际启动补写的元数据，不是预启动日志。
更早load16.8857/0samples的延期保留在[原历史报告](../../resume-2026-11-20.md)，不归到这两次。
GDB/debug/frame-pointer诊断没有混入吞吐。

## 证据与离线复核

| 原件 | 原 SHA-256 |
| --- | --- |
| 三方 comparison | 215fcf2b8d7cf70f92dd8de08d7093e45e370864aed14bdb6772ce18e8bec13d |
| focused comparison | 416fb0d8cd9dbb9297d8956f1b8dcf0a4ff75a56d7198c791a93d3cc4ccbcb28 |
| 三方 collector | b615db6147ea6dfaf52d0b98ec6a6180c058bfa9d4b8d7e91a4e07463f887cf8 |
| focused collector | a4aa9bd773c94716f60377c383d3f129108a2c741ba49db6e794bae579f10257 |

公开副本仅将私有根工作树前缀换成WORKTREE、性能工作树换成PERF_WORKTREE。
[redaction.json](redaction.json)给出每份原/公开SHA和字节数、全部变化字段和原字符串摘要。
数值字面量、JSON空白/键顺序、run顺序、失败/hostguard字段和stdout/stderr保持；原件不覆盖。
CSV/Markdown是派生视图，JSON保留权威数值。

```sh
python3 docs/benchmarks/resume-2026-11-20/rejected-print-batch-28d5d26/verify-record.py
```

命令离线重算中位数、比值、顺序和stdout对应关系，校验旧记录与文件摘要，不运行产品、不编译、不联网。
原采集器[三方](run-three-way-print-batch.py)、[focused](run-focused-print-batch.py)，
构建回执[a207](build-a207.json)/[28d5](build-28d5.json)、[benchmark源码](source/benchmark.rs)均保存。
hostguard来自[固定根helper](https://github.com/LIghtJUNction/ztty/blob/da8b697d6ca3b3948810ee632e3f5d90cbd60913/tools/benchmark_terminals.py)，
SHA-256 f87af5c337fcbc17c5a0ec519db1c1a391cae9460bc924f203efa9bc5c61cf69。

采集器要求准确的原始历史/二进制摘要并拒绝覆盖输出。公开脱敏文件不能冒充未脱敏原件通过这些断言。
重新测量应在独立目录按固定源码、工具链与回执完整命令构建，记录自己的新摘要/环境，
再使用相同准入规则和交错顺序；不同机器不预期得到相同MiB/s。
本次只归档与复核既有数据，没有启动新测量。
