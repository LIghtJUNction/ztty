# 固定源码规模比较

- Before: f2db31d7ea35bf1a0554e76d69d2e44faafd0cab
- After: 9855da563f3038adee2fa88e1e24e20eafb77010
- 审计脚本 SHA-256: 960b8dcd52363816dea50943261e2f7f2852e0c7de8e932b928b0e4cc9ab0612

统计 Git 对象中的 .rs/.c/.h；物理行包含空白、注释和内联测试。根仓库直接维护代码与递归 gitlink 库代码分列。此数值不与旧 37848 基数直接相减。

| 范围 | 前文件数 | 后文件数 | 前行数 | 后行数 | 行数变化 |
|---|---:|---:|---:|---:|---:|
| 根仓库直接维护 | 79 | 89 | 40436 | 30893 | -9543 |
| 递归 gitlink 库 | 3 | 39 | 1345 | 14306 | +12961 |
| 合计 | 82 | 128 | 41781 | 45199 | +3418 |

## 组件

| 组件 | 前归属 | 后归属 | 前文件/行 | 后文件/行 | 前/后排除路径 |
|---|---|---|---:|---:|---:|
| crates/zgpui | gitlink_library | gitlink_library | 3/1345 | 3/1345 | 16/16 |
| crates/zgraphics | root_repository | gitlink_library | 8/3183 | 10/3203 | 18/27 |
| crates/zinput | root_repository | gitlink_library | 1/1908 | 4/2539 | 3/16 |
| crates/zpty | root_repository | gitlink_library | 5/2381 | 5/2381 | 6/15 |
| crates/zterm | root_repository | gitlink_library | 10/4731 | 17/4838 | 18/28 |
| crates/ztty-plugin-api | root_repository | root_repository | 1/18 | 1/18 | 3/3 |
| ztty-host | root_repository | root_repository | 54/28215 | 88/30875 | 1064/1374 |

## Before 最大文件

| 文件 | 归属 | 物理行数 | 内联测试文本标记 |
|---|---|---:|---|
| src/platform/windows.rs | root_repository | 3739 | 未检出标记 |
| src/session.rs | root_repository | 3663 | 未检出标记 |
| src/x11.rs | root_repository | 3363 | 未检出标记 |
| crates/zterm/src/terminal.rs | root_repository | 2725 | 有，全部计入 |
| src/platform/macos.rs | root_repository | 2066 | 未检出标记 |
| crates/zinput/src/lib.rs | root_repository | 1908 | 有，全部计入 |
| src/mcp_http.rs | root_repository | 1593 | 有，全部计入 |
| src/mcp.rs | root_repository | 1570 | 有，全部计入 |
| crates/zgraphics/src/lib.rs | root_repository | 1206 | 有，全部计入 |
| crates/zpty/src/windows.rs | root_repository | 973 | 有，全部计入 |

## Before 排除项汇总

| 原因 | 路径数 |
|---|---:|
| dedicated_test_filename | 18 |
| excluded_directory:examples | 5 |
| excluded_directory:tests | 14 |
| outside_production_roots | 1076 |
| unsupported_extension:(none) | 5 |
| unsupported_extension:.md | 5 |
| unsupported_extension:.toml | 5 |

## After 最大文件

| 文件 | 归属 | 物理行数 | 内联测试文本标记 |
|---|---|---:|---|
| crates/zinput/src/lib.rs | gitlink_library | 1920 | 有，全部计入 |
| src/mcp_http.rs | root_repository | 1593 | 有，全部计入 |
| src/mcp.rs | root_repository | 1570 | 有，全部计入 |
| src/session.rs | root_repository | 1369 | 未检出标记 |
| crates/zpty/src/windows.rs | gitlink_library | 973 | 有，全部计入 |
| src/gpu/mod.rs | root_repository | 937 | 有，全部计入 |
| src/broker.rs | root_repository | 881 | 未检出标记 |
| native/wayland_services.c | root_repository | 763 | 未检出标记 |
| src/session/requests.rs | root_repository | 714 | 未检出标记 |
| src/platform/windows/diagnostics.rs | root_repository | 687 | 未检出标记 |

## After 排除项汇总

| 原因 | 路径数 |
|---|---:|
| dedicated_test_filename | 19 |
| outside_production_roots | 1457 |
| unsupported_extension:(none) | 1 |
| unsupported_extension:.md | 1 |
| unsupported_extension:.toml | 1 |

## 核验边界

audit.json 包含每个根仓库/子模块的完整提交与 tree SHA、每个计入文件的 Git blob、SHA-256、物理行数，以及逐路径排除原因。源码来自固定 Git 对象，不读取工作树文件来代替提交。
内联测试标记只作披露，不解析 Rust、不删除测试代码；未检出标记不保证文件没有测试。
仅遍历生产路径中的 gitlink；不展开 crates.io 或系统库依赖。同一库多处挂载时分别计数。
依赖拆出根仓库属于维护归属变化，不能据此声称整个递归源码规模等量减少。
