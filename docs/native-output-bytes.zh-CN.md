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
