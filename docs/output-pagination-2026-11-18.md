# ztty：修复 MCP 输出分页停滞

日期：2026-11-18（UTC+8）。

本轮修复了一个会让 Agent 一直读到空内容的问题：PTY 已经输出完整数据，但客户端使用较小的 `max_bytes` 分页时，非法 UTF-8 前缀可能被误判为仍在等待后续字节，导致游标停止前进。修复已接入现有开发分支和 [Draft PR #2](https://github.com/LIghtJUNction/ztty/pull/2)。开发机上的真实 stdio／HTTP 会话回归通过，同一用例在上一轮二进制上明确失败。

## 1. 问题与实际影响

以两个原始字节 `E2 58` 为例：`E2` 看起来像一个三字节 UTF-8 字符的开头，但随后已经到达的 `58` 是 ASCII 字符 `X`，足以确定前面的编码非法。按照现有文本输出规则，应返回替换字符 `�`，随后继续读取 `X`。

旧实现先按一字节页长切出 `E2`，再检查这张页是否包含完整字符。此时检查已经看不到后面的 `X`，于是反复返回空输出。只要 PTY 保持开放，客户端沿用返回游标就会停在同一个位置。

| 条件 | 旧二进制的实际结果 | 修复后的实际结果 |
| --- | --- | --- |
| 子进程保持运行，第一页从 0 开始，`max_bytes=1` | `output=""`、`next_cursor=0`、`has_more=true` | 返回 `�`，`next_cursor=1` |
| 下一页继续读取 | 再次从 0 读取仍停滞 | 返回 `X`，继续前进 |
| 真正尚未完整的 emoji 前缀 | 保持等待 | 保持等待，字节到齐后返回完整字符 |
| 子进程结束后仍有不完整尾部 | 使用替换字符输出 | 保留相同行为及原始字节游标 |

旧二进制的 SHA-256 与上一轮记录逐字节匹配，源码为 `f2a49ec`，对应文档检查点为 `bd92d3b`。回归先使用大页确认 broker 已接收完整测试数据，再用小页检查进度；子进程由显式文件门控保持运行，避免退出后的尾部处理掩盖问题。

[旧二进制复现记录](https://github.com/LIghtJUNction/ztty/blob/fix/output-pagination-20261118/docs/measurements/output-pagination-2026-11-18/mcp-before.json) · [上一轮二进制来源](https://github.com/LIghtJUNction/ztty/blob/bd92d3b49a3431c442700901d0e01f7744fe2c61/docs/measurements/shell-resume-2026-11-18/binary.json)

## 2. 实现与边界

修复将 UTF-8 校验放到分页之前，复用原来已有的 `max_bytes + 3` 字节缓冲。先利用已经到达的后续字节判断末尾是否真的不完整，再选择本页边界。UTF-8 字符最多占四字节，因此这三字节前瞻足以判断所有起点位于本页内的字符。

这次调整没有新增依赖、常驻字段或读取缓冲。每页仍最多返回 `max_bytes + 3` 个原始字节；独立观察者、环形保留区、截断标志、会话身份校验和 EOF 处理沿用原有接口。

| 提交 | 内容 |
| --- | --- |
| [5c2513e](https://github.com/LIghtJUNction/ztty/commit/5c2513e09f02339f47b3e9a7ebd4210267f06330) | 调整校验顺序；补充小页、真实未完成尾部及大页边界的 Rust 回归 |
| [2499758](https://github.com/LIghtJUNction/ztty/commit/24997585b7aa8187535a356f2c59b5ab9e8228c6) | 增加真实 PTY／MCP 回归；验证源码检查点 |

## 3. 验收结果

| 检查 | 结果 |
| --- | --- |
| 输出缓冲 Rust 集成测试 | 19 项通过，包含新增 3 项回归 |
| ztty 库测试 | 4 项通过，包含缓冲存储与读取分配上限检查 |
| 格式、严格 Clippy、Linux release 构建 | 全部通过 |
| 生产 no_std 检查 | 通过，release 二进制未检测到 Rust std 符号 |
| 完整 MCP 会话验收脚本 | 通过；包含新分页用例和已有续读、权限、输入队列、生命周期检查 |
| 旧二进制运行新分页用例 | 在 PTY 仍开放时准确捕获游标 0 停滞，测试会话清理完成 |

新的真实用例混合 `E2 X`、`F0 9F X`、中文、emoji 及紧随非法前缀的合法字符。stdio 从游标 0 以一字节页长读取，HTTP 从游标 2 以两字节页长独立读取；进程退出后交换两种页长，再检查未完成尾部的替换字符。所有分页文本拼接均与完整数据的文本转换一致。

开发机验证的可执行文件为 **682,160 bytes**，版本标识仍为开发候选 `ztty 0.2.1`；源码检查点是 `24997585b7aa8187535a356f2c59b5ab9e8228c6`。其身份记录、测试原始输出及命令退出状态已保存，日志中的 checkout 根路径统一替换为 `<checkout>`。

[命令与退出状态](https://github.com/LIghtJUNction/ztty/blob/fix/output-pagination-20261118/docs/measurements/output-pagination-2026-11-18/gates.json) · [Rust 测试](https://github.com/LIghtJUNction/ztty/blob/fix/output-pagination-20261118/docs/measurements/output-pagination-2026-11-18/output-tail.log) · [完整 MCP 结果](https://github.com/LIghtJUNction/ztty/blob/fix/output-pagination-20261118/docs/measurements/output-pagination-2026-11-18/mcp-sessions.log) · [二进制身份](https://github.com/LIghtJUNction/ztty/blob/fix/output-pagination-20261118/docs/measurements/output-pagination-2026-11-18/binary.json) · [no_std 结果](https://github.com/LIghtJUNction/ztty/blob/fix/output-pagination-20261118/docs/measurements/output-pagination-2026-11-18/no-std.log)

## 4. 复现

在包含此次提交的 checkout 中执行：

```sh
cargo test --locked --test output_tail --lib
cargo fmt --all -- --check
cargo clippy --locked -p ztty --all-targets -- -D warnings
cargo build --locked --release
python3 tools/check_no_std.py
python3 tests/mcp_sessions.py --binary ./target/release/ztty
```

最后一条命令使用 Linux 真实 PTY、stdio 与回环 HTTP。开发机已验证二进制位于：

```text
/home/lightjunction/Documents/GITHUB/ztty/artifacts/output-pagination-worktree/target/release/ztty
```

## 5. 结论检查与后续范围

| 检查维度 | 依据与范围 |
| --- | --- |
| 实际作用 | 修复开放会话的输出续读停滞；旧二进制准确复现，新二进制通过同一断言。 |
| 说明清楚 | 原始字节、分页参数、游标变化及运行命令均明确列出。 |
| 证据强度 | Rust 边界回归与真实 PTY／MCP 两层验收相互补充。 |
| 覆盖范围 | 验证 Linux stdio／HTTP、小页及大页边界、合法／非法混合文本、独立游标和 EOF；本轮未重测吞吐、RSS 或原生 macOS／Windows。 |
| 设计代价 | 复用已有有界前瞻缓冲，改变检查顺序，保持原有存储与接口约束。 |

本次交付继续归入开发候选，未创建正式 Release。输出等待与订阅，以及其他平台的原生验收，继续由现有项目清单推进。
