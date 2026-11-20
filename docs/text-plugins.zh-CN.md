# 文本扩展：用 zplugin 处理选区并复制

ztty 现在可以注册并调用独立编译的原生文本扩展。按 **Ctrl+Shift+P** 打开命令面板，找到 **Copy using 名称**，确认后将选区交给扩展；没有选区时使用当前可见区域，包括向上滚动后看到的内容。成功结果写入剪贴板，随后显示短暂提示。结果不会自动输入到 shell。空结果、错误和取消会保留原剪贴板。

首个实际示例是 **Dedent**：复制缩进代码块时，删除各非空行共有的前导空格，保留相对缩进、中文、组合字符和原换行。它是一个普通动态库，使用与后续扩展相同的注册及调用接口；宿主没有识别示例名称的特殊分支。

## 使用

在仓库根目录编译终端和独立示例：

```sh
cargo build --release -p ztty
cargo build --release --manifest-path examples/dedent-plugin/Cargo.toml
plugin_path=$(realpath examples/dedent-plugin/target/release/libztty_dedent_plugin.so)
./target/release/ztty plugin check "$plugin_path"
env -u ZTTY_SOCKET ./target/release/ztty --text-plugin "dedent=$plugin_path"
```

`plugin check` 实际加载库、创建接口实例、销毁实例并检查合作卸载。新窗口中选中文字，按 **Ctrl+Shift+P**，输入 `dedent`，选择 **Copy using dedent**。面板也支持搜索 `plugin`。运行期间按 **Esc** 取消；若当前打开了搜索或命令面板，Esc 先关闭最上面的面板。

`env -u ZTTY_SOCKET` 只让这次新窗口启动自行选择当前构建的 broker，适用于从旧版本窗口试用候选二进制，不改变原有会话。正常安装后的独立启动无需这一步。显式 `ZTTY_SOCKET` 仍表示使用指定 broker。

也可以在配置文件中保存注册：

```ini
text-plugin = dedent=/absolute/path/libztty_dedent_plugin.so
text-plugin = another=/absolute/path/libanother.so
```

名称以 ASCII 字母开头，随后允许字母、数字、`-`、`_`，总长 1–32 字符。名称统一转为小写；后出现的同名注册替换前一个。命令行可以重复指定 `--text-plugin NAME=PATH`，并覆盖配置中的同名项。最多注册 8 项；路径必须是绝对 UTF-8 路径，允许空格，最长 4096 字节。注册不会加载库；只有主动执行动作或 `plugin check/run` 时才加载。[注册规则](../src/text_plugins.rs)

配置热重载沿用终端现有机制，也可按 **Ctrl+Shift+R**。修改或移除正在使用的注册会取消该次调用。重新载入的是文件内容；仅通过命令行提供的注册需要同时写入配置才能在重载后保留。关闭所属视图，或切换到隐藏它的标签页，也会取消调用。

命令行可以单独转换标准输入，不启动窗口：

```sh
printf '    first\n      second\n' | ./target/release/ztty plugin run "$plugin_path"
```

成功输出不附加额外换行。等待标准输入结束属于普通管道读取；执行期限从工作进程启动时开始计算。

## 接口与工作方式

[共享接口 crate](../crates/ztty-plugin-api/src/lib.rs)、当前宿主和示例统一使用 `zplugin = "=0.3.1"`，接口保持 `#![no_std]`。`ztty-plugin-api 0.1.0` 尚待首次发布，示例通过本仓库的路径依赖构建。接口标识是 `dev.project-z.ztty.text`，版本为 `1`：

```rust
fn transform(
    &mut self,
    input: &str,
    output: &mut [u8],
) -> Result<u64, zplugin::Error>;
```

输入和输出借用仅在调用内有效。扩展把 UTF-8 结果写入宿主提供的缓冲区，返回有效字节数。宿主检查返回长度和 UTF-8，再决定是否写剪贴板。修改这个签名需要新的应用接口版本；不能在相同版本下改变 ABI。

每个库提供一个文本动作，可以注册不同名称和不同库。扩展通过 `zplugin::interface` 定义代理，使用 `zplugin::export!` 导出，并明确实现合作卸载的 `UnloadSafe` 契约。实例不能在调用结束后留下使用库内代码或借用内存的线程、任务、回调或注册。可直接从 [Dedent 源码](../examples/dedent-plugin/src/lib.rs) 开始实现。

示例拥有独立 Cargo workspace，让扩展使用 `std`，同时避免 Cargo 特征合并把 `std` 带进终端宿主。zplugin 核心和共享接口保持 no_std；宏工具的编译依赖不会成为终端运行时依赖。

一次界面调用按以下顺序执行：

1. 用户确认动作，终端快照选区或当前可见文本。
2. broker 启动当前终端映像的独立 worker。库在 worker 内加载，broker 不调用库代码。
3. worker 创建实例、转换文本、销毁实例，然后调用合作卸载。
4. broker 验证完整结果和正常退出，在保留退出 worker 身份时清理其进程组，再回收 worker、写剪贴板。

工作进程使用独立进程组；GUI 对通信描述符做非阻塞读写，每轮限制工作量。关闭视图后仍保留作业对象，直到对应子进程被回收，避免遗留结果写入后来打开的视图。无扩展作业时不启动 worker、不建立扩展描述符或定时器；PTY 读入、VT 解析和逐字符绘制路径不调用扩展。[宿主实现](../src/plugin_host.rs) · [界面接入](../src/session.rs)

## 限额与适用范围

| 项目 | 限额或行为 |
|---|---|
| 单次输入、输出 | 各 1 MiB，UTF-8 |
| 母进程执行期限 | 2 秒，超时取消作业 |
| worker 自身备用期限 | 3 秒的 alarm；母进程退出时终止直接 worker |
| CPU 时间 | 1 秒 |
| 虚拟地址空间 | 256 MiB；不是常驻内存统计 |
| 文件描述符 | 最多 64；执行前关闭非标准描述符 |
| core dump | 禁用 |
| 并发 | 每视图一次调用，每 broker 最多 8 个未回收作业 |
| 失败处理 | 丢弃整次结果，保留剪贴板，显示短暂错误 |

原生扩展拥有当前用户权限，必须信任其代码和依赖。进程分离、执行期限与资源限额约束出错的扩展，**不构成安全沙箱**；不能用它执行不可信的原生代码。插件应遵守合作卸载约定，不能把测试夹具中故意泄漏子进程的方式作为实现示例。

本次宿主与 GUI 入口仅接入 **Linux**，在 x86-64 Linux 的原生动态库和私有 Xvfb 上验证。执行前通过 `close_range` 关闭继承描述符，因此要求 Linux 5.9 或更新版本；旧内核会拒绝启动该 worker。没有把共享 no_std 接口的可移植性当成 Windows/macOS GUI 已支持，也没有测量物理显示器延迟。

## 复现检查

以下命令分别构建真正独立链接的库、验证 CLI 生命周期，再验证真实 XTEST/剪贴板/PTY：

```sh
python3 tests/build_text_plugins.py --output artifacts/zplugin
python3 tests/plugin_cli.py --binary target/release/ztty --example artifacts/zplugin/example.so --fixtures artifacts/zplugin --output artifacts/zplugin/cli.json
python3 tests/plugin_gui.py --binary target/release/ztty --example artifacts/zplugin/example.so --fixture artifacts/zplugin/fixture-valid.so --artifacts artifacts/zplugin/gui
```

GUI 检查使用新建的私有 Xvfb、独立 broker socket 和合成文本，不操作用户桌面、剪贴板或现有会话。故障库覆盖接口版本不匹配、拒绝合作卸载、错误长度、非法 UTF-8、panic、原生崩溃、超时、CPU 限额、异常 stdout、遗留子进程通道以及母进程退出。它只用于测试，不应安装成日常扩展。

## 历史验证记录（zplugin 0.3.0）

验证源码为 `68030066cb18e97fa5677a77881d54188fdc93f3`，Linux ELF SHA-256 为 `ec064a5220f3308f7c6722484f2e81a29314782241233f8668b450d07e517c51`。同一 ELF 上的 CLI 22 项、私有 Xvfb GUI 10 项、日常工作流 19 项与重绘 11 项全部通过。Clippy 以 `-D warnings` 通过；Windows GNU 前端通过 `cargo check`，这不代表 Windows 已接入插件宿主。注册规则 2 项、SessionCore 真实 PTY 9 项与 Dedent 2 项测试通过，对应源码未被后续 worker 收尾修复修改。macOS 未测试。

独立审查发现并直接复现了一个退出边界：插件子进程关闭标准描述符后，原 worker 可以正常返回，而子进程仍留在进程组。修复使用 `waitid(WNOWAIT)` 保留已退出 leader 的身份，先终止属于本次调用的进程组，再 `waitpid` 回收；新增故障用例验证该子进程也被终止并由测试进程回收。测试没有按进程名称清理，也没有触碰用户会话。

[验证汇总](measurements/zplugin-2026-11-19/validation.json) · [CLI 与生命周期](measurements/zplugin-2026-11-19/cli.json) · [GUI 检查](measurements/zplugin-2026-11-19/gui.json) · [基线复现](measurements/zplugin-2026-11-19/group-cleanup-baseline.json) · [依赖特征图](measurements/zplugin-2026-11-19/features.log)

[命令面板中的注册动作（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)

[选区处理后的轻量复制反馈（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)
