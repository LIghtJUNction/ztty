# 大段输入结束后释放队列缓冲

日期：2026-11-20（Asia/Taipei）

## 结果

ztty 在大段输入完成或被明确取消后，释放超过 64 KiB 的输入缓冲。普通键盘输入继续复用已有的小缓冲。一次 8 MiB 输入的回归测试中，清空后的队列容量从 **8,388,608 字节降为 0**。

| 场景 | 修改前 | 修改后 |
| --- | ---: | ---: |
| 8 MiB 输入入队后的容量 | 8,388,608 B | 8,388,608 B |
| 同一输入清空后保留的容量 | 8,388,608 B | 0 B |
| 普通 4 KiB 输入清空后再次输入 | 复用缓冲 | 继续复用缓冲 |
| 大输入存在未发送后缀 | 保留待发送数据 | 保留待发送数据 |

本次测量对象是 Rust `Vec` 持有的分配容量。释放后由内存分配器管理这些空间；整进程的 RSS/PSS 和吞吐变化需要单独的应用测量。本报告没有进行这类测量。

## 修改原因与行为

旧的 `InputQueue::clear()` 调用 `Vec::clear()`，把逻辑长度归零，同时保留分配容量。因此，一次大粘贴结束后，多 MiB 的缓冲仍会由会话长期持有。

新实现只在清空阶段判断容量：不超过 64 KiB 时保留以供复用，超过时释放整个缓冲。发送完毕、移交输入控制权或明确取消输入都会经过这条已有路径。部分写入、`WouldBlock` 和写入错误仍保留未发送的后缀。已经被内核接受的输入保持原有语义。

修改没有增加依赖、公共接口或后台任务。频繁的大粘贴会重新申请大缓冲，这是减少空闲会话持有内存的明确取舍；普通小输入保留原有复用方式。

## 固定源码

- 生产基线：[4e72b17bf938c2e18972a7300160d150221e486b](https://github.com/LIghtJUNction/ztty/commit/4e72b17bf938c2e18972a7300160d150221e486b)。
- 最终代码：[d1db25ccfdac288ccc1fbf2ff0ffa76f15c899d4](https://github.com/LIghtJUNction/ztty/commit/d1db25ccfdac288ccc1fbf2ff0ffa76f15c899d4)。
- 实现：[src/input_queue.rs](https://github.com/LIghtJUNction/ztty/blob/d1db25ccfdac288ccc1fbf2ff0ffa76f15c899d4/src/input_queue.rs)。
- 原生背压回归：[tests/session_core.rs](https://github.com/LIghtJUNction/ztty/blob/d1db25ccfdac288ccc1fbf2ff0ffa76f15c899d4/tests/session_core.rs)。

旧版复现使用上述生产基线和新增的容量回归测试。最终代码与已验证的本地代码逐文件一致；提交方式变化没有改变源码字节。

| 文件 | SHA-256 |
| --- | --- |
| `src/input_queue.rs` | `60c79aa74eff789c202594e1b8d4d1e36f8a1138aa9cfa304e2a9ddcf1979b9a` |
| `tests/session_core.rs` | `12eff80ef2394d3bcde2d1720c24ec8ce9c16330cce869e93380b79a75d6c535` |

## 验证

容量回归在旧代码上真实失败：清空后仍保留 8 MiB。修改后两项检查通过，分别覆盖大输入释放和小输入复用。

SessionCore 的 10 项检查全部通过。新增检查在测试子进程回收后，把其持有的 PTY 描述符替换为一个真实的非阻塞 Linux pipe，以稳定触发部分写入和 `WouldBlock`。它请求 4096 字节容量，按内核实际返回值验收；本次实际容量为 4096 字节。测试覆盖：

- 三轮超过 2 MiB 的输入，逐字节核对完整内容。
- 部分发送后追加输入，等待可写后继续发送。
- `WouldBlock` 后待发送长度保持不变。
- 同一控制者继续持有输入；切换或释放控制权时取消尚未发送的后缀。
- 已进入内核的前缀保持不变。
- 所有测试子进程完成回收。

测试对 Linux pipe 容量接受内核按页大小取整，避免把宿主页大小差异误判成产品失败。生产修改后运行的容量测试可复用；这一测试调整后的 SessionCore 已另行重跑。

执行命令与退出码保存在 [evidence.json](evidence.json)。主要命令为：

```sh
cargo test --locked --offline --lib input_queue::tests:: -- --nocapture
cargo test --locked --offline --test session_core
rustfmt --edition 2024 --check src/input_queue.rs tests/session_core.rs
git diff --check
```

| 检查 | 结果 | 原始输出 |
| --- | --- | --- |
| 旧生产代码的容量回归 | 1 项按预期失败、1 项通过 | [baseline.stdout.log](baseline.stdout.log) |
| 修复后的容量回归 | 2/2 通过 | [queue-unit.stdout.log](queue-unit.stdout.log) |
| 最终 SessionCore | 10/10 通过 | [session-core.stdout.log](session-core.stdout.log) |
| 格式、补丁空白检查 | 通过 | [evidence.json](evidence.json) |

上述验证在 Linux 开发机上执行。公共队列实现被所有平台复用；正式发行的各平台构建和原生验收由应用 Release 工作流记录。
