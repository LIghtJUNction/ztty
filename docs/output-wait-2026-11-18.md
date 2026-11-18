# ztty 输出等待与 MCP 并发验收

日期：2026-11-18。范围：Linux 共享会话 broker、MCP stdio 与 JSON 响应模式的 Streamable HTTP。

本轮让 Agent 在一次 `session_read` 调用中等待新输出，同时继续处理其他请求。空闲会话最多等待 30 秒；有可交付输出、保留区发生截断或读取端结束时立即返回。等待期间，其他客户端仍可列出会话、发送输入和关闭会话。新增 11 组真实 PTY／传输检查全部通过，原有三套 MCP 检查及 9 项共享窗口检查通过。

| 记录 | 值 |
| --- | --- |
| 开发分支 | `iter/output-wait-20261118` |
| 功能提交 | [`d9c5fc768e9aa885f3c7b7f8ba532474ee6c0d5c`](https://github.com/LIghtJUNction/ztty/commit/d9c5fc768e9aa885f3c7b7f8ba532474ee6c0d5c) |
| 最终验证源码 | [`c2fdb7caf8d4a6f548a29c9decf33bc4ccddd80c`](https://github.com/LIghtJUNction/ztty/commit/c2fdb7caf8d4a6f548a29c9decf33bc4ccddd80c)；已合入并行完成的 UTF-8 分页修复 `4df37df` |
| 实测可执行文件 SHA-256 | `df97c8609c8c1ded13b41cdbe435347be558823803358decc4c27640cb788b05` |
| 完整原始记录 | [验收汇总](measurements/output-wait-2026-11-18/summary.json) · [二进制身份](measurements/output-wait-2026-11-18/binary.json) · [文件清单与校验值](measurements/output-wait-2026-11-18/manifest.json) |
| 接口说明 | [Sessions and MCP](sessions.md) |
| 新增验收程序 | [`tests/mcp_wait.py`](../tests/mcp_wait.py) |

## 用户能直接用到的变化

Agent 可以先发起输出等待，再继续发送 ping、查询会话或输入命令。stdio 的回复按实际完成顺序返回，客户端通过 JSON-RPC `id` 对应请求。HTTP 的不同连接共享同一监听器，空闲终端的等待连接不会占住整个监听器。

客户端可以在任务取消或页面关闭时释放等待资源。取消输出读取保留终端进程；重新连接后，客户端用保存的会话身份与字节游标继续读取。普通工具请求在完整发送后关闭输入方向，仍能完成并交付回复，保持管道脚本和 HTTP 半关闭的兼容行为。

## `session_read` 的等待语义

`wait_ms` 是可选的整数参数，范围为 `0..30000`。省略或设为零保持即时读取；正值允许空读取暂留在事件循环中。已有的 `cursor`、`max_bytes`、`expected_session` 和观察者权限继续生效。

| 条件 | 返回行为 |
| --- | --- |
| 已有可交付字节 | 立即返回输出页，正等待请求带 `timed_out: false` |
| 游标早于保留区 | 立即返回 `truncated: true` 和当前可读范围，正等待请求带 `timed_out: false` |
| `output_eof: true` | 返回保留输出或空尾部，正等待请求带 `timed_out: false` |
| 当前只有残缺 UTF-8 后缀 | 继续等待补全、读取端结束或截止时间 |
| 到达截止时间且仍无可交付输出 | 返回空输出、当前屏幕和 `timed_out: true`，游标保持不变 |
| 游标超过输出末端，或参数非法 | 立即返回错误 |
| 等待期间原会话被关闭 | 返回工具错误；同名新 PTY 不会接替原请求 |

例如，初始化 MCP 后可以发送以下请求。URI、游标及身份字段应从实际的创建或读取结果复制；示例数值仅说明结构。

```json
{
  "jsonrpc": "2.0",
  "id": 42,
  "method": "tools/call",
  "params": {
    "name": "session_read",
    "arguments": {
      "session": "ztty://mcp/stdio/checks",
      "cursor": 4096,
      "max_bytes": 16384,
      "wait_ms": 30000,
      "expected_session": {
        "uri": "ztty://mcp/stdio/checks",
        "id": 7,
        "created_at": 1794988800000,
        "broker_pid": 12345
      }
    }
  }
}
```

每个已登记的等待都会绑定当时的 PTY 实例，即使该次调用省略了 `expected_session`。客户端跨请求、重连或恢复时仍应携带完整身份，以覆盖两次请求之间发生的同名替换。多个观察者分别保存自己的 `next_cursor`，读取互不消耗对方的位置。

输出是否读完由 `output_eof && !has_more` 判断，并同时检查 `truncated` 与退出状态。直接子进程可能先退出，后代继续持有 PTY 并产生输出；本轮真实回归覆盖退出码 7 之后收到后代 `LATE` 输出的情况。

## 事件循环、取消与资源上限

broker、stdio 和 HTTP 都通过非阻塞连接与现有 poll 循环推进请求。连接、发送、等待输出和回复交付有各自的期限。等待对象保存会话 ID、游标、页大小和截止时间，完成时才构造屏幕结果。这样，等待空闲输出无需持续重建整屏 JSON，也无需为每个等待启动线程。

| 范围 | 上限及行为 |
| --- | --- |
| 单个正等待 | 最多 30,000 ms |
| 每个 stdio 进程 | 最多 32 个并发工具请求，其中正输出等待最多 24 个 |
| 每个 HTTP 监听器 | 最多 64 个并发连接，其中输出等待最多 32 个 |
| 共享 broker | 窗口等待与输出等待合计最多 128 个，保留管理连接容量 |
| stdio 排队数据 | 合计受 4 MiB 上限约束；stdout 连续五秒无法取得写入进展时结束 transport 并释放 pending 请求 |
| HTTP 在途数据 | 请求、回复和保留的 broker 缓冲合计受 4 MiB 上限约束；收包与回包分别使用十秒期限，输出等待单独计时 |

上述字节预算约束应用保留的在途内容。分配器容量、解析后的 JSON、终端状态和内核 socket 缓冲另有开销；本轮没有测量完整进程 RSS/PSS。

取消使用标准通知，`requestId` 为原调用的 JSON-RPC ID：

```json
{
  "jsonrpc": "2.0",
  "method": "notifications/cancelled",
  "params": {"requestId": 42}
}
```

有效取消会丢弃对应 broker 连接，取消的调用不再产生结果。未知、畸形及已经完成的取消被忽略。HTTP 取消仅匹配同一已认证 MCP transport session；另一个 session 使用相同 ID 无法取消该请求。取消通知本身返回 HTTP 202，原等待 HTTP 连接关闭。DELETE 返回 HTTP 200，释放该 transport session 的等待并保留终端进程。

输入方向 EOF 会取消正等待，包括 HTTP 的 `shutdown(SHUT_WR)`。已经完整接收的普通请求继续完成：本轮用一次 stdio 写入初始化、ready、list、即时 read 后关闭 stdin，实际收到三个对应回复；HTTP 普通 list 与即时 read 在写方向关闭后仍收到完整 200 回复。客户端还应处理取消和结果完成同时发生的竞争，见 [MCP cancellation](https://modelcontextprotocol.io/specification/2025-11-25/basic/utilities/cancellation)。

## 实际验收结果

新增程序使用独占抽象 Unix socket、临时运行目录、真实 PTY 和实际 stdio／HTTP 连接。子进程用文件握手确认就绪，回复按 ID 关联；broker 和后代用 pidfd 绑定生命周期后清理。该程序不操作已有桌面会话。

| 新增检查组 | 实测结果 |
| --- | --- |
| stdio 等待期间 ping、list、write 并行处理 | 通过 |
| 短超时、零等待、五种非法参数及 future cursor | 通过 |
| Emoji 前两个字节保持等待，补全后一次返回四字节字符 | 通过 |
| 300 KiB 输出触发保留区淘汰，过期游标立即返回有界页 | 通过 |
| 静默 EOF 与直接子进程退出后的后代输出 | 通过 |
| 关闭并重建相同 URI，原等待拒绝读取新 PTY | 通过 |
| stdio 取消、24 个等待配额、管理余量、配额复用与 stdin EOF | 通过 |
| HTTP 操作用户和只读观察者独立游标并发 | 通过 |
| HTTP 重复 ID、取消隔离、32 个等待配额、断连复用与 DELETE | 通过 |
| 11 秒 HTTP 空等待完整返回超时结果 | 通过 |
| 完整普通请求在 stdio EOF／HTTP 写方向半关闭后继续完成 | 通过 |

上述 11 组检查在最终合并源码上重新执行并全部通过，见 [真实等待记录](measurements/output-wait-2026-11-18/mcp-wait.json)。原有 `mcp_transport.py`、`mcp_sessions.py`、`mcp_security.py` 三套检查及共享窗口的 9 项检查也重新通过，见 [运行门禁](measurements/output-wait-2026-11-18/runtime-gates.json) 与 [共享窗口记录](measurements/output-wait-2026-11-18/shared-gui.json)。其中 `mcp_sessions.py` 包含并行分页修复新增的畸形 UTF-8 跨页回归。测试进程和私有显示环境已清理。

完整源码门禁也全部通过：

| 检查 | 结果与原始记录 |
| --- | --- |
| Rust 工作区标准单元／集成测试 | 230 项通过，[日志](measurements/output-wait-2026-11-18/rust-tests.log) |
| SessionCore 真实进程检查 | 8 项通过，与上述标准测试分开计数，见同一日志 |
| doctest | 6 项通过，见同一日志 |
| 格式与严格 Clippy | `cargo fmt --all -- --check`、`cargo clippy --locked --workspace --all-targets -- -D warnings` 通过，[门禁](measurements/output-wait-2026-11-18/build-gates.json) |
| release 与生产 no_std | 优化构建通过；对最终二进制重新审计，未发现 Rust std 符号，[审计](measurements/output-wait-2026-11-18/no-std.log) |
| 显式裸机目标 | `zterm`、`zinput`、`zgraphics` 的 `x86_64-unknown-none` 检查通过，[日志](measurements/output-wait-2026-11-18/bare-metal.log) |

裸机检查仍显示上游 `zune-png 0.5.2` 的 Rust 未来兼容警告；当前编译退出码为零。本轮未修改该依赖。

### 功能验收中的时间观测

以下数据来自本次功能验收的单次观测，用于检查等待时长及其他请求是否被阻塞。它们没有多轮采样、硬件对照或统计区间，不作为性能基准，也不用于推导吞吐、内存或相对其他终端的性能结论。

| 场景 | 设定或负载 | 本次观测 |
| --- | --- | --- |
| stdio 空读取超时 | `wait_ms=140` | 140.627 ms |
| HTTP 空读取超时 | `wait_ms=11000` | 11,001.947 ms |
| stdio 在输出等待期间完成 ping 与 list | 一个正等待仍未结束 | 0.312 ms |
| HTTP 在等待配额满时完成 ping | 32 个输出等待 | 0.329 ms |

11 秒 HTTP 检查覆盖长于旧普通请求截止时间的等待。满额时的 ping 和配额复用检查覆盖管理请求可达性及断连后的资源释放。

## 复现命令

在具备仓库 Linux 构建依赖、Python 3 与 Xvfb 的开发环境中，使用独立检出固定到最终验证源码。测试自行隔离 broker；共享窗口检查使用私有显示环境。

```sh
git clone --branch iter/output-wait-20261118 https://github.com/LIghtJUNction/ztty.git ztty-output-wait-check
cd ztty-output-wait-check
git switch --detach c2fdb7caf8d4a6f548a29c9decf33bc4ccddd80c

export CARGO_TARGET_DIR="$PWD/target/output-wait-check"
cargo build --locked --release
cargo fmt --all -- --check
cargo test --locked --workspace
cargo clippy --locked --workspace --all-targets -- -D warnings
python3 tools/check_no_std.py
cargo check --locked -p zterm -p zinput -p zgraphics --target x86_64-unknown-none

ZTTY_VERIFY_BIN="$CARGO_TARGET_DIR/release/ztty"
python3 tests/mcp_wait.py --binary "$ZTTY_VERIFY_BIN" --output artifacts/output-wait/report.json
python3 tests/mcp_transport.py --binary "$ZTTY_VERIFY_BIN"
python3 tests/mcp_sessions.py --binary "$ZTTY_VERIFY_BIN"
python3 tests/mcp_security.py --binary "$ZTTY_VERIFY_BIN"
python3 tests/shared_gui_smoke.py --binary "$ZTTY_VERIFY_BIN"
```

`mcp_wait.py` 的 JSON 包含实测二进制 SHA-256、逐组状态、耗时和清理状态。发生失败时仍写出已经完成的检查及失败内容，便于定位具体场景。

## 验收范围与后续工作

当前结论覆盖 Linux 共享 broker 及其两种 MCP transport。macOS、Windows 原生终端的共享 broker／MCP 接入与运行验收继续沿用各自任务清单，本轮没有新增平台通过结论。

HTTP 保持 JSON 请求／响应模式。本轮实现有上限的输出长轮询；SSE、主动输出订阅和多观察者推送仍待后续实现。等待接口没有增加图形协议、Wayland 后端或新的窗口界面。长期稳定性、完整 RSS/PSS、吞吐和输入到呈现延迟也不在本次功能观测范围内。

## 结论与证据对应

| 结论 | 对应证据 | 状态 |
| --- | --- | --- |
| 一个输出等待保留其他请求的处理能力 | stdio 并发组、HTTP 双观察者组及满额 ping 检查 | 已通过 |
| UTF-8、EOF 和同名会话替换保持明确边界 | 分片 UTF-8、后代输出、实例替换三组真实 PTY 检查 | 已通过 |
| 取消和断连回收等待容量并保留终端 | stdio 配额复用、HTTP 全配额复用和 DELETE 检查 | 已通过 |
| 普通工具保持 EOF／半关闭兼容 | 单次 stdio 写入四条消息及两个 HTTP 半关闭请求 | 已通过 |
| 当前改变未破坏已有 MCP 与共享窗口验收 | 三套原有 MCP 检查、共享 GUI 9 项 | 已通过 |
| 多平台原生共享 MCP、SSE、长期资源与性能表现 | 本轮未执行对应验收 | 继续保留任务边界 |
