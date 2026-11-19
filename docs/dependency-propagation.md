# 依赖传播维护记录

更新：2026-11-20（Asia/Taipei）。本轮独立库已发布，ztty 的核心版本/pin/lock 已对应；**最终应用验收与 oh-my-ztty 传播尚未闭环**。本记录区分库发布、消费者验收和应用安装，后续回执应更新对应行。

## 维护约定

底层库 owner 负责把变更送到所有受影响的直接、间接消费者，包括库、应用、运行服务和安装套件。

1. 用 Project Z 工具枚举影响，再检查实际 manifest、源码 pin、功能开关和安装关系。新消费者或缺失关系必须补入目录。
2. 先联系现有 owner，明确每条边的接手人；在隔离工作树推进，保留他人的未提交改动。
3. 按依赖顺序更新兼容代码、版本约束、固定源码和锁文件。发布版本与消费者实际解析的源码必须对应。
4. 对固定提交执行适用的库、原生应用、服务或安装检查，分阶段提交、推送，并读回远端。
5. 关闭传播项时记录消费者提交、版本/pin/lock、验证范围和回执。发布通知或消息送达本身不构成完成；尚未完成的项保留实际阻碍与下一位 owner。

变更也需要适用性判断：如果只修发布工具，且工具不进入发布包、库实现和运行时依赖没有改变，可记录证据后保留消费者原 pin。此判断不能代替对包内容和依赖的核对。

## 本轮范围与发布点

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

## 实际反向影响

本轮读取了 Project Z 的适用 `AGENTS.md` 和工具 help，执行：

```sh
python3 scripts/project_z.py impact --repositories <repository-or-package>
```

工具从 `.gitmodules`、`project-z.toml` 和 Cargo metadata 即时构图。`--repositories` 包含显式声明的运行时/安装套件关系；Cargo 图还包含开发依赖、可选功能和各目标平台的边。因此结果表示需要评估的范围，不能直接当作所有平台都启用的运行时依赖。

2026-11-19 23:08 UTC 的目录快照含 32 个仓库、53 个包。中央 HEAD 为 `32e5e82685e107b5b1f85a32d054922a0b8cbfa6`，但工作树有活动改动；仅该 HEAD 不能重建本次输入。七组原始命令结果与随后保存的单次 inventory 反向闭包一致。

### 本轮负责的库

下表保留工具输出中的来源仓库自身。缺失的独立仓库身份见下一节。

| 输入 | 直接 Cargo 消费包 | 实际仓库结果 |
| --- | --- | --- |
| zgpui | zterm | oh-my-ztty, zgpui, ztty |
| zgraphics | zterm | oh-my-ztty, ztty |
| zpty | ztty | oh-my-ztty, ztty |
| zterm | ztty | oh-my-ztty, ztty |

### 本轮消费的库

| 输入 | 直接消费关系 | 实际仓库结果 |
| --- | --- | --- |
| zinput | zterm、ztype | oh-my-ztty, zinput, ztty, ztype |
| zplugin | zinput/ztype 的可选功能；ztty 及共享 API；zharness、ssmcp、zcalld 及其插件；库内示例/合同测试 | oh-my-ztty, ssmcp, zcall-gui, zcalld, zcalld-compat, zharness, zinput, zplugin, ztty, ztype |
| zsys | zplugin；zuse-computer 的可选 std 路径 | oh-my-ztty, ssmcp, zcall-gui, zcalld, zcalld-compat, zharness, zinput, zplugin, zsys, ztty, ztype, zuse |

zcall-gui、zcalld-compat 和 oh-my-ztty 的部分影响来自应用或套件关系，不应改写为直接 Rust 链接依赖。其他 owner 的传播进展不计入本轮 ztty 已完成项。

## 目录登记缺口

本次只读盘点没有修改中央工作树。以下事项交由 Project Z 聚合 owner 与 root 协调：

| 状态 | 实际证据 | 下一步 |
| --- | --- | --- |
| 待登记 | `.gitmodules` 已有 zgpui、zinput，尚无独立 zgraphics、zpty、zterm | 登记三个独立仓库及其固定源码，避免继续把它们归属为 ztty 内部包 |
| 待更新 | 中央 ztty checkout 为 `8342d27720759bdd39354df41b0eeb413ffd10f5`，其中 zpty 仍为 0.2.1 | 接收 root 最终 ztty 提交后，更新聚合 gitlink 与包归属；不把旧快照当作最新验收 |
| 包边已有，仓库身份待修正 | zterm → zgpui/zgraphics/zinput；ztty → zterm/zpty 已在 Cargo 图中，但三个新库仍归 ztty | 完成登记与消费者 pin 更新后重新运行 impact，确认独立 zterm 能出现在结果中 |
| 套件边已登记，版本待传播 | `oh-my-ztty.requires` 已含 ztty、ssmcp、zshard、zdot | 保留套件边；收到正式应用制品后更新源码、制品锁和文档 |
| 聚合验收待完成 | 观察时 `catalog.json` 不存在，中央维护文件正在修改 | 由聚合 owner 运行 `sync`、`check`、`resolve --all`，提交推送后回传固定 SHA |

生成文件由 `python3 scripts/project_z.py sync` 维护，不能手改生成内容来掩盖缺失关系。影响图完成登记前，本表不声称覆盖所有实际下游。

## 已验证的库消费边

**zgpui / zgraphics / zinput → 独立 zterm 已验证。** graphics owner 从空 Cargo 缓存下载三个实际 registry 包，对照 Cargo.lock 的 SHA-256 并解析 `.cargo_vcs_info.json`，源码均与上表发布点一致。

zterm 固定源码 `13d5e2c` 的本地验收包含 158 个单测、3 个文档测试、14 个发布工具测试，以及 fmt、Clippy、Wasm、裸机和打包检查。[正式发布 44635656559](https://github.com/LIghtJUNction/zterm/actions/runs/44635656559) 在同一 SHA 上完成全部 8 个 job，包含三个操作系统、两项 no_std 检查、质量检查及发布准备/发布。它没有替代 ztty 应用验收。

[zterm 0.5.0 发布包](https://github.com/LIghtJUNction/zterm/releases/tag/v0.5.0) 为 75,791 字节、38 个成员；SHA-256：

```text
5cad31f851ec516590643444f339eb6e0a2b7b034b68d190172233e9392364b1
```

zgraphics 0.2.0 的[正式发布](https://github.com/LIghtJUNction/zgraphics/actions/runs/44630384752)与 zpty 0.2.2 的[正式发布](https://github.com/LIghtJUNction/zpty/actions/runs/44630374582)也已有回执。zpty 的平台验收还包含实际 capture 示例与 macOS spawn fixture。

## ztty 消费检查点与剩余 owner

早期原始证据保留了 23:10 UTC 的干净提交 `4aec236`。随后 root 已提交、推送 [7544a0d](https://github.com/LIghtJUNction/ztty/commit/7544a0d7a6d01bfe149d743ed3147baff188d67a)，完成独立 zterm 接入。GitHub 读回确认五个 gitlink 都指向上表发布点；[Cargo.toml](https://github.com/LIghtJUNction/ztty/blob/7544a0d7a6d01bfe149d743ed3147baff188d67a/Cargo.toml) 用根 patch 固定传递依赖，[Cargo.lock](https://github.com/LIghtJUNction/ztty/blob/7544a0d7a6d01bfe149d743ed3147baff188d67a/Cargo.lock)保持不变。root 回执的 metadata 每个核心 path 库恰好一份，zterm 的 28 个 Rust 源文件与实际 registry 包逐字一致。

| 消费边 | `7544a0d` 的实际状态 | 后续 owner / 状态 |
| --- | --- | --- |
| zterm → zgpui / zgraphics / zinput | 版本为 0.1.0 / 0.2.0 / 0.1.3；根 patch 与三个固定 gitlink 保证使用同一源码 | 版本/pin/lock 已对应；root 负责最终应用验收 |
| ztty → zpty | 约束 0.2.2、gitlink `66919bc`、锁文件 0.2.2 一致 | 版本/pin/lock 已对应；wayland 的提取/库验收已交付 |
| ztty → zterm | gitlink 为正式 0.5.0/`13d5e2c`，已移出应用 workspace 成员 | 版本/pin/lock 已对应；root 负责完整应用 gate |
| Linux host / ztty-plugin-api → zplugin | 两处精确约束 `=0.3.1`；锁文件为单一 zplugin/macros 0.3.1 和 zsys 0.1.1 | root：在最终应用提交上完成原生、MCP、GUI 和发布/安装验收 |

完整应用 gate 暂待分支协调。另一个活动 owner 的 `release/0.5.0` 已到 [80888ee](https://github.com/LIghtJUNction/ztty/commit/80888eede7d9f5c5bb5b62a2acc21680691eec72)；root/graphics 正在对账其 Wayland、IME 等工作。这里的单窗口 Wayland 描述只属于 `iter/resume-20261120` 检查点，不能据此限定最终合并版的能力。现有 owner 的工作树和发布职责继续保留。

### oh-my-ztty 的明确接手条件

该套件使用固定制品和源码。观察的 suite main 为 [8f86baa](https://github.com/LIghtJUNction/oh-my-ztty/commit/8f86baa04b49ea507f74b55439b1557d571d80e7)，`components/ztty` 固定 `6b91e29e923aaee0207953f2b04393087f19e0d8`；[dependencies.lock.json](https://github.com/LIghtJUNction/oh-my-ztty/blob/8f86baa04b49ea507f74b55439b1557d571d80e7/dependencies.lock.json)固定 ztty v0.4.1 的 x86_64/aarch64 下载地址和 SHA-256。构建脚本核对 gitlink 与锁定 release tag 一致。

`omztty update` 获取最新的**套件**发布；套件不会自动跟随 ztty 的最新开发提交。现有 suite 0.1.2 验收覆盖的是旧应用组合。

当前 oh-my-ztty owner 的活动分支为 `feat/unified-workspace-20261120`，协调记录首批提交为 `b57e998`；其 zshard/zdot/zrc 工作保持由该 owner 负责。新 ztty 消费项仍待 root 提供：

- 正式应用版本、完整源码 SHA、递归子模块 pins。
- Linux 两架构制品及校验和、归档布局、glibc 下限与共享库要求。
- CLI/配置契约变化，以及最终应用验收范围。

套件 owner 随后一起更新 gitlinks、制品锁和文档，完成原生图形、安装/重复安装/失败回滚/卸载及公开下载制品检查，并回传已推提交。**此项待完成，接手人为当前 oh-my-ztty owner，交付方为 root。**

### 其他下游的 owner 回执

这些是现有 owner 的状态记录；本轮没有修改其工作树，也没有代替其应用验收。

| 下游 | 已取得的实际回执或观察 | 仍待谁完成 |
| --- | --- | --- |
| zharness → ssmcp | ssmcp 记录了 zharness 0.5.1/`e3580c01` 与 zplugin 0.3.1/`00d566ee` 的匹配点；其部分消费者提交 [9c13277](https://github.com/LIghtJUNction/ssmcp/commit/9c13277d1eacb68c2378f692a4d4660cb92f0c62)已推，不能当作完整四包组合 | Project Z 的 `coord/shared-cohort-20261120` owner 负责共享依赖组合；SSMCP owner 负责最终应用、PTY 和桌面验收 |
| ssmcp | 中央 checkout `b49da12` 仍声明 zplugin =0.3.0、zharness =0.5.0；协调记录明确新组合仍在独立分支 | 原 SSMCP owner / 聚合协调 owner 回传最终版本、pin/lock、测试和发布结果；不由 ztty 分支替换 |
| ztype | 中央 checkout `657f7cf` 声明 zinput =0.1.3、可选 zplugin =0.3.1 | 当前 ztype / Project Z owner 回传消费者验收与已推提交；本次未取得该边最终回执 |
| zcalld / zcall-gui / zcalld-compat | `zcalld.md` 最新记录已接收 zplugin 0.3.1/zsys 0.1.1，并分别记录核心、GUI、兼容扩展的进行中工作 | 现有 zcalld/GUI owner 回传最终整合和发布回执；其要求的 zcalld → zcall-gui 可选源码套件边由聚合 owner 登记 |
| zuse | owner 已回报 zuse-computer 0.2.0 发布源码 [557ea46](https://github.com/LIghtJUNction/zuse/commit/557ea46ae70e9de8f93b0928b505132e8c94e9db)；中央仍固定 0.1.1/`3d7cb7e` | 当前 zuse owner 负责其传播，聚合 owner 更新适用 pins；其他消费者不能凭发布通知视作已迁移 |

zinput、zplugin、zsys 的其他外部消费边由各自当前库 owner 与以上应用 owner 协调。缺少最终回执的边保持待完成，不归入 ztty 的完成统计。

## 发布工具修正的适用性记录

两库曾在全新缓存中执行离线锁文件生成失败，退出码为 101。修复先 `cargo fetch --locked`，再改候选版本并离线生成锁文件：

| 仓库 | 工具维护提交 | 保留的发布源码 |
| --- | --- | --- |
| zgraphics | [b296a58](https://github.com/LIghtJUNction/zgraphics/commit/b296a58f09d6052f13c86dc09e9e7b7f7e87438f) | 0.2.0 仍为 `fe6357f` |
| zterm | [13d5e2c](https://github.com/LIghtJUNction/zterm/commit/13d5e2c74571d35cccd920b1204798211b70c79f) | 正式 0.5.0 使用该提交 |

GitHub 读回确认每仓仅修改 `scripts/bump_version.py` 和 `scripts/release_tests.py`。库源码、Cargo.toml、Cargo.lock 无变化，scripts 不在 crate 中；真实空缓存 patch 探针成功，registry 版本与校验和不变，各 14 项工具测试通过。探针中的 0.5.1/0.2.1 只属于隔离测试 fixture，不是新发布版本。

**该工具维护子项已验证，无需把 zgraphics 消费者 pin 从已发布的 `fe6357f` 改到 `b296a58`。** zterm 的正式发布仍按完整的新候选 SHA 验证。此结论只适用于已核对的工具改动，不适用于未来库实现或依赖变化。

## 证据位置与后续更新

原始输出保存在整合工作树的 ignored 目录：

```text
artifacts/dependency-propagation-20261120/
```

目录包含七个 `impact-*.repositories.txt`、工具 help、目录输入哈希与内部依赖 inventory、ztty manifest/lock/gitlink 快照、oh-my-ztty 消费方式、四个现有 owner 协调文件副本、发布工具探针与 zterm registry 来源证明；`SHA256.json` 校验各文件。协调文件来自 `/tmp/project-z-coordination-20261120`，后面的更新可取代早期状态。

root 收到最终回执后，在本记录补充 ztty 消费者 SHA、实际依赖解析、完整 gate/发布/安装结果，以及 oh-my-ztty 和 Project Z 聚合仓库的最终提交。届时重新运行影响枚举，确认新仓库和新下游均已登记，再关闭对应项。
