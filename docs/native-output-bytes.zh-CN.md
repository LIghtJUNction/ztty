# MCP 输出：可读文本与原始字节

Linux `session_read` 现在同时返回 `output` 和 `output_base64`。前者保持原来的可读 UTF-8 文本行为；后者使用标准 Base64 和 `=` 填充，编码**同一页未经 UTF-8 替换的原始 PTY 字节**。已有字段、工具名称、输入 schema 和字节游标规则不变。stdio 与 HTTP 返回相同的结构化字段。

例如，PTY 输出单个 `0xff` 字节时，`output` 是替换字符 `�`，`output_base64` 是 `/w==`。把 `output` 重新编码成 UTF-8 会得到三个不同字节，不能作为原始输出使用。需要逐字节转发的调用方应解码 `output_base64`：

```python
raw = base64.b64decode(result["output_base64"], validate=True)
next_cursor = result["next_cursor"]
page_start = next_cursor - len(raw)
```

空页的 `output_base64` 是空字符串。`cursor`/`next_cursor` 继续表示原始字节偏移，不是字符数量或 Base64 字符数量。`first_cursor` 仍表示最早保留的字节位置，不一定等于本页起点；本页起点可按上式计算。

## 仍然遵循现有分页

- `max_bytes` 为 1–65536。为完成一个 UTF-8 字符，一页最多可以多返回 3 字节。
- 输出仍在继续时，尚未结束的 UTF-8 尾部会暂留，直到后续字节或 EOF 明确边界；空等待结果不会提前推进游标。
- 已知无效字节仍保留在 `output_base64` 中。`output` 继续使用替换字符供人阅读。
- 发生留存淘汰时，`truncated` 仍为 `true`；已有规则会在被截断的左边界跳过 UTF-8 延续字节。新增字段不恢复已经丢失或按原分页规则跳过的前缀。

编码只发生在显式读取结果时。[OutputTail](../src/output_tail.rs) 的留存、UTF-8 边界判断和游标计算未改变；[Session 返回值](../src/session.rs) 新增一个字段。没有增加 PTY feed 回调或第三方编码依赖。

## 验证

`cargo test --test output_tail` 包含标准 Base64 填充向量以及全部 256 种字节值。真实 MCP 回归使用独立 broker、合成 PTY 输出和 stdio/HTTP 客户端，核对无效前缀与分片 UTF-8、继续输出和 EOF 时的小页、全部字节值逐页重建、65536 字节上限跨四字节字符，以及空页等待超时：

```sh
python3 tests/mcp_raw_output.py --binary target/release/ztty --output artifacts/native-raw-output/acceptance.json
```

脚本复用并加强 [既有 MCP 分页回归](../tests/mcp_sessions.py) 对原始字节的断言，不接触用户现有 broker 或终端会话。

## 本轮实测记录

合并候选源码 `f05dd2710c795412651f58839e28a345231445f6`，ELF SHA-256 `717487afce1e6c1588018654cbda70d1148fe7f5c22706220360d72678c5c39a`。20 项 OutputTail Rust 检查、4 组真实 stdio/HTTP 原始字节回归、同一 ELF 的 22 项插件 CLI 与 10 项插件 GUI 检查均通过，清理报告为空。格式检查、release 构建与 Clippy `-D warnings` 均成功。

[验收索引](measurements/native-output-2026-11-19/validation.json)、[原始字节案例](measurements/native-output-2026-11-19/acceptance.json)、[源码哈希核对](measurements/native-output-2026-11-19/source-equivalence.json)。原始字节回归在提交前运行，原报告的源码提交字段原样保留；另逐项核对其记录的源码与测试文件哈希。

下表是新增插件与原始字节字段后的短回归检查，基线为已安装性能版本 `8f35b084…`，候选为 `717487af…`。既往吞吐提升仍归属于基线版本，本表不将它计入新功能收益。沿用原输出 collector：120×32、2000 行历史、8 MiB × 3 样本，CPU 23/22/21 分别固定 broker、写入方与 Xvfb，先 AB 再 BA。表格值为两个运行中位数的中位数；完整范围与交互 DSR p50/p95 保留在 JSON。

| 输出 | 模式 | 基线 MiB/s | 候选 MiB/s | 变化 |
|---|---|---:|---:|---:|
| 普通日志 | 无窗口 | 95.58 | 97.59 | +2.10% |
| 普通日志 | X11 窗口 | 79.27 | 80.99 | +2.17% |
| 彩色日志 | 无窗口 | 83.60 | 87.35 | +4.48% |
| 彩色日志 | X11 窗口 | 86.14 | 81.06 | -5.89% |
| Unicode 日志 | 无窗口 | 46.86 | 46.21 | -1.38% |
| Unicode 日志 | X11 窗口 | 40.17 | 41.29 | +2.79% |
| 原地进度 | 无窗口 | 167.72 | 182.96 | +9.08% |
| 原地进度 | X11 窗口 | 161.55 | 173.74 | +7.55% |
| 原地 ASCII | 无窗口 | 131.11 | 131.67 | +0.42% |
| 原地 ASCII | X11 窗口 | 123.89 | 126.94 | +2.46% |

彩色日志窗口是本次需要保留的负向信号：候选两次运行中位数为 79.81/82.31 MiB/s，基线为 86.66/85.61 MiB/s，表中的汇总下降 5.89%。两个范围没有重叠，不能直接归为噪声；本轮短检查尚未确定原因。其他场景的正值也不作为新增优化收益。

计时终点是 PTY 的逻辑光标回复，包含并行软件绘制的争用，但不等于物理显示完成。全部 40 个场景检查了完整可见文本与自有进程清理，其中 20 个窗口场景另确认实际映射。短样本仅用于发现意外回归，负值如实保留，不能据此证明所有机器或工作负载都没有性能变化。

[逐样本数据](measurements/native-output-2026-11-19/risk-raw.json)、[汇总与范围](measurements/native-output-2026-11-19/risk-summary.json)、[顺序编排脚本](measurements/native-output-2026-11-19/run_risk_ab.py)。本记录验证 Linux 原生路径；不声称 Windows/macOS 已运行这些 MCP 或插件验收。
