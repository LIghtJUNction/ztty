# 常用快捷键：开发机上的实际候选验证

此记录对应源码 **`bc89a9d4f0d6ca720f649d2620b9282467d1302f`**，版本输出为 `ztty 0.3.1`。使用 [GitHub Actions 运行 44360944908 的 Linux x64 成功任务](https://github.com/LIghtJUNction/ztty/actions/runs/44360944908/job/135001946175) 中 artifact `14455180463` 的二进制，在开发机私有 Xvfb 和测试 PTY 中直接运行。没有重新编译或安装。

**59 个输入向量和 14 个字号场景全部通过，退出码为 0。** 测试从 2026-11-19 01:54:40 到 01:54:48（Asia/Taipei），平台为 Linux 7.3.4-arch1-1 x86_64 / glibc 2.44。

| 操作或场景 | 验收结果 |
| --- | --- |
| `Ctrl+=`、`Ctrl+Shift+=` / `Ctrl++` | 增大字号；真实 `Ctrl+Shift+Plus` 把单元格从 9×24 改为 10×25，把 PTY 网格从 100×28 改为 90×26 |
| `Ctrl+-`、`Ctrl+Shift+-` | 缩小字号；Shift 场景下单元格从 9×24 改为 8×23，网格从 100×28 改为 112×29 |
| `Ctrl+0`，Linux 上的 `Ctrl+Shift+0` 别名 | 恢复默认字号，实际恢复为 9×24 单元格和 100×28 网格 |
| `Ctrl+KP_Add`、`Ctrl+KP_Subtract`、`Ctrl+KP_0` | 数字键盘加减在 Num Lock 开／关下通过；恢复操作在 Num Lock 开启时通过 |
| 非美式键盘布局 | 测试通过 XTEST 在映射后的物理键上输入加号，实际触发字号增加 |
| 自定义禁用与重映射 | `none` 消费按下及释放；数字键盘自定义映射优先于默认绑定 |
| `Ctrl+Tab`、`Ctrl+Shift+Tab` | 按住快捷键切换标签再释放的 3 次手势通过；两个 PTY 的 6 项字节检查均为空 |
| 恢复与清理 | 字体和键盘映射恢复；stderr 为空；测试拥有的剩余进程为 `[]` |

窗口像素尺寸保持不变。字号增加后，一个字符占用更多像素，窗口能容纳的行列数随之减少；测试同时检查了可见单元格尺寸和内核 PTY 网格，证明缩放已经影响布局。使用同一测试脚本时，旧版在新增的第一项缩放场景仍保持 9×24 单元格、100×28 网格；候选在同一场景产生上述变化。

Windows 上 `Ctrl+Shift+0` 可能被系统保留，因此它是受操作系统配置影响的别名；跨平台恢复字号的主快捷键是 `Ctrl+0`。这份记录只证明指定 Linux 候选在开发机的行为，后续发布源码需要由对应的正式验收记录确认。

来源核对依次覆盖 GitHub artifact 摘要、构建清单、归档内文件摘要与 CI 已测试 ELF 摘要。运行前后源码、脚本和 ELF 均未变化，源码工作树干净。

| 证据 | SHA-256 |
| --- | --- |
| 实际运行 ELF | `945eccdd73816cf47b35b973fcc2a3d53bb50e86fcc5d4f914f7f591d569ea25` |
| `tests/input_gui.py` | `27d316c42c55b8aa785d178ebba35418869194a55b8d5d28e661f9e1d0ba235f` |
| GitHub artifact ZIP | `6b9abcd1422a2981f2955922a6e287f88319f8ed286eb5b3da987ede312ea093` |
| `dev-input-gui.json` | `3690d71408b8b2d3f357533ed5cf9d7b93cd57ceaff1273f9609e3e147556441` |
| `dev-gui-attestation.json` | `c78fa223a49caaf16a79821091baf5dd42aa88b655228ba7e07fdf47af8c2649` |
| `evidence-manifest.json` | `9f2264e03f3cdaf2e05943b07b7dadb1328c36a4758853bc828b7506c0e229cf` |

完整证据在开发机 `artifacts/shortcut-candidate-ci-evidence/`，包括源码检出、下载归档、构建清单、二进制、测试结果及截图。
