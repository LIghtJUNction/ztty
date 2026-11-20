# ztty 0.5.2 发布与验收报告

日期：2026-11-20（Asia/Taipei）。本报告将代码改动、实测结果、公开发布和本机安装分别记录，所有结论绑定到同一发布源码。

## 1. 发布身份与交付状态

| 项目 | 值 |
|---|---|
| 版本 | `ztty 0.5.2` |
| 发布源码 | `6e2ce246b0b50e6b82da99756832feae6e966f21` |
| 源码 tree | `57c38b1e962578a29dae65c4e54adab65fd6b1bf` |
| 不可变标签 | `v0.5.2` |
| annotated tag 对象 | `8b2fd0c22b3d53f630f0377ca5a9d225df1a089b` |
| 正式工作流 | [Release #44712216777](https://github.com/LIghtJUNction/ztty/actions/runs/44712216777) |
| GitHub Release | 已公开，47 个附件；2026-11-20 17:45:17（UTC+08:00） |
| crates.io | 已发布，未 yanked；公开包与验收包 SHA-256 完全一致 |
| AUR | Git HEAD `1bcb8f01463f29210beb3080ee557758db027080`，版本 `0.5.2-1` |
| 本机实际 PATH 命令 | 交互式 Zsh：`/home/lightjunction/.cargo/bin/ztty` → `ztty 0.5.2` |
| 下游套件 | 公开 oh-my-ztty 最新版在核对时仍为 0.2.5；0.2.6 的单一发布负责人正在整合 0.5.2 的源码指针和正式资产锁，套件发布状态单独记录 |

代码和版本更新已通过 GitHub 插件分阶段提交。最终发布使用上述固定提交；发布后的报告提交不会移动标签或改变已发布制品。

## 2. 本轮改动与实际效果

### 2.1 大段输入结束后释放大缓冲

输入队列需要暂存尚未写进 PTY 的字节。`Vec::clear()` 只把有效长度改为零，仍保留原来分配的空间。因此，一次很大的粘贴完成后，空队列仍可能长期持有同样大的缓冲。

0.5.2 在既有清空路径中加入容量判断：容量超过 **64 KiB** 时释放分配，不超过这一阈值时继续复用。普通键盘输入保留复用行为；大段输入全部交付、输入控制权移交或明确取消后，不再由队列保留大分配。部分写入、`WouldBlock` 和错误处理仍保存尚未发送的后缀，已经被内核接受的前缀也不会被撤回。

| 同一个 8 MiB 输入案例 | 旧实现 | 0.5.2 |
|---|---:|---:|
| 入队后的容量 | 8,388,608 B | 8,388,608 B |
| 清空后仍由队列持有的容量 | 8,388,608 B | **0 B** |
| 小缓冲复用回归 | 通过 | 通过 |

基线源码为 [4e72b17](https://github.com/LIghtJUNction/ztty/commit/4e72b17bf938c2e18972a7300160d150221e486b)，改动提交为 [d1db25c](https://github.com/LIghtJUNction/ztty/commit/d1db25ccfdac288ccc1fbf2ff0ffa76f15c899d4)，已纳入最终发布源码。旧代码在新增大缓冲回归中按预期失败，修复后通过。原始测量和复现步骤见[内存报告](https://github.com/LIghtJUNction/ztty/blob/v0.5.2/docs/benchmarks/input-buffer-memory-20261120/report.zh-CN.md)。

**测量边界：**这里测量的是队列拥有的分配容量。释放后，内存分配器可能继续缓存这段空间，因此本结果不能解释为整进程 RSS/PSS 必然下降 8 MiB。本轮没有测量整进程 RSS/PSS 或吞吐提升。频繁的大粘贴需要重新分配缓冲，这是减少长期保留容量的明确取舍。

### 2.2 保证源码和实际发布包使用同一组依赖

Cargo 发布会移除本地 `path` 和 `patch`，从 registry 重新解析依赖。先前的普通版本约束允许兼容更新，导致 0.5.1 源码使用 `zgpui 0.1.2`，实际归档却选入 `0.1.3`。0.5.2 在应用层使用精确约束，并核对实际 `.crate`，使源码与用户安装的包得到同一组版本和 features。

| 依赖 | 固定版本 |
|---|---|
| zgpui | `0.1.2` |
| zgraphics | `0.2.0` |
| zinput | `0.1.3` |
| zpty | `0.2.3` |
| zterm | `0.5.2` |
| ztty-plugin-api | `0.1.0` |

实际归档中，全部非 root 锁项均来自官方 registry；normalized manifest 不含依赖 `path`、`git` 或顶层 `patch`、`replace`。源码与解包后的 **22 个解析节点**在名称、版本、features 和依赖边上完全一致，归档中的 **1,774 个源码文件**与冻结源码逐字节匹配。`zgpui 0.1.2` 的 checksum 为 `0c5cddf5d0ed86ccb9cf21e035585146cf5aff0e316301bc037684b3c51b1046`。

实现和流程说明见 [Cargo.toml](https://github.com/LIghtJUNction/ztty/blob/v0.5.2/Cargo.toml) 与 [releasing.md](https://github.com/LIghtJUNction/ztty/blob/v0.5.2/docs/releasing.md)。

### 2.3 修正窗口测试中的就绪条件，保留真实事件断言

切换标签时，新窗口已经映射，并不表示旧窗口已经取消映射。原有探针在这一中间状态发送外部释放事件，可能仍受到旧窗口的 X11 指针抓取影响。0.5.2 的探针同时等待“新视图可见”和“旧视图已取消映射”，再执行外部释放检查。

原有超时、外部窗口实际收到 `ButtonRelease`、PTY 字节以及清理断言均保留。修正后的同一探针在最终源码 ELF 的 GPU 和软件渲染路径各通过 24 项检查。代码见 [native_ui_gui.py](https://github.com/LIghtJUNction/ztty/blob/v0.5.2/tests/native_ui_gui.py)。

正式 Linux 发布流程还保留并验证了图片粘贴保护、两种渲染模式下的真实重载和完整标签切换画面。每份证据都必须绑定对应发布 ELF 的 SHA、实际 renderer 和清理状态。

## 3. 验收结果

### 本机固定源码与真实发布包

| 验收 | 结果 |
|---|---|
| 格式、严格 Clippy、Wayland release 构建、no_std 审计 | 全部通过 |
| root/workspace 测试 | 191 通过、0 失败、6 项既有 ignored |
| SessionCore 真实 Linux 管道背压回归 | 10 项通过，子进程全部回收 |
| 剪贴板 GUI | 25/25；INCR 收到 1,360,012 B，内容 SHA 一致 |
| 原生窗口交互 | GPU 24/24，software 24/24 |
| `cargo package --locked --features wayland` | 实际打包及 Cargo 自带验证通过 |
| 离仓、离线、锁定依赖的独立安装 | 通过；安装版本为 0.5.2 |
| 源码 ELF → 独立安装 ELF 重载 | 9 项检查、2 次真实成功重载 |
| 本机 Codex 独立只读复核 | 固定源码与原始回执核对通过，无发布阻塞发现 |

容量回归属于上述 191 项测试，不重复相加。本机 GPU 路径使用 llvmpipe，代表 GPU 渲染路径验收，不代表物理显卡性能基准。测试仅创建并清理自己拥有的进程，没有重启用户现有会话。

### 正式发布工作流

Verify 以及 Linux、macOS、Windows 各两个架构的正式任务均通过。正式运行一次通过，最终状态为 success。两份公开 Linux tar 的哈希和大小与 Actions 产物一致；每个架构的 22 份报告、312 项报告检查均绑定对应发行 ELF。图片粘贴保护 10 项、剪贴板 25 项、GPU/software 窗口交互各 24 项、重载各 9 项和两次 exec、标签切换各 3 项，以及 Wayland 65 项均通过。两个标签切换 scope 的 invalid samples 为 0，owned process 清理通过。

发布工作流实际构建并安装了 Arch 源码包，核对六平台产物、证据合同和 GitHub 下载回读后才公开 Release。源码 tar 的 SHA-256 为 `85a77183e74d359bd02980f309a1f0d50ff1c65405bec5d21b016723720a0da3`；完整原生证据 ZIP 为 `95967bb333dbf0e417b8c7bc264006a891b29a3d390116562970a3bec0385020`。AUR 的公开 Git checkout 已读回 `0.5.2-1`，其源码 URL 和 SHA 与 Release 一致。Cargo 发布随后成功，公开 index 和实际下载包均匹配下表的 `.crate` 身份，包内提交和六个 Z 依赖也已再次核对。

### 可复核的产物身份

| 产物 | 字节数 | SHA-256 |
|---|---:|---|
| 本机源码构建 ELF | 1,386,344 | `531130e14db08583624ba66b43b54824a319d8e0aba79fd8993f3f70907e9355` |
| 验收的 `.crate` | 7,791,804 | `75e3c82bbd2ca39334f554169027ae9a16c12f326af225faec7907d8cb664025` |
| 从解包内容独立安装的 ELF | 1,367,384 | `92632835aad93254e67d5eed7af7c762488e090f174c42852448529a792a4316` |
| 本机最终 PATH ELF | 1,367,376 | `0d697554c280ddc51631874d67a4ce729c709cce861d400e052e00e9f95b174a` |

源码构建、registry 安装和各平台构建的二进制身份分别记录，不把不同构建环境的 ELF 当作同一个字节产物。发布身份由冻结源码、实际依赖图、验收结果和制品哈希共同确定。

## 4. 历史版本与安装记录

`v0.5.1` 保持原有不可变标签和失败记录。其[正式运行](https://github.com/LIghtJUNction/ztty/actions/runs/44706305817)在两个 Linux 架构的 GPU outside-release 探针失败，GitHub Release 和 Cargo 应用发布均未完成。本次没有覆写旧标签；0.5.2 重新执行了完整正式门禁。

本机通过 Cargo 的正常安装机制，将交互式 Zsh 命中的本地源码版 `0.4.1` 升级为官方 registry 的 `0.5.2`，启用 `wayland`，使用 release profile。安装前备份了旧二进制和两份 Cargo tracking 文件。安装后，两份 tracking 文件中的其他 **37 个 crate 条目均保持原样**；独立回读确认了实际 PATH、版本、ELF、官方 registry 来源与 features。

机器上的另一条路径 `~/.local/bin/ztty` 链接到系统包 `/usr/bin/ztty`，非交互式任务可能按自身 PATH 使用该系统包。此次核实的交互式终端入口是 `~/.cargo/bin/ztty`；报告不将两者混为同一次安装。现有运行会话继续使用其已加载的镜像，本轮未对用户会话做强制重载。

## 5. 交付入口

- [GitHub Release v0.5.2](https://github.com/LIghtJUNction/ztty/releases/tag/v0.5.2)
- [源码与提交](https://github.com/LIghtJUNction/ztty/commit/6e2ce246b0b50e6b82da99756832feae6e966f21)
- [crates.io ztty 0.5.2](https://crates.io/crates/ztty/0.5.2)
- [正式工作流与原始日志](https://github.com/LIghtJUNction/ztty/actions/runs/44712216777)
- [AUR ztty](https://aur.archlinux.org/packages/ztty)
- [Google 云盘报告目录](https://drive.google.com/drive/folders/1XwquftPqNH9H6fjScYw6FPpn5QdxZZxa)

