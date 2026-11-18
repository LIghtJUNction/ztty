# 发布资产保留与回读验证记录

记录日期：2026-11-19（Asia/Taipei）。发布资产的永久修复源码为 [`99d9463`](https://github.com/LIghtJUNction/ztty/commit/99d94633391150c4ca431b568c27139ef63441dd)。本记录确认证据打包与回读助手的七项测试、Python AST、YAML 和发布 Shell 语法检查通过，并明确区分测试覆盖与之后追加的静态复核。它不报告一次实际 GitHub Release 发布成功。

## 修复后的行为

**空日志和嵌套文件保留原始字节。** `tools/release_assets.py` 将证据目录中每个普通文件的相对路径、长度、SHA-256 和内容写入非空 ZIP；零字节日志也作为条目保留。发布时只把非空顶层文件、ZIP 和其校验记录作为独立附件上传，嵌套文件保存在 ZIP 内。这让 GitHub 附件上传获得非空请求，同时保持平台证据账本中的原始文件集合。

**草稿必须通过下载回读后才能公开。** 新发布先创建草稿，再把附件下载到独立目录。下载的 ZIP 必须与本地待上传 ZIP 逐字节相同；助手继续验证版本、源码 SHA、归档条目及其内容，并逐字节核对非空顶层附件。所有这些检查通过后，才补回账本需要的顶层零字节文件。工作流还核对校验记录、源码包校验值、安装脚本和六个平台的构建账本，最后才把草稿设为公开。[发布步骤源码](https://github.com/LIghtJUNction/ztty/blob/99d94633391150c4ca431b568c27139ef63441dd/.github/workflows/release.yml)

**既有不完整草稿会安全停止。** 如果目标 Release 已存在，流程直接进入回读检查；缺失或冲突的附件导致检查失败。该流程没有自动补传或覆盖既有附件的分支，不能把它称为断点续传。已有公开 Release 也不会通过这个路径被重复公开。

**发布旧版本不会把更新的 latest 降回去。** 只有当前 latest 标签符合稳定版 `vX.Y.Z` 且版本不高于目标时，才传入 `--latest=true`；更新的版本或无法识别的标签使用 `--latest=false`。查询 latest 本身失败时，发布步骤停止。此保护来自源码复核；下面的只读任务没有实际调用发布或 latest 修改命令。

## 实际运行与源码边界

[验证运行 44404653933](https://github.com/LIghtJUNction/ztty/actions/runs/44404653933) 的 [job 135156541913](https://github.com/LIghtJUNction/ztty/actions/runs/44404653933/job/135156541913) 成功。实际 head 是 `a4bc61e9964d016d608b97b02f42efc8e47f3fe2`：它在永久逻辑基线 `691fbd5d1b447b8dd35a5865d958e6cef7b37d25` 上只增加一个发布专用验证工作流，权限为 `contents: read`。

| 实际检查 | 结果与范围 |
| --- | --- |
| Python AST | `tools/release_assets.py` 与 `tests/release_assets_test.py` 均通过解析 |
| 助手测试 | 7 项全部通过；unittest 报告耗时 0.013 s |
| YAML | `release.yml` 与该分支的 `validate-release-evidence.yml` 均通过 YAML 语法解析 |
| 发布 Shell | 对基线发布步骤执行 `bash -n` 通过；没有执行发布命令 |

七项测试覆盖确定性归档与重复回读、空文件和嵌套文件保留、非空附件变化时提前停止、冲突的空文件不被覆盖、归档内容变更时仍被拒绝、符号链接／递归输出拒绝，以及版本或源码不符拒绝。[测试源码](https://github.com/LIghtJUNction/ztty/blob/a4bc61e9964d016d608b97b02f42efc8e47f3fe2/tests/release_assets_test.py)

永久提交 `99d9463` 的 tree 为 `81b9d08b881481908fe6a84717813303a773bef2`，相对已验证基线 `691fbd5` 只在发布 Shell 增加下面一行。两个 Python 文件的 Git blob 与实际运行版本完全相同。

```bash
cmp "$evidence.sha256" "$readback/${evidence##*/}.sha256"
```

这行要求下载回来的 ZIP 校验记录原文与本地记录完全一致。它已经静态复核，**未在 `a4bc61e` 的运行中执行，也未被该次 `bash -n` 覆盖**。因此，本记录不会把永久版本的完整发布步骤标为已经端到端执行。

本次是发布流程定向验证，没有重跑普通 CI、原生 GUI 或跨平台构建。v0.3.1 专用恢复的结果见[独立应用发布记录](application-release-validation-2026-11-19.zh-CN.md)。本文不把该任务的上传、回读、草稿公开或 latest 操作记作本次只读检查的执行结果。精确提交、blob 标识、测试名与验证范围见[机器可读记录](release-assets-validation-2026-11-19.json)。
