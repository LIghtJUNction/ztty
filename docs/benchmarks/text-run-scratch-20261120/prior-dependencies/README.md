# 依赖更新前的验证记录

日期：2026-11-20（Asia/Taipei）。以下记录在上游 Project Z 依赖更新之前产生。

基线：`2410064eef530400f750225b0561262445a3fa66`。候选：`2e195b543b96feee02648fd3d692e6d87109e78c`。

三场景各完成三组成对采样，每次统计 100 帧，9/9 组 RGBA 输出逐字节一致。候选预热后的 Rust 分配和重分配次数均为零。原始值与固定子模块提交见 [results.json](results.json)。

另完成五组成对计时，每场景每次 1,000 帧、固定 CPU、一个 llvmpipe 工作线程；结果完整保存在 [timing-control.json](timing-control.json)。这组时间值仅表示当时的软件 EGL 环境，不与更新后依赖的样本合并。

当前源码的验证、结论和复现步骤见 [主报告](../report.zh-CN.md)。
