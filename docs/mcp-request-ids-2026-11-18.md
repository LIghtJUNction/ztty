# ztty MCP 请求编号精确匹配验收

日期：2026-11-18（UTC+8）。范围：Linux 共享会话的 MCP stdio 与 Streamable HTTP JSON 响应模式。

本轮修复取消和重复请求检查对数字编号的匹配。`1`、`1.0`、`1e0` 现在对应同一整数编号，客户端改变数字的 JSON 写法后仍能匹配正在等待的请求。响应保留请求原来的编号文本。最终候选通过 45 项针对性 Rust 测试、13 组真实 PTY 等待检查和三套既有 MCP 回归；旧二进制在新增的 stdio、HTTP 重复编号检查中分别失败。[核心修改](https://github.com/LIghtJUNction/ztty/commit/7b8aac8b8513ef94213e7f5e7890d9ca0021eb4f) · [验收门禁](measurements/mcp-request-ids-2026-11-18/gates.json)

| 记录 | 固定位置 |
| --- | --- |
| 起点 | [b0a158f](https://github.com/LIghtJUNction/ztty/commit/b0a158f59392f58e7a263ccec3192e911b4a7110)，包含输出等待与 UTF-8 分页修复 |
| 本轮生产代码／二进制构建源码 | [7b8aac8](https://github.com/LIghtJUNction/ztty/commit/7b8aac8b8513ef94213e7f5e7890d9ca0021eb4f) |
| 最终测试脚本与接口文档 | [5afe3c0](https://github.com/LIghtJUNction/ztty/commit/5afe3c0065c10370c2616c7a0401425a38c61e7e) |
| 开发分支 | `fix/mcp-request-ids-20261118` |
| 持续集成分支／审阅入口 | [feat/production-roadmap-20261118](https://github.com/LIghtJUNction/ztty/tree/feat/production-roadmap-20261118) · [Draft PR #2](https://github.com/LIghtJUNction/ztty/pull/2) |
| 原始证据 | [环境](measurements/mcp-request-ids-2026-11-18/environment.json) · [二进制身份](measurements/mcp-request-ids-2026-11-18/binary.json) · [文件校验清单](measurements/mcp-request-ids-2026-11-18/manifest.json) |

## 问题与修复

JSON 中的数字可以用不同文本表示同一个值。原验证器已经接受精确整数的十进制和指数写法，但等待队列直接比较保存下来的原始文本。因此，`id: 1e0` 的等待请求不会与 `id: 1` 的重复请求匹配；同样的比较方式也用于取消通知。[原实现](https://github.com/LIghtJUNction/ztty/blob/b0a158f59392f58e7a263ccec3192e911b4a7110/src/mcp.rs) · [接口约定](sessions.md#cancelling-a-pending-request)

修复增加一个 MCP 专用的比较函数，供两种传输的去重和取消共四处调用。它把数字看成“符号、有效数字和十进制位置”，比较实际值而保留原始响应编号。小数点和末尾零只改变十进制位置；超长指数逐位处理，不展开成庞大的整数，也不经过浮点数。比较借用已有文本，额外空间为常数，没有新增依赖或每请求持久字段。[实现](https://github.com/LIghtJUNction/ztty/blob/7b8aac8b8513ef94213e7f5e7890d9ca0021eb4f/src/mcp.rs)

| 输入关系 | 结果 |
| --- | --- |
| `7`、`7.0`、`7e0`、`70e-1`、`0.7e1` | 相同编号 |
| `0`、`-0`、带任意指数的零 | 相同编号 |
| 数字 `7` 与字符串 `"7"` | 不同编号 |
| `9007199254740992` 与 `9007199254740993` | 不同编号，不因浮点舍入而合并 |
| `1e9223372036854775808` 与 `10e9223372036854775807` | 相同编号，支持超出 `i64` 的指数 |
| 相邻的超大指数或正负非零数 | 不同编号 |

上述规则由新增单元测试和真实传输用例覆盖。另有半 MiB 指数文本的进位用例，检查极长编号的处理。共享 JSON `Value` 的通用相等规则保持原状，HTTP 仍先限定同一已认证 transport session。[Rust 记录](measurements/mcp-request-ids-2026-11-18/protocol.log) · [真实等待记录](measurements/mcp-request-ids-2026-11-18/mcp-wait-final.json)

MCP 要求客户端在同一会话中为每次请求使用新编号。本轮服务器的防御检查针对正在处理的重复编号；它不维护完整会话的历史编号集合。测试中的重复请求是故意发送的非法输入，验证服务器能及时拒绝；成功请求与取消后的补位请求使用全新编号。[MCP 基础协议](https://modelcontextprotocol.io/specification/2025-11-25/basic/index) · [取消规则](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)

## 真实验收与失败对照

验收使用开发机上的独立 worktree、临时运行目录和专属抽象 broker socket。每个终端运行真实 PTY 子进程，并通过文件握手确认就绪。新增测试检查原始 JSON 响应文本，避免测试程序自己把大数字转成浮点数。[测试源码](https://github.com/LIghtJUNction/ztty/blob/5afe3c0065c10370c2616c7a0401425a38c61e7e/tests/mcp_wait.py)

| 检查 | 实际结果与证据 |
| --- | --- |
| Rust 协议／JSON／输出测试 | **45 项通过**：22 项 MCP 协议、4 项 JSON、19 项 OutputTail；其中本轮新增 3 项协议测试。[日志](measurements/mcp-request-ids-2026-11-18/protocol.log) |
| 最终真实等待测试 | **13 组通过**，含本轮新增的 stdio／HTTP 数字编号两组。[JSON](measurements/mcp-request-ids-2026-11-18/mcp-wait-final.json) |
| 满额取消和资源回收 | stdio 的 24 个、HTTP 的 32 个等待名额占满后，按等值数字取消一个等待，全新编号成功补位并收到输出；负零取消、字符串隔离和相邻大整数也通过。见同一 JSON 中两项 `numeric_request_ids` |
| 原始 ID 回显 | 数字 `1e0`、`7.0`、超大指数及字符串编号均逐个核对原始响应 token。见同一 JSON 的 `echoed_ids` |
| 既有 MCP 行为 | transport、sessions、security 三套全部通过，覆盖认证、16 并发、IPv6、跨传输续读、UTF-8、只读角色和会话隔离。[传输](measurements/mcp-request-ids-2026-11-18/mcp-transport.log) · [会话](measurements/mcp-request-ids-2026-11-18/mcp-sessions.log) · [权限](measurements/mcp-request-ids-2026-11-18/mcp-security.log) |
| 格式／Clippy／release／no_std | 全部通过，最终二进制没有 Rust std 符号。[门禁](measurements/mcp-request-ids-2026-11-18/gates.json) · [no_std](measurements/mcp-request-ids-2026-11-18/no-std.log) |
| 旧版 stdio 对照 | 新用例发出 `1e0` 后，以 `1` 发送重复请求，5 秒内未收到预期拒绝，测试退出 1；清理通过。[记录](measurements/mcp-request-ids-2026-11-18/before-stdio.json) |
| 旧版 HTTP 对照 | 同一重复请求场景未及时拒绝，socket 读取超时，测试退出 1；清理通过。[记录](measurements/mcp-request-ids-2026-11-18/before-http.json) |

两个旧版对照均在重复编号阶段失败，没有执行后续取消断言。取消、配额释放和原始编号回显的通过结论来自新版完整用例。验收记录中的毫秒数用于确认超时与响应行为，不是性能基准。最终检查及两次失败对照均记录 `cleanup: passed`。

## 二进制与资源变化

| 项目 | 修复前 | 修复后 |
| --- | --- | --- |
| 构建源码 | `c2fdb7caf8d4a6f548a29c9decf33bc4ccddd80c` | `7b8aac8b8513ef94213e7f5e7890d9ca0021eb4f` |
| 可执行文件字节数 | 708,456 | 711,168 |
| SHA-256 | `df97c8609c8c1ded13b41cdbe435347be558823803358decc4c27640cb788b05` | `8f54bd24c88d6459197136092c07f11eb926b77ccf26e2d09fafb3f12195194a` |

旧二进制与[上一轮验收记录](output-wait-2026-11-18.md)的哈希一致。本次 release 文件增加 **2,712 字节**。比较函数没有新增堆分配，但本轮未重新测量 RSS/PSS、吞吐或输入呈现延迟，不能据此推导整个终端的内存或速度变化。[二进制记录](measurements/mcp-request-ids-2026-11-18/binary.json)

## 复现命令

在具备仓库 Linux 构建依赖与 Python 3 的检出中，固定到测试提交 `5afe3c0065c10370c2616c7a0401425a38c61e7e`。它与 `7b8aac8` 的生产 Rust 代码一致。

```sh
cargo fmt --all -- --check
cargo test -p ztty --test mcp_protocol --test json --test output_tail
cargo clippy -p ztty --all-targets -- -D warnings
cargo build --release
python3 tools/check_no_std.py
python3 tests/mcp_wait.py --binary ./target/release/ztty \
  --output artifacts/mcp-request-ids/report.json
python3 tests/mcp_transport.py --binary ./target/release/ztty
python3 tests/mcp_sessions.py --binary ./target/release/ztty
python3 tests/mcp_security.py --binary ./target/release/ztty
```

单独运行新增用例可重复指定 `--check`。旧版会在第一项失败后退出，因此两种传输的失败对照应分别执行，并把 `--binary` 指向上述旧源码构建的可执行文件：

```sh
python3 tests/mcp_wait.py --binary ./target/release/ztty \
  --check stdio_numeric_request_ids \
  --check http_numeric_request_ids \
  --output artifacts/mcp-request-ids/numeric-ids.json
```

## 当前边界

本轮验收覆盖 Linux MCP 两种传输的请求匹配，没有新增完整工作区、GUI、macOS 或 Windows 原生验收结论。共享会话的跨平台接入、SSE 和主动输出订阅继续保留在待办中；本轮候选已提交至 Draft PR #2，供审阅，尚未成为正式 Release。上一轮全工作区与原生平台记录继续按其固定源码解释。
