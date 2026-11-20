# 依赖传播维护记录

更新：2026-11-20。本节登记本轮性能、运行时重载与标签切换修复的固定源码，以及安装套件和聚合仓库的接续条件。**本轮是源码更新，尚未形成新的正式发布。** 已发布的 ztty 0.5.0 对应此前的 `c4f8f48`，其发布记录已包含正式制品；本页下半部分保留的“0.5.0 发布前”记录只用于追溯历史。[0.5.0 固定发布记录](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/docs/reports/release-0.5.0-20261120/summary.json)

## 当前：性能与运行时重载源码更新

### 可复用的源码与实际消费关系

本轮 ztty 候选已经固定新的 zterm、zpty 源码。下表以提交内容为准，不依赖候选分支或 `main` 随后如何移动。ztty 的 `Cargo.toml` 使用这两个库的子模块路径，`Cargo.lock` 中的包版本应与这些固定源码一同解释。[依赖声明](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/Cargo.toml) · [锁文件](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/Cargo.lock) · [子模块固定点](https://github.com/LIghtJUNction/ztty/tree/f7e019654684ceae4b0cb64d47d651ebed99bc35/crates)

| 项目 | 本轮固定源码 | 源码中的包版本 | 本轮变更与消费位置 |
| --- | --- | --- | --- |
| zterm | [25b57fae7e544f0fe6762011508ba1438a26aca1](https://github.com/LIghtJUNction/zterm/commit/25b57fae7e544f0fe6762011508ba1438a26aca1) | [0.5.1](https://github.com/LIghtJUNction/zterm/blob/25b57fae7e544f0fe6762011508ba1438a26aca1/Cargo.toml) | 字符簇边界处理优化，以及 `Terminal::checkpoint` / `restore_checkpoint`；ztty 的 `crates/zterm` 固定到此提交。 |
| zpty | [519968d04c26bdbb00b3dedd9464609bfcf02b82](https://github.com/LIghtJUNction/zpty/commit/519968d04c26bdbb00b3dedd9464609bfcf02b82) | [0.2.2](https://github.com/LIghtJUNction/zpty/blob/519968d04c26bdbb00b3dedd9464609bfcf02b82/Cargo.toml) | Linux `Pty::from_raw_parts` 接管原 PTY 描述符、子进程身份和退出状态；ztty 的 `crates/zpty` 固定到此提交。 |
| ztty | [f7e019654684ceae4b0cb64d47d651ebed99bc35](https://github.com/LIghtJUNction/ztty/commit/f7e019654684ceae4b0cb64d47d651ebed99bc35) | [0.5.0](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/Cargo.toml) | 消费上述两个源码固定点，包含绘制调度优化、`ztty reload`、标签页准备好画面后再切换及相应回归检查。 |

随后已合入[剪贴板文本格式修正 `2aa561bbf51e894756cd4bc5fec00db63107bdd0`](https://github.com/LIghtJUNction/ztty/commit/2aa561bbf51e894756cd4bc5fec00db63107bdd0)，保持相同 zterm/zpty 固定点和包版本。它是本轮最终产品源码；上表 `f7e0196` 是完整性能测量所绑定的集成点。两者的二进制身份、实际验收与原始记录见[本轮报告](benchmarks/performance-reload-2026-11-20.md)。

**这些版本号没有为本轮新增 API 建立新的 registry 发布。** 新 API 的依据是上表中的源码：zterm 的[状态编码实现](https://github.com/LIghtJUNction/zterm/blob/25b57fae7e544f0fe6762011508ba1438a26aca1/src/terminal/checkpoint.rs)与 zpty 的[Linux PTY 接管实现](https://github.com/LIghtJUNction/zpty/blob/519968d04c26bdbb00b3dedd9464609bfcf02b82/src/linux.rs)。复现本轮组合必须使用匹配的源码固定点；不能因包版本仍显示 0.5.1、0.2.2 或 0.5.0，就把已有 registry 包或 0.5.0 应用制品认作本轮实现。

运行时重载通过同一 broker 进程重新执行新二进制，并保存原 PTY、文本终端状态和工作区布局。当前实现针对 Linux X11/XWayland 与无窗口会话；无法可靠保存的状态会拒绝重载并保留原会话。原生 Wayland、图形协议资源和不完整解析状态的限制，须随源码一起告知消费者。它们不能用“重新连接旧 broker”或“源码已更新”代替运行时升级。[重载实现](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/src/runtime_reload.rs) · [会话保存与恢复](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/src/session/reload.rs)

本记录用于依赖传播，不把测试脚本的存在当作验收结果。发布接续应保留本轮实际运行的源码、二进制哈希和结果，覆盖重载期间的进程与状态保留、失败后的会话可用性，以及软件/GPU 路径的标签切换画面和鼠标释放事件。对应门禁已进入候选源码：[运行时重载](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/tests/reload_runtime.py)、[连续像素采样](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/tests/tab_switch_pixels.py)、[鼠标释放](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/tests/tab_mouse_release.py)和[重绘检查](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/tests/repaint_gui.py)。性能数值和实际平台范围由本轮测量报告给出，不沿用下方历史快照中的验收结论。

### 实际影响范围与下游固定点

以 Project Z 的固定快照 [142dbe043bffd8fa1cf8aa9b1d55fe7422f08127](https://github.com/LIghtJUNction/project-z/tree/142dbe043bffd8fa1cf8aa9b1d55fe7422f08127) 的目录和 `repository_impact` 规则核对，`zterm`、`zpty`、`ztty` 的联合影响范围是 **oh-my-ztty、zpty、zterm、ztty**。其中包含变更源本身，以及 Cargo 和安装套件关系引入的下游。[固定目录](https://github.com/LIghtJUNction/project-z/blob/142dbe043bffd8fa1cf8aa9b1d55fe7422f08127/catalog.json) · [影响计算规则](https://github.com/LIghtJUNction/project-z/blob/142dbe043bffd8fa1cf8aa9b1d55fe7422f08127/scripts/project_z.py) · [套件关系声明](https://github.com/LIghtJUNction/project-z/blob/142dbe043bffd8fa1cf8aa9b1d55fe7422f08127/project-z.toml)

Project Z 已登记独立的 zterm、zpty 仓库；旧记录中的“尚未登记”属于历史状态。聚合快照仍固定此前的源码，尚未消费本轮更新；其四个相关 gitlink 如下。[固定源码目录](https://github.com/LIghtJUNction/project-z/blob/142dbe043bffd8fa1cf8aa9b1d55fe7422f08127/catalog.json)

| Project Z 路径 | 聚合快照中的固定提交 | 与本轮的关系 |
| --- | --- | --- |
| `zpty` | [66919bc6dcb7757e0437fe955bfa809fb1282b68](https://github.com/LIghtJUNction/zpty/commit/66919bc6dcb7757e0437fe955bfa809fb1282b68) | 尚未推进到本轮的 `519968d`。 |
| `zterm` | [a2074bac4c030cfd7c0b2514c852a89dcf54648f](https://github.com/LIghtJUNction/zterm/commit/a2074bac4c030cfd7c0b2514c852a89dcf54648f) | 尚未推进到本轮的 `25b57fa`。 |
| `ztty` | [c4f8f48c9598284a589e609f25242b49ceabfea8](https://github.com/LIghtJUNction/ztty/commit/c4f8f48c9598284a589e609f25242b49ceabfea8) | 对应已经发布的 0.5.0，尚未包含本轮候选。 |
| `oh-my-ztty` | [0fdc87873c53e7c87d18952f83a7b94fc4149de5](https://github.com/LIghtJUNction/oh-my-ztty/commit/0fdc87873c53e7c87d18952f83a7b94fc4149de5) | 聚合 pin 与下方单独审计的套件 main 快照不同；均不能视为本轮传播完成。 |

oh-my-ztty 的 main 审计快照是 [6d95c7eaa1a4ef27df2ae246edd7bddde5f3b31b](https://github.com/LIghtJUNction/oh-my-ztty/tree/6d95c7eaa1a4ef27df2ae246edd7bddde5f3b31b)。其 `components/ztty` 仍固定 `c4f8f48`，锁文件仍选择 `v0.5.0` 和以下两个正式制品；这些是旧发布的有效记录，不是本轮候选的资产。[组件固定点](https://github.com/LIghtJUNction/oh-my-ztty/tree/6d95c7eaa1a4ef27df2ae246edd7bddde5f3b31b/components) · [固定锁文件](https://github.com/LIghtJUNction/oh-my-ztty/blob/6d95c7eaa1a4ef27df2ae246edd7bddde5f3b31b/dependencies.lock.json)

| 平台 | 锁定制品 | SHA-256 |
| --- | --- | --- |
| Linux x86_64 | `v0.5.0/ztty-linux-x86_64.tar.gz` | `4b79ded489c37325ac049e4962e75c7ff773c2c5d073d8fbad154bb47dfed6b2` |
| Linux aarch64 | `v0.5.0/ztty-linux-aarch64.tar.gz` | `f86ddc20d6359c8301e7198adf80359c067ea02c28228139e2a39705a5ddd09d` |

### 尚未完成的传播与下一位维护者

**当前安装传播的实际阻碍，是尚无包含本轮改动的新正式 ztty 发布及其 Linux x86_64、aarch64 制品。** 套件要求 ztty 使用正式版本标签，取得两个架构的制品 URL 和 SHA-256 后才更新固定点；打包时还会检查 `components/ztty` 是否对应锁文件中的标签。因此，不能只把套件子模块推进到 `f7e0196` 而继续使用旧的 0.5.0 二进制锁。[套件接入约定](https://github.com/LIghtJUNction/oh-my-ztty/blob/6d95c7eaa1a4ef27df2ae246edd7bddde5f3b31b/CONTRIBUTING.md) · [固定版本工具](https://github.com/LIghtJUNction/oh-my-ztty/blob/6d95c7eaa1a4ef27df2ae246edd7bddde5f3b31b/scripts/workspace.py) · [打包一致性检查](https://github.com/LIghtJUNction/oh-my-ztty/blob/6d95c7eaa1a4ef27df2ae246edd7bddde5f3b31b/scripts/build_bundle.py)

| 接续阶段 | 下一位维护者 | 完成条件 |
| --- | --- | --- |
| 库与应用正式发布 | zterm、zpty 与 ztty 的发布维护者 | 为新增公共 API 确定适用的新 registry 版本，保持 ztty 的版本约束、子模块和锁文件一致；按正式发布流程交付包含本轮实现的 ztty 标签、Linux 两种架构制品及其 SHA-256，保留对应源码与原生验收结果。 |
| 安装套件消费 | oh-my-ztty 维护者 | 取得新正式发布后，一起更新 `components/ztty` 与 `dependencies.lock.json` 的标签、两个架构 URL 和 SHA-256；完成套件构建、安装与卸载、终端启动和配置保留验收，回传消费者提交。 |
| 聚合源码消费 | Project Z 聚合维护者 | 对齐根目录 zterm/zpty 与 ztty 内部相同库的固定点，接入已验证的 ztty 和套件提交；运行 `sync` 生成目录，再执行 `check`、`resolve --all` 和适用的下游验证，回传聚合提交与结果。 |

上述表格是接续条件，未把发布、通知或下游验收写成已完成。本轮提供的复用入口是上表三个固定源码提交；套件及聚合项保留待完成状态。聚合检查应继续遵守其[固定维护规则](https://github.com/LIghtJUNction/project-z/blob/142dbe043bffd8fa1cf8aa9b1d55fe7422f08127/AGENTS.md)与[源码对齐说明](https://github.com/LIghtJUNction/project-z/blob/142dbe043bffd8fa1cf8aa9b1d55fe7422f08127/README.md)，不以库发布或通知送达替代消费者实际接入。

## 历史快照：ztty 0.5.0 正式发布前

> 以下保留 [dfc9a639a58675ecec570586048c732f128557c2 中原有的维护记录](https://github.com/LIghtJUNction/ztty/blob/dfc9a639a58675ecec570586048c732f128557c2/docs/dependency-propagation.md)，仅将标题降一级。它描述的是 0.5.0 正式发布前的协调进度。原文中的“本轮”“当前”“尚未发布”“待验收”和目录缺口均指该历史阶段；其中“正式下载仍为 0.4.1”“0.5.0 仍为候选”等表述，不代表本页上半部分记录的现况。后来的 0.5.0 正式发布已有[固定证据](https://github.com/LIghtJUNction/ztty/blob/f7e019654684ceae4b0cb64d47d651ebed99bc35/docs/reports/release-0.5.0-20261120/summary.json)。

更新：2026-11-20（Asia/Taipei）。本轮独立库已发布，统一候选 `6053a97` 已合入共享 Wayland，核心版本/pin/lock 与本地完整检查已核对；**新 GUI、正式原生发布验收与 oh-my-ztty 传播尚未闭环**。本记录区分库发布、消费者验收和应用安装，后续回执应更新对应行。

### 维护约定

底层库 owner 负责把变更送到所有受影响的直接、间接消费者，包括库、应用、运行服务和安装套件。

1. 用 Project Z 工具枚举影响，再检查实际 manifest、源码 pin、功能开关和安装关系。新消费者或缺失关系必须补入目录。
2. 先联系现有 owner，明确每条边的接手人；在隔离工作树推进，保留他人的未提交改动。
3. 按依赖顺序更新兼容代码、版本约束、固定源码和锁文件。发布版本与消费者实际解析的源码必须对应。
4. 对固定提交执行适用的库、原生应用、服务或安装检查，分阶段提交、推送，并读回远端。
5. 关闭传播项时记录消费者提交、版本/pin/lock、验证范围和回执。发布通知或消息送达本身不构成完成；尚未完成的项保留实际阻碍与下一位 owner。

变更也需要适用性判断：如果只修发布工具，且工具不进入发布包、库实现和运行时依赖没有改变，可记录证据后保留消费者原 pin。此判断不能代替对包内容和依赖的核对。

### 本轮范围与发布点

“本轮负责”表示本次 ztty 迭代承担该库的传播；“本轮消费”表示我们更新自己的消费边，其他下游由现有 owner 接续。

| 库 | 本轮角色 | 已发布版本 | 固定发布源码 |
| --- | --- | --- | --- |
| zgpui | 本轮负责；root | [0.1.0](https://crates.io/crates/zgpui/0.1.0) | [1d796d1](https://github.com/LIghtJUNction/zgpui/commit/1d796d1fc63808dc099ed7b10c92e75835ae2814) |
| zgraphics | 本轮负责；graphics，root 负责发布 | [0.2.0](https://crates.io/crates/zgraphics/0.2.0) | [fe6357f](https://github.com/LIghtJUNction/zgraphics/commit/fe6357f98cf80fb6cc74739e45e8866ee17706ad) |
| zpty | 本轮负责；wayland，root 负责发布 | [0.2.2](https://crates.io/crates/zpty/0.2.2) | [66919bc](https://github.com/LIghtJUNction/zpty/commit/66919bc6dcb7757e0437fe955bfa809fb1282b68) |
| zterm | 本轮负责；graphics，root 负责发布 | [0.5.0](https://crates.io/crates/zterm/0.5.0) | [13d5e2c](https://github.com/LIghtJUNction/zterm/commit/13d5e2c74571d35cccd920b1204798211b70c79f) |
| zinput | 本轮消费 | [0.1.3](https://crates.io/crates/zinput/0.1.3) | [3761731](https://github.com/LIghtJUNction/zinput/commit/37617313dd7aaabcca9dd567344a76c4cbe03907) |
| zplugin / macros | 本轮消费 | [0.3.1](https://crates.io/crates/zplugin/0.3.1) | [00d566e](https://github.com/LIghtJUNction/zplugin/commit/00d566eedcff7b2932e90b9dfeb44910c783d305) |
| zsys | 本轮消费 | [0.1.1](https://crates.io/crates/zsys/0.1.1) | [808fcfd](https://github.com/LIghtJUNction/zsys/commit/808fcfdb73584ca7ca122e20b76f0209f56ab0f7) |

ztty 0.5.0 仍是应用候选；正式应用下载仍为 0.4.1。独立库源码继续存在于各自仓库，应用消费关系需要保留在目录中。

### 实际反向影响

本轮读取了 Project Z 的适用 `AGENTS.md` 和工具 help，执行：

```sh
python3 scripts/project_z.py impact --repositories <repository-or-package>
```

工具从 `.gitmodules`、`project-z.toml` 和 Cargo metadata 即时构图。`--repositories` 包含显式声明的运行时/安装套件关系；Cargo 图还包含开发依赖、可选功能和各目标平台的边。因此结果表示需要评估的范围，不能直接当作所有平台都启用的运行时依赖。

2026-11-19 23:08 UTC 的目录快照含 32 个仓库、53 个包。中央 HEAD 为 `32e5e82685e107b5b1f85a32d054922a0b8cbfa6`，但工作树有活动改动；仅该 HEAD 不能重建本次输入。七组原始命令结果与随后保存的单次 inventory 反向闭包一致。

#### 本轮负责的库

下表保留工具输出中的来源仓库自身。缺失的独立仓库身份见下一节。

| 输入 | 直接 Cargo 消费包 | 实际仓库结果 |
| --- | --- | --- |
| zgpui | zterm | oh-my-ztty, zgpui, ztty |
| zgraphics | zterm | oh-my-ztty, ztty |
| zpty | ztty | oh-my-ztty, ztty |
| zterm | ztty | oh-my-ztty, ztty |

#### 本轮消费的库

| 输入 | 直接消费关系 | 实际仓库结果 |
| --- | --- | --- |
| zinput | zterm、ztype | oh-my-ztty, zinput, ztty, ztype |
| zplugin | zinput/ztype 的可选功能；ztty 及共享 API；zharness、ssmcp、zcalld 及其插件；库内示例/合同测试 | oh-my-ztty, ssmcp, zcall-gui, zcalld, zcalld-compat, zharness, zinput, zplugin, ztty, ztype |
| zsys | zplugin；zuse-computer 的可选 std 路径 | oh-my-ztty, ssmcp, zcall-gui, zcalld, zcalld-compat, zharness, zinput, zplugin, zsys, ztty, ztype, zuse |

zcall-gui、zcalld-compat 和 oh-my-ztty 的部分影响来自应用或套件关系，不应改写为直接 Rust 链接依赖。其他 owner 的传播进展不计入本轮 ztty 已完成项。

### 目录登记缺口

本次只读盘点没有修改中央工作树。以下事项交由 Project Z 聚合 owner 与 root 协调：

| 状态 | 实际证据 | 下一步 |
| --- | --- | --- |
| 待登记 | `.gitmodules` 已有 zgpui、zinput，尚无独立 zgraphics、zpty、zterm | 登记三个独立仓库及其固定源码，避免继续把它们归属为 ztty 内部包 |
| 待更新 | 中央 ztty checkout 为 `8342d27720759bdd39354df41b0eeb413ffd10f5`，其中 zpty 仍为 0.2.1 | 接收 root 最终 ztty 提交后，更新聚合 gitlink 与包归属；不把旧快照当作最新验收 |
| 包边已有，仓库身份待修正 | zterm → zgpui/zgraphics/zinput；ztty → zterm/zpty 已在 Cargo 图中，但三个新库仍归 ztty | 完成登记与消费者 pin 更新后重新运行 impact，确认独立 zterm 能出现在结果中 |
| 套件边已登记，版本待传播 | `oh-my-ztty.requires` 已含 ztty、ssmcp、zshard、zdot | 保留套件边；收到正式应用制品后更新源码、制品锁和文档 |
| 聚合验收待完成 | 观察时 `catalog.json` 不存在，中央维护文件正在修改 | 由聚合 owner 运行 `sync`、`check`、`resolve --all`，提交推送后回传固定 SHA |

生成文件由 `python3 scripts/project_z.py sync` 维护，不能手改生成内容来掩盖缺失关系。影响图完成登记前，本表不声称覆盖所有实际下游。

### 已验证的库消费边

**zgpui / zgraphics / zinput → 独立 zterm 已验证。** graphics owner 从空 Cargo 缓存下载三个实际 registry 包，对照 Cargo.lock 的 SHA-256 并解析 `.cargo_vcs_info.json`，源码均与上表发布点一致。

zterm 固定源码 `13d5e2c` 的本地验收包含 158 个单测、3 个文档测试、14 个发布工具测试，以及 fmt、Clippy、Wasm、裸机和打包检查。[正式发布 44635656559](https://github.com/LIghtJUNction/zterm/actions/runs/44635656559) 在同一 SHA 上完成全部 8 个 job，包含三个操作系统、两项 no_std 检查、质量检查及发布准备/发布。它没有替代 ztty 应用验收。

[zterm 0.5.0 发布包](https://github.com/LIghtJUNction/zterm/releases/tag/v0.5.0) 为 75,791 字节、38 个成员；SHA-256：

```text
5cad31f851ec516590643444f339eb6e0a2b7b034b68d190172233e9392364b1
```

zgraphics 0.2.0 的[正式发布](https://github.com/LIghtJUNction/zgraphics/actions/runs/44630384752)与 zpty 0.2.2 的[正式发布](https://github.com/LIghtJUNction/zpty/actions/runs/44630374582)也已有回执。zpty 的平台验收还包含实际 capture 示例与 macOS spawn fixture。

### ztty 消费检查点与剩余 owner

早期原始证据保留了 23:10 UTC 的干净提交 `4aec236`。随后 root 已提交、推送 [7544a0d](https://github.com/LIghtJUNction/ztty/commit/7544a0d7a6d01bfe149d743ed3147baff188d67a)，完成独立 zterm 接入。GitHub 读回确认五个 gitlink 都指向上表发布点；[Cargo.toml](https://github.com/LIghtJUNction/ztty/blob/7544a0d7a6d01bfe149d743ed3147baff188d67a/Cargo.toml) 用根 patch 固定传递依赖，[Cargo.lock](https://github.com/LIghtJUNction/ztty/blob/7544a0d7a6d01bfe149d743ed3147baff188d67a/Cargo.lock)保持不变。root 回执的 metadata 每个核心 path 库恰好一份，zterm 的 28 个 Rust 源文件与实际 registry 包逐字一致。

统一候选 [6053a97](https://github.com/LIghtJUNction/ztty/commit/6053a97920c29c852cd92d8b96e35f62cff92ecb) 保留同样的五个发布 gitlink 和 Cargo.lock（SHA-256 `c4290a5f00aa4437ad31007dfcc0f35e582b4c4f62511ade176ff553fc34efa3`）；本次只读检查已再次核对。

| 消费边 | `6053a97` 的实际状态 | 后续 owner / 状态 |
| --- | --- | --- |
| zterm → zgpui / zgraphics / zinput | 版本为 0.1.0 / 0.2.0 / 0.1.3；根 patch 与三个固定 gitlink 保证使用同一源码 | 版本/pin/lock 已对应；root 负责最终应用验收 |
| ztty → zpty | 约束 0.2.2、gitlink `66919bc`、锁文件 0.2.2 一致 | 版本/pin/lock 已对应；wayland 的提取/库验收已交付 |
| ztty → zterm | gitlink 为正式 0.5.0/`13d5e2c`，已移出应用 workspace 成员 | 版本/pin/lock 已对应；root 负责完整应用 gate |
| Linux host / ztty-plugin-api / 当前示例 → zplugin | 统一为 `=0.3.1`；锁文件为单一 zplugin/macros 0.3.1 和 zsys 0.1.1 | root：新 GUI、正式原生发布与安装仍待完成；API 0.1.0 尚未首次发布 |

#### 统一候选与验收归属

共享 Wayland 已冻结为 [8b9f566](https://github.com/LIghtJUNction/ztty/commit/8b9f566c15ec3fb1020c4e2d96d41138c8cd2293)，进入 release 分支 [8b73651](https://github.com/LIghtJUNction/ztty/commit/8b7365122379b958942aedfb51c62419271698c2)，再经 `99a22c2` 合并到本轮候选 `6053a97`。此前缺少可合并 feature SHA 的阻碍已解除。[PR #18](https://github.com/LIghtJUNction/ztty/pull/18) 是接收路径；本次 GitHub 读回为 open/draft，head 为 `6053a97`、base 为 `8b73651`。尚未观察到现有 release owner 对接收和发布角色的明确 ACK，由 root 继续协调。

当前 Wayland 与 X11 共用 broker、PTY 管理和输入控制，支持 attach/MCP、混合后端会话、剪贴板/primary selection 与 text-input-v3。它仍需显式 `--backend wayland`；默认 X11，Wayland 拒绝 tab/split/workspace 容器。物理 GPU、实际桌面 IME 组合与 HiDPI/fractional scale 尚未实机验证。

| 验证或交付 | 已有证据 | 当前状态 / owner |
| --- | --- | --- |
| 共享 Wayland 实现 | [owner 报告](reports/native-wayland-shared-20261119/README.zh-CN.md)绑定 `8b9f566`；65 项 Wayland 检查属于 ELF `7fbaf9942d82c168bc08c54a472ccda8acd81c24f31d65558ebc48771a4dee74` | feature 已合并；旧 ELF 结果不代替 `6053a97` 验收 |
| `6053a97` 本地检查 | root 回执：默认 release 构建、默认/Wayland 严格 Clippy、`ZTTY_WAYLAND=1 BARE_METAL=1 make check` 通过；466 Rust 测试通过，6 项在该命令中 ignored；5 组原生服务预算 ASan/UBSan 通过，IME peer 已构建；同一候选的 6 项 EGL 专项随后实际全部通过（Mesa 软件 GL） | 本地完整 gate 与 EGL 专项已通过；不外推物理 GPU 性能 |
| 新整合 GUI 与正式原生发布 | 新 21 阶段 GUI（含 65 项 Wayland、14 项 Wayland CLI）和六架构正式原生门禁尚未取得通过回执 | root 与 wayland owner 接续；不借用旧候选结果 |
| `ztty-plugin-api` 0.1.0 | 当前源码、host 与示例依赖已统一；尚未首次 registry 发布 | root 与现有 release owner 确认首发源码和发布归属 |
| 新基准 | 本次 admission 因 load `16.886 > 12` 延后，实际 0 runs | root：等待满足资源条件再测；目前没有本候选性能结果 |
| ztty 0.5.0 应用及安装 | 仍为源码候选；正式下载保持 0.4.1 | root 与现有 release owner 完成接收、发布、安装回执后再交套件 owner |

较早 `7544a0d` 的本地/GUI 记录保留为该阶段证据，不能自动转记到新的合并源码。中央目录与外部消费者的以下观察仍是原快照；未取得新 owner 回执的条目保持待完成。

#### oh-my-ztty 的明确接手条件

该套件使用固定制品和源码。观察的 suite main 为 [8f86baa](https://github.com/LIghtJUNction/oh-my-ztty/commit/8f86baa04b49ea507f74b55439b1557d571d80e7)，`components/ztty` 固定 `6b91e29e923aaee0207953f2b04393087f19e0d8`；[dependencies.lock.json](https://github.com/LIghtJUNction/oh-my-ztty/blob/8f86baa04b49ea507f74b55439b1557d571d80e7/dependencies.lock.json)固定 ztty v0.4.1 的 x86_64/aarch64 下载地址和 SHA-256。构建脚本核对 gitlink 与锁定 release tag 一致。

`omztty update` 获取最新的**套件**发布；套件不会自动跟随 ztty 的最新开发提交。现有 suite 0.1.2 验收覆盖的是旧应用组合。

当前 oh-my-ztty owner 的活动分支为 `feat/unified-workspace-20261120`，协调记录首批提交为 `b57e998`；其 zshard/zdot/zrc 工作保持由该 owner 负责。新 ztty 消费项仍待 root 提供：

- 正式应用版本、完整源码 SHA、递归子模块 pins。
- Linux 两架构制品及校验和、归档布局、glibc 下限与共享库要求。
- CLI/配置契约变化，以及最终应用验收范围。

套件 owner 随后一起更新 gitlinks、制品锁和文档，完成原生图形、安装/重复安装/失败回滚/卸载及公开下载制品检查，并回传已推提交。**此项待完成，接手人为当前 oh-my-ztty owner，交付方为 root。**

#### 其他下游的 owner 回执

这些是现有 owner 的状态记录；本轮没有修改其工作树，也没有代替其应用验收。

| 下游 | 已取得的实际回执或观察 | 仍待谁完成 |
| --- | --- | --- |
| zharness → ssmcp | ssmcp 记录了 zharness 0.5.1/`e3580c01` 与 zplugin 0.3.1/`00d566ee` 的匹配点；其部分消费者提交 [9c13277](https://github.com/LIghtJUNction/ssmcp/commit/9c13277d1eacb68c2378f692a4d4660cb92f0c62)已推，不能当作完整四包组合 | Project Z 的 `coord/shared-cohort-20261120` owner 负责共享依赖组合；SSMCP owner 负责最终应用、PTY 和桌面验收 |
| ssmcp | 中央 checkout `b49da12` 仍声明 zplugin =0.3.0、zharness =0.5.0；协调记录明确新组合仍在独立分支 | 原 SSMCP owner / 聚合协调 owner 回传最终版本、pin/lock、测试和发布结果；不由 ztty 分支替换 |
| ztype | 中央 checkout `657f7cf` 声明 zinput =0.1.3、可选 zplugin =0.3.1 | 当前 ztype / Project Z owner 回传消费者验收与已推提交；本次未取得该边最终回执 |
| zcalld / zcall-gui / zcalld-compat | `zcalld.md` 最新记录已接收 zplugin 0.3.1/zsys 0.1.1，并分别记录核心、GUI、兼容扩展的进行中工作 | 现有 zcalld/GUI owner 回传最终整合和发布回执；其要求的 zcalld → zcall-gui 可选源码套件边由聚合 owner 登记 |
| zuse | owner 已回报 zuse-computer 0.2.0 发布源码 [557ea46](https://github.com/LIghtJUNction/zuse/commit/557ea46ae70e9de8f93b0928b505132e8c94e9db)；中央仍固定 0.1.1/`3d7cb7e` | 当前 zuse owner 负责其传播，聚合 owner 更新适用 pins；其他消费者不能凭发布通知视作已迁移 |

zinput、zplugin、zsys 的其他外部消费边由各自当前库 owner 与以上应用 owner 协调。缺少最终回执的边保持待完成，不归入 ztty 的完成统计。

### 发布工具修正的适用性记录

两库曾在全新缓存中执行离线锁文件生成失败，退出码为 101。修复先 `cargo fetch --locked`，再改候选版本并离线生成锁文件：

| 仓库 | 工具维护提交 | 保留的发布源码 |
| --- | --- | --- |
| zgraphics | [b296a58](https://github.com/LIghtJUNction/zgraphics/commit/b296a58f09d6052f13c86dc09e9e7b7f7e87438f) | 0.2.0 仍为 `fe6357f` |
| zterm | [13d5e2c](https://github.com/LIghtJUNction/zterm/commit/13d5e2c74571d35cccd920b1204798211b70c79f) | 正式 0.5.0 使用该提交 |

GitHub 读回确认每仓仅修改 `scripts/bump_version.py` 和 `scripts/release_tests.py`。库源码、Cargo.toml、Cargo.lock 无变化，scripts 不在 crate 中；真实空缓存 patch 探针成功，registry 版本与校验和不变，各 14 项工具测试通过。探针中的 0.5.1/0.2.1 只属于隔离测试 fixture，不是新发布版本。

**该工具维护子项已验证，无需把 zgraphics 消费者 pin 从已发布的 `fe6357f` 改到 `b296a58`。** zterm 的正式发布仍按完整的新候选 SHA 验证。此结论只适用于已核对的工具改动，不适用于未来库实现或依赖变化。

### 证据位置与后续更新

原始输出保存在整合工作树的 ignored 目录：

```text
artifacts/dependency-propagation-20261120/
```

目录包含七个 `impact-*.repositories.txt`、工具 help、目录输入哈希与内部依赖 inventory、ztty manifest/lock/gitlink 快照、oh-my-ztty 消费方式、四个现有 owner 协调文件副本、发布工具探针与 zterm registry 来源证明；`SHA256.json` 校验各文件。协调文件来自 `/tmp/project-z-coordination-20261120`，后面的更新可取代早期状态。

root 收到最终回执后，在本记录补充 ztty 消费者 SHA、实际依赖解析、完整 gate/发布/安装结果，以及 oh-my-ztty 和 Project Z 聚合仓库的最终提交。届时重新运行影响枚举，确认新仓库和新下游均已登记，再关闭对应项。
