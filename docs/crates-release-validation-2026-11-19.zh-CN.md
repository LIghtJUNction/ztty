# 四个基础库的发布与实际消费者验证

记录时间：2026-11-19 00:30（Asia/Taipei）。发布源码：[74a5994](https://github.com/LIghtJUNction/ztty/commit/74a599445fa358bbe7d1fdb71e923a97fe398467)。

## 发布结果

四个库均已通过正常 Cargo 发布验证并上传至 crates.io。随后从官方注册表重新查询版本、下载归档，核对了注册表校验和、本地发布包校验和及包内 `.cargo_vcs_info.json`。四份归档都对应上述源码提交。

| 库 | 已发布版本 | 官方页面 |
| --- | --- | --- |
| zinput | 0.1.1 | [crates.io](https://crates.io/crates/zinput/0.1.1) |
| zgraphics | 0.1.1 | [crates.io](https://crates.io/crates/zgraphics/0.1.1) |
| zpty | 0.2.1 | [crates.io](https://crates.io/crates/zpty/0.2.1) |
| zterm | 0.3.0 | [crates.io](https://crates.io/crates/zterm/0.3.0) |

zterm 的公共 Cell 类型增加字段，因此提升了 0.x 的次版本，避免把公共接口变化隐藏在补丁版本里。工作区内各库的最低依赖版本已同步；变更见 [PR #10](https://github.com/LIghtJUNction/ztty/pull/10)。

## 消费者验证

消费者依赖使用精确的 crates.io 版本，没有使用本地路径或 Git 依赖替代注册表归档。四个单库消费者及一个组合消费者均通过：

| 检查 | 通过数量 | 验证内容 |
| --- | ---: | --- |
| Cargo metadata | 5 | 实际依赖源、版本与归档源码提交 |
| no_std 库编译 | 5 | 消费者库在 no_std 下使用公开接口 |
| 可执行消费者运行 | 5 | 实际调用库接口；zpty 创建并操作真实 PTY |

全部 15 条 Cargo 命令通过，整轮耗时 283.92 秒。可执行测试程序使用 std；no_std 结论来自独立库编译检查。本轮运行平台为 Linux，不代表已在所有目标平台完成消费者运行。

原始完整报告 SHA-256 为 `a52a192ae8a37762eb72c2f72825883edf51636ae0235c0688aae5e81d63d96b`。版本、四份归档校验和与验证摘要见[机器可读记录](crates-release-validation-2026-11-19.json)。

## 共享编译队列与超时处理

早期两次验证因开发机共享编译队列和单命令 90 秒上限而未完成，记录保留。验证工具新增 `--cargo-timeout SECONDS`，默认仍是 90 秒，只接受正的有限数值，并在报告中记录实际预算。本轮显式使用 1,800 秒，源码、版本、no_std 和实际运行检查全部保留；工作站全局编译包装器未修改。

[PR #12](https://github.com/LIghtJUNction/ztty/pull/12) 包含该改动以及 metadata、编译、运行和组合消费者超时路径的回归测试。五项 Python 测试、Python 编译及补丁检查通过。该超时配置不改变注册表归档的代码。

这份记录覆盖四个基础库。ztty 应用本体、原生安装包与 GitHub Release 使用各自的最终源码和发布验收记录。
