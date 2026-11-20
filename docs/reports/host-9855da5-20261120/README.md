# 9855da5：Auto 软件路径修复后的宿主检查点

**本轮确认了显式软件环境下的内存问题已得到针对性处理，吞吐仍有需要保留的差距。**
新旧配对中，1 / 10 窗口应用 PSS 中位数为 12.09 → 12.83 / 17.74 → 21.42 MiB；
先前约 100 MiB 的额外成本没有在这次 Auto 路径重现。
逻辑输出吞吐的中位数之比分别为 +12.62% / −21.82%。
1 窗口旧版样本波动较大，不能把这一结果写成普遍加速。

本页固定生产源码与采集器
[9855da563f3038adee2fa88e1e24e20eafb77010](https://github.com/LIghtJUNction/ztty/commit/9855da563f3038adee2fa88e1e24e20eafb77010)。
与 [9f 功能检查点](../host-9f5a77a-20261120/README.md)相比，生产范围只改变 `src/x11/gpu.rs`：
当请求 Auto 且 `LIBGL_ALWAYS_SOFTWARE` **恰为 `1`** 时，在初始化 GPU 前选择已有 software 路径。
显式 GPU、变量未设或其他值沿用原有选择逻辑。
[固定对象对照](source/production-change.json)和[实际补丁](source/production-change.patch.gz)保留了范围。

本页记录新的本地门禁、32 项定向 GUI 和三组宿主性能采集。
正式标签 [v0.5.0](https://github.com/LIghtJUNction/ztty/tree/v0.5.0) 指向
[c4f8f48c9598284a589e609f25242b49ceabfea8](https://github.com/LIghtJUNction/ztty/commit/c4f8f48c9598284a589e609f25242b49ceabfea8)，
其父提交是 9855，新增范围只有此前的 9f 性能文档，生产/测试/工具/workflow 输入相同。
2026-11-20 03:32 UTC 启动的[正式发布门禁](https://github.com/LIghtJUNction/ztty/actions/runs/44668398577)
在本页整理时仍进行中；新六平台结果、下载制品及下游安装需由对应回执关联。

## 固定输入与功能结果

| 输入 | 身份 |
| --- | --- |
| ztty 源码 / collector | 9855da563f3038adee2fa88e1e24e20eafb77010 |
| Wayland feature ELF | 1,237,824 B；SHA-256 `9c0297477b2a24355f865d12413135a327a69a7a8f7dd50e40927ae9e21f1e66` |
| zterm | 已发布 0.5.1；a2074bac4c030cfd7c0b2514c852a89dcf54648f |
| Cargo.lock | SHA-256 `b86e3438a14ff8710a28b09bafa33ab1861f8cf116b722dc94be83133d4543b5` |

[本地 gate](local/local-gate.json)以 Wayland + bare-metal 要求运行完整 `make check`，47.163 s、exit 0。
[原始日志](local/local-gate.log)实际合计 **472 Rust passed、0 failed、6 ignored**。
这 6 项在本次 gate 中仍为 ignored；前一 9f 检查点单独执行过的 6 项 EGL 不计作本次新执行。
[C 服务预算结果](local/generated/wayland-services/validation.json)另记录五个场景在 ASan/UBSan 下通过：
满预算替换、超限拒绝、活跃 writer 保留、取消 writer 生命周期和最终记账。

门禁使用 768 MiB 内存上限、0 swap、1 CPU 配额。峰值达到 805,306,368 B，
`memory.events max=2856`，OOM / OOM kill 均为 0。
命中上限事件与 OOM 是不同指标，两者都在回执中保留。
[closure](local/closure.json)确认源码前后相同、原历史输出已恢复，自有 unit 的 MainPID 为 0、cgroup 为空。

### 新的定向 GUI：32 项

| 现有测试 | 实际检查 | renderer 观测 |
| --- | ---: | --- |
| native UI，Auto | 24/24 | software；driver 为空 |
| text，明确 GPU | 8/8 | gpu；llvmpipe；包含 GPU → software → GPU 热切换 |

[Auto 原始结果](gui/native-ui-auto.json)覆盖图像像素、结构化动作、焦点、失效手势、权限和生产者更新；
[GPU 原始结果](gui/text-gpu.json)覆盖字形、透明、复制、闪烁、字体重载及既有 PTY 的热切换断言。
这些是私有 Xvfb 中的实际像素/状态检查，不是物理桌面输入延迟测量。
两阶段均固定上述 ELF，自有进程与显示服务清理均为空。

原外层脚本在 Auto 24 项已经 exit 0 后，错误读取 `display_ownership`，而 native UI 报告使用 `xvfb`，
因而产生 KeyError。原 [acceptance](gui/acceptance.json)、[脚本](gui/run.py)和 [unit 日志](gui/unit.log)均保留。
[补充验收](gui/verified-acceptance.json)按实际字段校验；Auto 没有重跑，续行只执行此前尚未启动的 GPU 阶段。
native UI 测试会合法关闭首个 view 后再创建 producer view，因此前后窗口 ID 可以不同，renderer 语义仍逐项核对。
这 32 项定向检查没有被写成重新执行完整 21 阶段 / 365 项。

## 性能方法与样本

2026-11-20 03:19:39–03:22:28 UTC，三阶段顺序完成，均 exit 0，没有阶段内自动重试。
[cohort 回执](performance/cohort-receipt.json)记录整段采集耗时：
paired 32.458 s、output 6.469 s、terminals 130.197 s；这些不是单个输出样本的计时区间。

| 阶段 | 实际设计 | 保存的计时原值 |
| --- | --- | --- |
| paired | 新旧 ztty × 1/10 窗口 × 3 轮，12 runs | 198 个输出 burst；19,800 个 DSR 值 |
| output | 3 种负载 × headless/software window，6 组 | 18 个约 8 MiB 输出样本；1,200 个 idle DSR 值 |
| terminals | ztty/Ghostty/Kitty × 1/10 窗口 × 3 轮，18 runs | 297 个输出 burst；29,700 个 DSR 值 |

[SAMPLES.md](SAMPLES.md)列出所有 run 的未舍入结果；[CSV](burst-samples.csv)保存 513 个输出 burst 的顺序、
字节数、原始耗时和速率。全部 50,700 个 DSR 原值、资源快照、guard、退出和清理字段留在完整 JSON。

生产者向 PTY 写入数据后等待光标位置报告（DSR/CPR）。这个终点说明字节已被逻辑处理，
没有等待物理屏幕呈现。paired / terminals 每窗口预热后测三个 1,050,937 B burst，
先取窗口中位数，再取 run 与三轮中位数；同一时刻只激活一个 PTY 生产者，10 窗口不代表十个并发写入者总带宽。

PSS 按进程共享比例分摊内存页。应用 PSS 汇总被标记的终端进程及 helper/等待 launcher，
排除 probe 子树与独立 Xvfb/D-Bus；完整 X server 另列。
每 run 在固定画面与字体预热后取三次快照，未清空系统页缓存或字体缓存。

机器为 i9-12900HX、24 个逻辑 CPU、Linux 7.3.4-arch1-1，governor 为 powersave。
入口实际继承 nice −4 和 CPU 0–23 affinity；output 另将 broker/writer/Xserver 绑定 CPU 23/22/21。
运行前后记录的 leaf 没有 `cpu.max` 字段，存在该字段的父级均为 `max 100000`；
构建门禁的 1 CPU / 768 MiB 限额没有移植到性能采集。
各阶段 guard 均通过，采集边界 load1 为 3.6445–6.3320，可用内存最低 5,511,876,608 B；
原始 host swap / PSI 仍保留，不能把自有进程 sampled swap=0 写成整台机器无 swap。

## 一、新旧配对：吞吐与 PSS

旧基线为 0.4.1 / `155fea51aa138b0503ea1d80be4252aacb2c61a7`，
ELF SHA-256 `9f58c1a679ccca62cffd09d9991fbbb9c35fff4254f7d64dfc5eedaa6a2bb704`。
旧版不接受 renderer 配置，按已记录的兼容开关使用 application-default，实际 renderer 身份未知。
新候选请求 Auto，采集前后实际为 software，driver 为空。

| 窗口 | 旧版 MiB/s，中位数 [范围] | 新版 MiB/s，中位数 [范围] | 中位数之比 | 配对比值中位数 |
| ---: | ---: | ---: | ---: | ---: |
| 1 | 193.328 [158.604, 331.228] | 217.730 [186.000, 234.256] | 1.126221 | 1.172729 |
| 10 | 278.808 [272.193, 280.572] | 217.971 [197.398, 218.625] | 0.781796 | 0.779212 |

“中位数之比”先取两侧三轮中位数再相除；“配对比值中位数”先将同轮新旧相除再取中位数。
两者与全部区间同时保留。

| 窗口 | 旧 → 新应用 PSS MiB | 旧 → 新 DSR p50 µs | 旧 → 新 DSR p95 µs |
| ---: | ---: | ---: | ---: |
| 1 | 12.09 → 12.83 | 7.8560 → 13.3240 | 8.629 → 20.007 |
| 10 | 17.74 → 21.42 | 12.0500 → 13.8900 | 18.790 → 21.776 |

新应用 PSS 分别比旧版高 0.74 / 3.68 MiB。
前一 [9f cohort](../../benchmarks/host-9f5a77a-20261120/README.md)的完整回退结果没有覆盖。
内存修复的依据由三个相互衔接的观测组成：同一 9f ELF 的 Auto/software 诊断相差 95.52 MiB；
固定补丁在 GPU 探测前处理明确的软件环境；本轮 Auto 的实际 driver 为空，额外约 100 MiB 的成本未再出现。

不能用两个 cohort 的吞吐相减来估算修复收益：旧版 1 窗口中位数也由上一轮约 296 MiB/s 变为本轮 193.328 MiB/s。
本轮 10 窗口三轮均低于对应旧版，−21.82% 的中位数之比仍是需要公开保留的结果。
这与核心 a207 的 Unicode 部分恢复是不同负载，不相互替代。

## 二、输出管线

六组都使用新 ELF，每组三个约 8 MiB 样本；window 计时前后校验实际 software，headless 没有终端窗口。
两种模式均保留完整可见文本检查。这部分没有旧版对照。

| 负载/模式 | 三次 MiB/s（原顺序） | 中位数 MiB/s | idle DSR p50 / p95 µs |
| --- | --- | ---: | ---: |
| ascii-log-headless | 114.057, 134.441, 134.401 | 134.401 | 16.875 / 18.648 |
| ascii-log-window | 92.594, 103.317, 102.929 | 102.929 | 32.859 / 36.549 |
| inplace-ascii-headless | 193.688, 193.790, 194.454 | 193.790 | 16.726 / 17.813 |
| inplace-ascii-window | 118.644, 140.823, 139.849 | 139.849 | 19.277 / 34.734 |
| unicode-log-headless | 27.238, 27.204, 27.421 | 27.238 | 16.874 / 18.751 |
| unicode-log-window | 24.138, 24.419, 24.647 | 24.419 | 25.268 / 26.724 |

全部输出与 idle DSR 原值在 [output 附件](performance/output/)。
原始 CPU 时间以 10 ms 内核 tick 计数，不将其解释成纳秒级 CPU 测量。

## 三、三应用矩阵

矩阵覆盖共享模式、固定 idle 画面与 1/10 窗口；没有测 history、images、hidden 或独立进程模式。
ztty 明确使用 GPU，实际 driver 为 llvmpipe 软件 OpenGL，与上面 Auto→software 配对分开。
Ghostty 1.4.0+omarchy.1 日志只提供 Mesa/OpenGL 4.6；Kitty 0.50.0 没有实际 driver 记录。
这些条件不支持宣称三者使用同一物理 GPU、同一 driver 或给出通用排名。

| 应用 | 窗口 | 应用 PSS MiB | 应用 + 完整 X server PSS MiB | MiB/s，中位数 [范围] | DSR p50 / p95 µs |
| --- | ---: | ---: | ---: | ---: | ---: |
| ghostty | 1 | 315.09 | 399.47 | 171.837 [110.338, 195.212] | 22.2355 / 383.177 |
| kitty | 1 | 177.02 | 259.95 | 57.896 [57.323, 66.982] | 4476.7720 / 9450.602 |
| ztty | 1 | 136.11 | 221.18 | 120.201 [112.024, 170.107] | 9.2285 / 11.114 |
| ghostty | 10 | 814.20 | 924.11 | 187.582 [170.461, 197.635] | 31.8130 / 121.334 |
| kitty | 10 | 356.34 | 452.58 | 59.001 [54.391, 60.248] | 4939.9540 / 8452.239 |
| ztty | 10 | 326.55 | 440.87 | 149.798 [132.084, 150.738] | 14.1655 / 27.697 |

三应用各有 33 次窗口运行。ztty 的 33 次有完整 32×120 文本检查；
Ghostty 与 Kitty 各 33 次只记录 CPR 与 payload hash，没有完整屏幕文本 oracle。
加上 paired 的 66 次 ztty 窗口运行，总计 99 次完整文本检查、66 次 CPR/hash 范围检查。
应用及专用 X server 的 sampled swap 均为 0；resident PSS 与 swap 保持分列。

[完整 JSON](performance/terminals/terminals.json.gz)、[原图](performance/terminals/terminals.svg)及
[规模图](performance/terminals/terminals-scaling.svg)保留全部三轮。
Ghostty ELF SHA-256 为 `ef28bf3a0f6ead69014da86ab0c9a2ef30fcf3bf297a6a27c4c18dfc80a56a62`，
Kitty 为 `e3aec26698a05b4c024a672e92365649a8fdeff8e884e432f345b14d54e20dd7`；身份前后由原件校验。

## 维护边界与依赖传播

计数继续采用同一脚本与 f2db31d7 基线，包含空白、注释和 inline tests，排除专用测试/示例。
`build/wayland.rs` 在两侧都属于既有规则之外，未为本次结果改变口径。

| 范围 | 文件数，前 → 后 | 物理行数，前 → 后 | 后者字节数 |
| --- | ---: | ---: | ---: |
| 根仓直接维护 | 79 → 89 | 40,436 → 30,893 | 1,094,322 |
| gitlink 库 | 3 → 39 | 1,345 → 14,306 | 491,136 |
| 全部展开 | 82 → 128 | 41,781 → 45,199 | 1,585,458 |

根维护面减少 9,543 行，展开后的总代码增加 3,418 行；独立库迁移与共享 Wayland 的新增实现需要一起计数。
相对 9f，本次根/展开均只增加 7 行、263 B，库不变。
[完整 audit](size/audit.json.gz)、[摘要](size/SUMMARY.md)与[执行回执](size/invocation.json)保留逐文件/blob 证据；
19 个文件的 blob 相同而归属改变，不能算作实现代码删除。旧 37,848 行来自不同口径，未混入本表。

五个 gitlink、Cargo manifest 与锁文件均未因本次修复改变，六库 registry 消费证据继续引用
[9f changed-graph 1+1](../host-9f5a77a-20261120/README.md)及
[36b 完整六独立 + 组合](../plugin-api-registry-20261120/README.md)。
本页没有声称再次运行 registry 图；`no_std` 是库目标，运行使用 std harness，组合消费者实际创建 PTY。
API 0.1.0 的发布 VCS 仍为 36b367ff，而非当前应用 SHA。

## 证据与离线核验

| 原件 | 原始 SHA-256（脱敏前） |
| --- | --- |
| [local/local-gate.json](local/local-gate.json) | `42a6580e0972b717cb87d728ddb9f97829cbd1a2f5d355d311dbf0a8d760b842` |
| [gui/acceptance.json](gui/acceptance.json) | `2ef3b60aad2c6b6b6d6248b665def7e80d4cbeaeac8e06284be241a2a2765c2f` |
| [gui/verified-acceptance.json](gui/verified-acceptance.json) | `5ba3188db7f33361f8097d8fbed24dc226c065f69a1bbe419d30b8785239f25b` |
| [performance/cohort-receipt.json](performance/cohort-receipt.json) | `2261bf8737db349c64ea8abc6f89b9a43f5b5217a682c7b37f8bad33da7fadcc` |
| [performance/throughput/paired.json.gz](performance/throughput/paired.json.gz) | `f3d6455464647119ed5eab9a4a6d1b89e49b146981ee5f8d29e283dd5cbcb9cc` |
| [performance/terminals/terminals.json.gz](performance/terminals/terminals.json.gz) | `4fee5064151ce7742ffd8a4faa579032b14ac00924735bd4a77fe74dc4ad87ad` |
| [size/audit.json.gz](size/audit.json.gz) | `3a8145b504404acdc0195221973fb3fb5c02a0044f6b0b59324f8a491a2dc0da` |

```sh
python3 docs/reports/host-9855da5-20261120/verify-record.py
```

Python 3.11+ 脚本只读公开附件：核对文件/hash、来源链、原失败与补充关系、472/32 的实际计数、
513 个 burst 的算术、DSR/中位数/清理/guard、源码规模及报告表格；不启动产品、编译或重新测量。
入口命令在各 launch/receipt 的数组中，固定采集器在 [collector](collector/)。
重现实验需准备相同源码/二进制、字体与显示服务，将路径占位换成本机位置，并保留主机压力与实际 renderer 记录。

[redaction.json](redaction.json)逐文件记录原始 SHA、公开 SHA 与路径变化字段。
只替换私有绝对路径前缀；数值、顺序、失败和资源原值保留。三个大型 JSON 与保持原始 Git 空白的补丁使用无损 gzip，
压缩文件与解压内容分别核验。原始 runs 的规范化摘要在脱敏前校验，公开副本另有自己的摘要。

此前的 [0 样本启动失败、9f 完整性能与内存诊断](../../benchmarks/host-9f5a77a-20261120/README.md)
以及[核心微基准、未采纳批处理实验](../../benchmarks/resume-2026-11-20.md)均保留原时点。
本页是一个新的固定源码检查点，未重写它们的结论。
