# 默认分支依赖与共享构建目录验收

日期：2026-11-21（Asia/Taipei）。应用版本：`0.5.2`。本轮完成 Git 依赖来源统一、共享构建目录适配，以及插件和热重载的本地验收。完整机器可读记录见 [results.json](results.json)。

## 为什么修改

此前主程序混用注册表依赖、跨仓库路径依赖和本地 patch，插件又单独解析依赖。库的默认分支即使修复了问题，应用构建也可能使用另一份源码。现在跨仓库 Project Z 依赖统一指向规范的 GitHub 默认分支，由 `Cargo.lock` 记录本次验证的精确提交；同仓库的 `ztty-plugin-api` 仍使用本地路径。[主清单](../../../Cargo.toml) · [插件 API 清单](../../../crates/ztty-plugin-api/Cargo.toml) · [示例清单](../../../examples/dedent-plugin/Cargo.toml)

开发机使用共享 Cargo 输出目录。Makefile 的安装入口、检查脚本和插件构建工具此前假定产物在仓库自己的 `target/release` 下，可能找不到刚编译的文件。现在它们读取 Cargo 的 `compiler-artifact` 消息，取得本次构建实际产物；构建失败或缺少目标产物时停止，不回退到旧文件。[Makefile](../../../Makefile) · [检查脚本](../../../tools/check.sh) · [插件构建工具](../../../tests/build_text_plugins.py) · [安装回归测试](../../../tests/install_test.py)

## 固定源码

下表九个包来自八个仓库。验证结束后逐一查询远端默认分支，确认其提交与解析图一致；五个独立库的子模块固定点也与应用锁文件一致。示例和测试插件的 `zplugin`、`zplugin-macros`、`zsys` 与应用使用相同来源。[锁文件](../../../Cargo.lock) · [来源审计](results.json)

| 包 | 包版本 | 本轮默认分支提交 |
| --- | --- | --- |
| `zgpui` | `0.1.5` | [b4d5493](https://github.com/LIghtJUNction/zgpui/commit/b4d5493e0cd985d00558c7be0b5726965ab5163a) |
| `zgraphics` | `0.2.0` | [aa0ed19](https://github.com/LIghtJUNction/zgraphics/commit/aa0ed19fb706ad147aff1df54c4ef762d918e864) |
| `zinput` | `0.1.3` | [fe1cbae](https://github.com/LIghtJUNction/zinput/commit/fe1cbaec744fa790a043383d45f612b8d19dcd32) |
| `zplugin` | `0.3.1` | [cf02c1d](https://github.com/LIghtJUNction/zplugin/commit/cf02c1d9db575b92b36297dfe1fde72de9ee51e4) |
| `zplugin-macros` | `0.3.1` | [cf02c1d](https://github.com/LIghtJUNction/zplugin/commit/cf02c1d9db575b92b36297dfe1fde72de9ee51e4) |
| `zpty` | `0.2.3` | [d263f9a](https://github.com/LIghtJUNction/zpty/commit/d263f9a8f0881d259e46c51a7b6753c61ab9be59) |
| `zsys` | `0.1.1` | [808fcfd](https://github.com/LIghtJUNction/zsys/commit/808fcfdb73584ca7ca122e20b76f0209f56ab0f7) |
| `zterm` | `0.5.2` | [1a76133](https://github.com/LIghtJUNction/zterm/commit/1a761334f0b9cf26da700426a5b82778f0d48c3d) |
| `zvte` | `0.1.3` | [b4e50a5](https://github.com/LIghtJUNction/zvte/commit/b4e50a5cde476fb5303060f3ecbc9200ccea5cc0) |

`zgraphics` 的查询状态与图像清除修复已快进到默认分支。`zterm` 把已有的查询状态修复合入当前默认分支，并保留此前的 Git 依赖迁移：只有能力查询时允许保存终端状态，有无法恢复的图形状态时继续拒绝。这次工作完成这些已有修复的默认分支传播。[zgraphics 修复](https://github.com/LIghtJUNction/zgraphics/commit/aa0ed19fb706ad147aff1df54c4ef762d918e864) · [zterm 锁文件与默认分支](https://github.com/LIghtJUNction/zterm/commit/1a761334f0b9cf26da700426a5b82778f0d48c3d)

## 验证结果

本轮在 `a43aebb17a11b8c25f2c970d0d36a43f67082f9f` 基线加上本报告所在提交的实现差异上运行验证；`results.json` 记录被测清单、锁文件与脚本的 SHA-256。文档在代码验证完成后补入。受控 Cargo 批次持有共享输出目录的 owner lock，复用现有缓存。

| 检查 | 实际结果 |
| --- | --- |
| `ZTTY_WAYLAND=1 ZTTY_BARE_METAL=1 sh tools/check.sh` | 退出码 0；格式、严格 Clippy、Wayland 构建、无 Rust std 符号检查、裸机 zterm 检查、MCP 与清理检查通过 |
| Rust 单元测试与文档测试 | 530 通过，0 失败，9 忽略 |
| 额外 SessionCore 检查 | 10 通过，所有测试子进程已回收 |
| 安装测试 | 20 通过，覆盖共享目录、带空格路径、构建失败和缺失目标产物 |
| 插件 | 4 个动态库构建成功；22 项 CLI、接口兼容与生命周期检查通过 |
| 软件渲染热重载 | 12 次尝试：6 次实际同 PID exec 成功，6 次按预期拒绝 |
| GPU 路径热重载 | 12 次尝试：6 次实际同 PID exec 成功，6 次按预期拒绝 |

9 项忽略测试包括一次完整 Unicode 标量扫描，以及 8 项需显式运行的 EGL/原生字体专项测试；本轮没有执行它们，未计入通过数。独立库还分别完成格式、严格 Clippy 和测试：`zgraphics` 为 58 个单元测试及 1 个文档测试；`zterm` 为 193 个单元测试及 3 个文档测试，另有 1 项忽略，并通过 `x86_64-unknown-none` 检查。[逐项结果与忽略名称](results.json)

MCP、插件 CLI 和 GUI 检查使用同一份 ztty ELF。其 SHA-256 为：

```text
239aa74c8952d272832188ccde80c4805505faf892bb4247904aaafbba1e1e8c
```

热重载的每次尝试都检查 broker PID、子进程与 PTY 身份、终端内容、实际像素以及后续输入输出。软件与 GPU 路径均保留这些状态，测试所属进程均已清理。GPU 路径运行在私有 Xvfb 和 Mesa `llvmpipe (LLVM 23.1.5, 256 bits)` 上，属于 Linux 图形集成验证，不是物理 GPU 性能测量或其他操作系统验收。[重载逐项记录](results.json)

已查看 GPU 路径的 `after-query-checks.png`：720×460 画面左上角的 `QUERY-ONLY-SHELL` 完整可见，未观察到裁切或图形残留。两种渲染路径各自的前后截图哈希一致。完整日志、ELF、动态库和截图保留在被忽略的 `artifacts/default-branch-validation/` 中，Git 仅保存文本记录。

## 下游接续

本轮完成 `zgraphics → zterm → ztty` 的源码传播。另两个消费方由已有整合任务接续：

| 消费方 | 当前状态与实际阻碍 | 接续责任与完成条件 |
| --- | --- | --- |
| oh-my-ztty | 观察到 main 比 origin/main 超前 5 个提交，多个组件固定点与依赖锁已有未提交改动 | 现有 oh-my-ztty 整合任务接收本轮 ztty 提交，运行 `python3 scripts/workspace.py sync ztty`、`check` 及套件自身构建验收 |
| Project Z | 管理工作树 `28257a0` 的实时 impact 命令因 `z58` 尚未初始化而停止 | 现有 Project Z 协调任务初始化缺失子模块，刷新 zgraphics/zterm/ztty 固定点，再完成 `sync`、`check`、`resolve --all` |

为明确影响范围，在相同管理提交的已提交 `catalog.json` 上调用原有 `repository_impact`，得到 `oh-my-ztty、zgraphics、zterm、ztty`。该结果只表示已提交目录声明的依赖关系，不代表实时工作树检查通过。[影响计算与原始阻碍](results.json)

本轮更新的是版本号仍为 `0.5.2` 的开发源码；没有发布新版本或 crate，也没有更新桌面安装。已有发布与安装记录保留各自的来源和产物身份。[依赖传播记录](../../dependency-propagation.md) · [发布说明](../../releasing.md)
