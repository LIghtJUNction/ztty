# zterm 锁文件传播

2026-11-20。Project Z 的 `resolve --all` 检查发现，zterm 独立仓库的已提交 `Cargo.lock` 仍选择 registry zgpui `0.1.2`。ztty 的根锁此前已使用 `0.1.5`，因此应用检查没有覆盖这个独立锁遗漏。

本次把 `crates/zterm` 更新到 [`cd9abc5`](https://github.com/LIghtJUNction/zterm/commit/cd9abc560d28c2d8ad6ed2073e8d9a7c27810da1)。该提交只改变锁文件中的 zgpui 版本和校验值，其余 47 个文件的 Git 身份完全相同。ztty 的依赖声明、根锁文件和应用实现保持当前主线内容。[机器可读结果](evidence.json)

验证使用基线 `d9d4ce40b10dd427ce4b59c901c8b03e2fc0ed70`。更新前后各执行一次 `cargo metadata --locked --offline --format-version 1`，完整输出相同：22 个包、22 个解析节点，zgpui `0.1.5`，特性列表为空。`git diff --check` 通过。

该基线保留刚合入的 GPU 文本缓存优化，其独立结果见[文本缓存报告](../../benchmarks/text-run-scratch-20261120/report.zh-CN.md)。本次没有重新运行界面或性能测试；此前的[依赖验收](../dependency-refresh-20261120/README.md)继续对应其原始源码与二进制。这里的结论仅是锁文件传播没有改变当前应用实现和依赖解析图。

正式标签和安装套件的制品锁保持原有版本。聚合仓库另行对新的固定提交执行 `sync`、`check`、`resolve --all` 和影响范围检查。
