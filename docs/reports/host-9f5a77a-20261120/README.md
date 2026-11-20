# 9f5a77a：zterm 0.5.1 的宿主消费与功能验证

**固定源码 9f5a77a 接入 zterm 0.5.1 后，本地完整门禁、六项 EGL 专项、21 阶段 GUI/CLI 与更新后的 registry 消费检查均有通过记录。**
本页保存这次 Linux 宿主运行的原件与来源关系：472 项 Rust 通过，另有 6 项 EGL 实际执行通过；
新 GUI/CLI 运行共 365 个记录检查通过。EGL 收尾解析器最初的 exit 1 和补充核验分别保留。

源码为 [9f5a77a1299093d0ba609bfafa2cec3b0ba931dd](https://github.com/LIghtJUNction/ztty/commit/9f5a77a1299093d0ba609bfafa2cec3b0ba931dd)，
消费的 zterm 为 [a2074bac4c030cfd7c0b2514c852a89dcf54648f](https://github.com/LIghtJUNction/zterm/commit/a2074bac4c030cfd7c0b2514c852a89dcf54648f)。
这是新核心接入后的实际执行。较早的 [edae 六架构报告](../native-edae40b-20261120/README.md)
绑定 zterm 0.5.0，继续保留自己的时点；它没有被改写成 9f5 的 Windows/macOS 验收。

本页不提供宿主吞吐、物理桌面输入延迟或应用发行/安装结论。
这些结果需要各自的采集器、制品和运行证据。

## 固定输入

| 输入 | SHA-256 / Git 提交 |
| --- | --- |
| Wayland-enabled ELF，1,237,696 B | c7935e9e76476a9cd7d0186f23f5806e3e2e2fc556a494b0e3741a2c893b3807 |
| --no-default-features ELF，1,144,288 B | 8ac2b30e72c33b68688234a180bd25e48f0fa64f8b54fac492fd597568bdeb46 |
| 测试 IME peer | 521fe34ee30e65aab7fb940939a623d94b8e7a86bb38743bb5caaac87ddb7f2d |
| 宿主 Cargo.lock | b86e3438a14ff8710a28b09bafa33ab1861f8cf116b722dc94be83133d4543b5 |
| zterm gitlink | a2074bac4c030cfd7c0b2514c852a89dcf54648f |

[Cargo.toml](source/Cargo.toml)与[锁文件](source/Cargo.lock)来自固定 Git 对象。
[GUI acceptance](gui/acceptance.json)保存两份 ELF、IME peer、插件、82 个测试/辅助文件的前后摘要，
以及干净源码和子模块身份。最终身份检查为 true，前后记录一致。

## 472 项常规 Rust 与六项 EGL

[完整本地回执](local/gate.json)记录 `make check`，启用 Wayland 并要求 bare-metal 检查，
exit 0，运行后源码仍为 9f5、tracked changes 为空。
[原始日志](local/gate.log)的 Rust test-result 行合计 **472 passed、0 failed、6 ignored**。
这里没有把 ignored 的六项当作已经执行。

随后运行的 EGL 专项确实执行了这六项，原始 [tests.log](egl/tests.log)记录
**6 passed、0 failed、0 ignored**，覆盖图片裁剪/透明与替换、字体重载、4K 纹理列、
连写字体与 emoji、字形图集复用，以及交错窗口/上下文生命周期。

原 [original.json](egl/original.json)仍保持 `passed=false`：
六项测试中的第一项在名称与结尾 `ok` 之间打印了 renderer 行，
原收尾脚本按单行匹配只识别到五项，随后断言失败，unit exit 1。
两条 Cargo 命令本身均 exit 0。原 [run.py](egl/run.py)、[unit.log](egl/unit.log)和日志没有覆盖。

补充 [verified.json](egl/verified.json)只重新核对完成的证据：
六个准确测试块都以 `ok` 结束，汇总为 6/0/0，Cargo exit 0，源与二进制仍一致。
**没有重新编译或重跑测试。**

此专项实际打印的 OpenGL renderer 为 **NVIDIA GeForce RTX 4060 Laptop GPU/PCIe/SSE2**。
这支持六项 EGL 用例的实际 renderer 身份；不能沿用较早候选的“软件 Mesa”标签。
它仍不等于用户物理 Wayland 桌面的端到端输入延迟测试。

## 新的 21 阶段 GUI/CLI

运行时间为 2026-11-20 02:24:27–02:26:30 UTC。
[acceptance.json](gui/acceptance.json)和[final-closure.json](gui/final-closure.json)记录
21/21 阶段通过、365 个检查通过，每个阶段 exit 0。
各阶段原始 JSON 在 [gui/stages](gui/stages/)；表中的模式名沿用夹具记录。

| 阶段 | 实际 checks |
| --- | ---: |
| 1. xim-all | 37 |
| 2. input-software | 14 |
| 3. clipboard-transfer | 25 |
| 4. copy-paste | 13 |
| 5. osc52-gpu | 13 |
| 6. graphics-software | 8 |
| 7. graphics-gpu | 8 |
| 8. text-software | 8 |
| 9. text-gpu | 8 |
| 10. retained-frame-software | 2 |
| 11. repaint-software | 11 |
| 12. repaint-gpu | 11 |
| 13. native-ui-software | 24 |
| 14. native-ui-gpu | 24 |
| 15. workspaces | 17 |
| 16. shared-gui | 9 |
| 17. plugin-cli-current | 22 |
| 18. plugin-cli-legacy | 22 |
| 19. plugin-gui-current | 10 |
| 20. wayland-shared-gui | 65 |
| 21. wayland-cli | 14 |

这次包含当前与旧版插件 CLI 各 22 项、当前插件 GUI 10 项；
原插件来源 155fea/zplugin 0.3.0 与当前 zplugin 0.3.1 的身份在 acceptance 中分别保存。
Wayland 的 65 项属于本次 9f5 ELF，不借用另一候选的 65 项。

GUI 使用私有 Xvfb、headless Sway 和测试 IME peer。
真实桌面输入法、物理显示链路与新核心的 Windows/macOS 运行范围仍需要对应证据。
夹具内的计时字段保留作原始记录，不提升为公平的应用性能比较。

所有阶段的 owned remaining 列表为空，私有 runtime 已删除。
第 21 阶段清理记录包含四次对自有子进程的 SIGTERM，之后 remaining 仍为空；
正常有界清理没有被写成“全程没有信号”。
[原 unit journal](gui/unit-journal.jsonl)与[启动回执](gui/launch.json)保留。
临时 unit 卸载后读到的 Infinity/Nice 0 不是执行时限额，应使用启动记录与实际运行回执。

## Registry：只复验实际变化的消费图

本次是 **zterm 单独消费者 + 六 crate 组合消费者**，即 1+1。
各运行 metadata、check --lib --locked、run --locked，共六条 Cargo 命令，均 exit 0。
[完整 result](registry/result.json)与[八个 consumer 文件](registry/consumers/)保存精确版本、锁、源代码和命令。
两个消费者的库目标均为 no_std，依赖来自精确 crates.io 版本，没有 path/git 替代。
no_std 指库目标；实际运行使用 std harness，组合消费者真实创建 PTY。

| Crate | 版本 | Registry VCS |
| --- | --- | --- |
| zgpui | 0.1.0 | 1d796d1fc63808dc099ed7b10c92e75835ae2814 |
| zgraphics | 0.2.0 | fe6357f98cf80fb6cc74739e45e8866ee17706ad |
| zinput | 0.1.3 | 37617313dd7aaabcca9dd567344a76c4cbe03907 |
| zpty | 0.2.2 | 66919bc6dcb7757e0437fe955bfa809fb1282b68 |
| zterm | 0.5.1 | a2074bac4c030cfd7c0b2514c852a89dcf54648f |
| ztty-plugin-api | 0.1.0 | 36b367ffda4cea9b3b3dfbf38dc6375567912323 |

新 zterm 包 checksum 为
`fc48bcaf9892a77d17ab235316448c3e235b0dccd00e5963b542586585c26c91`；
API 包仍为
`e00adc5df1eefa237ea1ccd8d837ceecabef1976141b78918901d95cb8e9d4a6`，VCS 仍为 36b，
不能改成当前宿主 9f5。其余包的 checksum 一并保存在 result 与 consumer locks。

较早 36b 的[六个独立消费者加一个组合消费者](../plugin-api-registry-20261120/README.md)
属于完整旧图覆盖，原 result SHA 保留。本次没有重跑其余五个独立检查，也没有把 1+1 写成新六独立验收。

[closure-helper-error.json](registry/closure-helper-error.json)保留一次收尾辅助代码错误：
空的退出后 ControlGroup 被误解为根 cgroup，读取断言在任何进程操作前停止。
随后用已记录的精确 owned-unit cgroup 完成[收尾核验](registry/closure.json)。
六条功能命令没有失败或重跑，自有 unit 已停，剩余 PID 为空。

## 同规则源码规模

固定比较 f2db31d7ea35bf1a0554e76d69d2e44faafd0cab → 9f5a77a。
同一[审计脚本](size/audit.py)递归枚举生产根中的受 Git 管理 blob 和 gitlink，
完整规则、逐文件摘要和排除项保存在无损压缩的 [audit.json.gz（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)。

| 范围 | 文件数：前 → 后 | 物理行：前 → 后 | 最终字节 |
| --- | --- | --- | --- |
| 根仓库直接维护 | 79 → 89 | 40,436 → 30,886 | 1,094,059 |
| gitlink 库 | 3 → 39 | 1,345 → 14,306 | 491,136 |
| 展开合计 | 82 → 128 | 41,781 → 45,192 | 1,585,195 |

根少 9,550 行，展开总量增加 3,411 行。结果说明维护职责转移与模块拆分；
不能写成整个生态实现减少。审计另列 19 处相同 Git blob 的维护归属变化。

计数包括空白、注释和 inline tests，排除专用 tests/examples 等；
`build/wayland.rs` 在前后双方都按原规则归为 outside_production_roots。
不与旧 37,848 行的另一口径比较。
最终 zterm 为 17 个生产文件、4,838 行；根最大文件为 mcp_http.rs（1,593 行），
递归最大为 zinput/src/lib.rs（1,920 行）。
[SUMMARY](size/SUMMARY.md)与[原 invocation](size/invocation.json)保存具体范围。

## 资源与原始证据

| 阶段 | 实际记录 | 说明 |
| --- | --- | --- |
| 完整本地 gate | 70.405 s；805,306,368 B peak；max 事件 3,326 | 达到 768 MiB cap，OOM 0、swap 0 |
| Default build + EGL | 805,306,368 B peak；max 事件 108 | 同样达到 cap；原解析器 exit 1 保留，OOM 0、swap 0 |
| 21 阶段 GUI | journal 记录 503.7M peak | 保留日志原精度，不换算成伪精确字节 |
| Registry 1+1 | 12.001569275002112 s；358,707,200 B peak | 受限 unit，OOM 0、swap 0，收尾完成 |

资源数字只属于对应阶段，不是终端吞吐或输入延迟。

| 原件 | 原 SHA-256 |
| --- | --- |
| 本地 gate.json | ce7f2aa4ac05f0f49f1b1488eef1c25fc4f435a45d2401ee0bade55efe1e815c |
| 本地 gate.log | 85af8505e98f228033a41c5776df34430b062f1a1034584e85e0e8c6ab813bd7 |
| 原 EGL receipt | 241772bcda2c547db919947a28fc3fbfff94b1cd4293f99bda2c0d54bf2f01a9 |
| EGL verified | ade83d7a11cf5c4ee63eabea23196bba6234c71be7cdaed101e15cc6883dade6 |
| 新 GUI acceptance | b1fd96e9879f999493383fbd1545729315c114de46ba33b5bc6dc45480cfcee8 |
| 新 GUI closure | 7a396a250709fd5bfcce9bcf1baabc17fc6996b11d240837b266770031507612 |
| Registry result | 424cab026a959bce33ae8caa771e5689e0b535e298079d92903944a83d42f94a |
| Registry closure | 435e8367dcb5999ac337d29445fcbc7c2ae333d1130998ba558e36ce43419562 |
| 完整源码 audit | b47d26d8785f43d2e52010c3007328999a2ea6e2186065e06ba146a822c9c721 |

## 离线复核

```sh
python3 docs/reports/host-9f5a77a-20261120/verify-record.py
```

需要 Python 3.11 或更新版本。脚本只读取这些附件，复核固定身份、原始失败与补充核验的关系、
472+6 汇总、每阶段实际 checks/退出/清理、registry 精确来源与六条命令，以及逐文件规模合计。
不会运行产品、Cargo、GUI 或 benchmark。

[redaction.json](redaction.json)记录所有原/公开 SHA、字节数和变化字段。
公开副本只替换必要的私有绝对路径前缀；数值、顺序、原始失败、stderr 与资源字段保持。
audit 先按同样规则脱敏，再 gzip 无损保存；压缩文件与解压内容各有摘要。
原件留在对应 ignored artifacts，旧报告和失败记录不覆盖。
