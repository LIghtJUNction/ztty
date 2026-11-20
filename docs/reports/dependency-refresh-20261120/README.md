# Project Z 依赖消费验收

日期：2026-11-20。应用基线为 `2410064eef530400f750225b0561262445a3fa66`。本轮把 ztty 的精确 zgpui 约束和锁文件推进到 `0.1.5`，并同步 zgpui、zinput、zgraphics 的固定源码。应用 Rust/C 实现、zpty 与 zterm 固定点保持既有内容。[当前依赖表](../../dependency-propagation.md)、[机器可读证据](evidence.json)

## 验收安排

- 核对版本约束、固定源码及便携核心的实际差异。
- 运行仓库本地门禁，再补当前依赖图的裸机编译。
- 验证真实 EGL、两种原生界面路径与插件进程行为。
- 检查 Cargo 包解析，并限定发行和平台结论。

## 固定输入与兼容边界

zgpui 的最终来源是 [`b4d5493e0cd985d00558c7be0b5726965ab5163a`](https://github.com/LIghtJUNction/zgpui/commit/b4d5493e0cd985d00558c7be0b5726965ab5163a)，其树为 `caf6d3c249c04f4547aa6556afb03c0c91da5e65`。第一阶段测试使用的 `48da8b7` 与此合并提交的树完全相同；第二阶段已使用最终提交。应用源码验证树为 `ce173782555a727b3009ba35080173cce06d6893`，后续文档与证据补充不改变该树中的 Rust/C 源码、依赖声明或锁文件。

从此前 `0.1.2` 的固定源码到本轮来源，zgpui 的五个便携核心与测试文件保持相同 Git blob 身份；变化位于可选原生层和新增 raster 子包。ztty 实际解析的 zgpui 特性列表为空，不启用这些可选层。[逐文件身份核对](source-identity.json) 与依赖解析记录共同支持这一接口边界。

zinput 当前核心新增四个输入键常量，原生适配器位于独立可选包；zgraphics 本轮固定点差异是发行前缓存准备及其测试。版本约束仍为 zinput `0.1.3`、zgraphics `0.2.0`、zpty `0.2.3`、zterm `0.5.2`。本轮没有用这些同版本号推断源码相同，实际来源由[固定提交](../../dependency-propagation.md)记录。

## 实际结果

下表的结果来自 Linux x86_64 开发机，Rust `1.100.0`，Cargo 与测试线程均限制为 1。X11 验收使用测试独占的 Xvfb、私有 broker 与合成 PTY 输入。原生界面测试使用二进制 SHA-256 `e7d431bd38141243005c61f4ae370679c8f2317e4c3c01f97b8bebb4c8b5ca1b`。

| 检查 | 实际结果 | 证据定位 |
| --- | --- | --- |
| `sh tools/check.sh` | 通过；汇总 494 项 Rust 测试，另有 8 项按原定义忽略 | `phase1` |
| 格式、严格 Clippy、Release 构建与 Rust `no_std` 依赖/符号检查 | 通过 | 仓库 `tools/check.sh` 与 `phase1` |
| 安装夹具、Bash 与 MCP transport/session/security/wait/SSE | 通过；私有资源清理完成 | `phase1` |
| 当前路径依赖图的 `x86_64-unknown-none` 消费检查 | 通过；实际使用 zgpui `0.1.5`、空特性列表 | `phase2.portable_zgpui` 与 `portable-check` |
| 原先忽略的 EGL/GPU 回归 | 7 项全部实际运行通过 | `gpu-tests` |
| 原生 UI：软件渲染 / GPU 代码路径 | 24 / 24 项通过，均无自有进程残留 | `ui.software`、`ui.gpu` |
| 插件 CLI / GUI | 22 / 10 项通过，清理错误为空 | `plugins.cli`、`plugins.gui` |
| `cargo package --locked --allow-dirty` | 通过本地打包及编译验证；生成包精确解析 registry zgpui `0.1.5` | `package` |

第一阶段的独立 zterm 裸机检查使用该库原有的注册表锁文件，因此第二阶段另外构建了一个指向当前 zterm、zgpui、zinput 和 zgraphics 路径的 `no_std` 消费者。它确认新依赖组合也能在裸机目标编译，避免把旧锁文件结果当作新图的验收。

第一次本地脚本因私有 XDG 环境下的 mise Python shim 配置信任检查退出。恢复仅在测试进程 PATH 中选择现有系统 Python 与 Rustup 工具，随后完整重跑成功；原失败回执保留在 `phase1`。没有修改全局配置或 trust 状态。

## 结论与证据对应

| 结论 | 证据 | 状态 |
| --- | --- | --- |
| 当前应用源码统一选择 zgpui `0.1.5` | manifest、锁文件、路径解析和生成包解析 | 已支持 |
| 默认 Rust 依赖图保持 `no_std` | 特性列表、符号检查和当前图裸机编译 | 已支持 |
| Linux 原生 UI 与插件消费仍可运行 | 两种界面路径、EGL、真实插件库和私有 PTY 回归 | 已支持 |
| 既有应用性能实现被保留 | 基线 `2410064`；本轮实现改动仅 manifest/lock/gitlink | 已支持；没有新增性能收益测量 |
| 本轮等于完整跨平台发行 | 没有执行新的发行与安装流程 | 不作此结论 |

GPU 代码路径使用 `llvmpipe (LLVM 23.1.5, 256 bits)`，结果不代表物理 GPU 或宿主桌面的验收。本轮未运行原生 Wayland、Windows、macOS 应用验收；正式发行继续使用完整原生工作流。剩余一项忽略测试是 zterm 的全 Unicode 标量穷举，原定义要求边界实现变更时显式运行；本轮没有修改该实现。

## 自审

- 贡献：消除版本与固定源码分歧，记录实际消费结果；没有宣称新算法或额外性能提升。
- 清晰度：区分源码树、二进制摘要、注册表版本和发行身份，并给出复核入口。
- 实验强度：使用实际 native/EGL/插件进程与像素检查，保留第一次环境失败。
- 覆盖范围：明示 llvmpipe、其他平台、发行安装及 Unicode 穷举的边界。
- 设计合理性：保留便携核心的空特性集合，按不可变来源统一依赖，未放松现有验收断言。
