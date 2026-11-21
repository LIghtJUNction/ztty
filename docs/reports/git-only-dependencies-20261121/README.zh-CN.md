# ztty 内部 Git 依赖维护与插件验收

2026-11-21（Asia/Taipei） · [源码 `bc37e35`](https://github.com/LIghtJUNction/ztty/commit/bc37e353ec5b7d4565c2e92d54f60eb84f4352c1)

本轮移除了 4 个开发 manifest 中的 9 处内部 Git 版本限制，并同步主程序、插件示例和故障测试插件的三份锁文件。所有内部 Git 来源均刷新至本轮可获取的默认分支；实际变化为 `zplugin`、`zplugin-macros` 从 `cf02c1d9` 更新至 `592a1be623b81a775026c470e6f0efe91fcf78f3`。这两个包仍为 0.3.1，上游只移除了 zsys 的版本限制，运行时代码和插件 ABI 保持一致。

## 依赖规则与实际结果

开发声明使用 canonical Git URL，不附加版本、标签或提交选择器。`Cargo.lock` 继续记录精确版本与提交。根 workspace、`examples/dedent-plugin` 和 `tests/fixtures/text-plugin` 的插件来源一致；五个子模块与根锁文件匹配，本轮无需改变 gitlink。同仓路径依赖、包本身版本和外部 registry 依赖保持原有关系。

源码基于 `99ef7a0f02dfb643a426b4b5f8daf0bc009d9752`。验证完成后，原样提交所测的依赖文件；原测试工具记录的运行前 Git HEAD 保留为该基线。本报告和 [validation.json](validation.json) 将实际验证结果关联到本轮源码提交及锁定图。

## 本地验证

所有 Cargo 构建复用共享构建目录，并在同一外层锁内顺序完成。插件 GUI 使用私有 Xvfb、独立 broker 和合成文本。

| 检查 | 结果 |
| --- | --- |
| Rust workspace，启用 Wayland | 214 通过，0 失败，8 忽略 |
| SessionCore 独立 harness | 10 项通过，测试子进程全部回收 |
| 格式、Clippy、release 构建 | 全部通过，Clippy 拒绝 warnings |
| no_std | 源图检查与 release ELF 检查通过，ELF 无 Rust std 符号 |
| Registry 版本选择 / 源码归档回归 | 6 / 3 项通过 |
| 独立插件动态库构建 | 4 个通过 |
| 插件 CLI | 22 项通过 |
| 插件 GUI | 10 项通过，清理错误 0 |

插件验收包含宽字符与组合字符复制、1 MiB 输入、非法 UTF-8、接口不匹配、panic、崩溃、超时、资源限制及子进程回收。GUI 检查还覆盖取消、热配置移除插件、关闭所属窗口，以及其他窗口继续响应。已查看动作菜单与选区复制截图；截图和编译产物保存在验收目录，Git 仅保存文本记录。

## 发布范围

本轮交付为 0.5.2 开发源码，未发布新 registry 包或替换桌面安装。8 项 ignored 测试保持未执行状态；Wayland 在本轮完成编译与 Rust 检查，实际插件 GUI 验收范围为 Linux X11。

下一次 registry 发布应从已提交锁文件生成独立 staging manifest，并核验锁定 Git package 的版本及最终包内容。Project Z 当前发布投影会拒绝带子模块的仓库，需先补齐递归 staging 后再用于 ztty。当前维护规则见 [发布说明](../../releasing.md)。
