# API 0.1.0 发布与六库 registry 消费验证

**ztty-plugin-api 0.1.0 已实际发布。** 2026-11-20 01:24 UTC 的读回中，
crates.io API、sparse index 和归档三个官方端点均返回 200；
下载归档与本次 dry-run 的 3,482 B 归档一致。
随后六个独立 registry consumer 及一个组合 consumer 全部通过，
验证的是已发布的 **zterm 0.5.0** 依赖图。

发布与消费验证固定在 ztty
[`36b367ffda4cea9b3b3dfbf38dc6375567912323`](https://github.com/LIghtJUNction/ztty/commit/36b367ffda4cea9b3b3dfbf38dc6375567912323)。
本记录不覆盖尚未发布的 zterm 0.5.1，也不代表 ztty 应用已发布、安装或完成全部原生平台验收。
早期“API 未发布”的报告保留其原有时点；本目录记录后续完成的步骤。

## API 发布证据

| 阶段 | 开始时间（UTC） | 结果 | 记录 |
| --- | --- | --- | --- |
| 同源码 dry-run | 01:22:08.188620 | exit 0；三个 registry 端点当时为 404 | [preflight](api-preflight.json)、[日志](api-preflight.log) |
| 实际上传 | 01:22:32.851800 | exit 0；`uploaded=true` | [publish](api-publish.json)、[日志](api-publish.log) |
| 官方 registry 读回 | 01:24:18.707812 | 三个端点 200；归档摘要相同 | [verify](api-verify.json) |

[索引记录](registry-index-version.json)的发布时间为 **2026-11-20T01:22:36Z**。
[API 响应](registry-api.json)还记录版本、3,482 B 大小和同一 checksum。
两个 JSON 是执行者保存的解析后响应；HTTP 原响应字节数和摘要另在 api-verify.json，
不能把重新排版的 JSON 文件摘要当作 HTTP 原始响应摘要。

归档共 7 个成员，VCS 为上述 36b367ff，`path_in_vcs=crates/ztty-plugin-api`。
包内规范化 manifest 精确依赖 `zplugin =0.3.1`，启用 `macros` 且关闭默认 features。
dry-run、上传前归档及 registry 下载的 SHA-256 相同：

```text
e00adc5df1eefa237ea1ccd8d837ceecabef1976141b78918901d95cb8e9d4a6
```

本次实际执行的发布命令保存在回执中：

```sh
cargo publish --locked --manifest-path crates/ztty-plugin-api/Cargo.toml --dry-run
cargo publish --locked --manifest-path crates/ztty-plugin-api/Cargo.toml
```

这是已完成上传的记录。后续维护按新版本候选发布，不重复上传本版本。
三阶段的输入文件摘要前后相同，包括根 Cargo.lock：
`c4290a5f00aa4437ad31007dfcc0f35e582b4c4f62511ade176ff553fc34efa3`。
归档成员、VCS、依赖字段和各阶段资源原值均留在 JSON；仓库不重复提交 .crate 二进制。

## 实际 registry 消费范围

[完整结果](consumers-result.json)记录六个独立 consumer 和一个组合 consumer。
每组依次运行 Cargo metadata、`check --lib --locked`、`run --locked`，
共 **21 条 Cargo 命令**均 exit 0。所有 consumer manifest 使用精确 crates.io 版本，
没有 path/git 依赖。选中包的 registry source、VCS 和 Cargo.lock checksum 均核对一致。

| 包 | 版本 | 包内 VCS | Cargo.lock / 归档 checksum |
| --- | --- | --- | --- |
| zinput | `0.1.3` | [`37617313`](https://github.com/LIghtJUNction/zinput/commit/37617313dd7aaabcca9dd567344a76c4cbe03907) | `d854f1abbf3a1b2fe2ce8ac2e9ac9ccf05452e6561cf8e8fc70f9f252c0914bd` |
| zgraphics | `0.2.0` | [`fe6357f9`](https://github.com/LIghtJUNction/zgraphics/commit/fe6357f98cf80fb6cc74739e45e8866ee17706ad) | `cc775cfedeae7a0563bb11e51e25b32e9d4a72757fb2935221ab7bc6f2c88e54` |
| zpty | `0.2.2` | [`66919bc6`](https://github.com/LIghtJUNction/zpty/commit/66919bc6dcb7757e0437fe955bfa809fb1282b68) | `1d46215c9aaad115aa3637217dcd3d7752d4f511f05ca29729c83ae58c0dd955` |
| zgpui | `0.1.0` | [`1d796d1f`](https://github.com/LIghtJUNction/zgpui/commit/1d796d1fc63808dc099ed7b10c92e75835ae2814) | `9943673fb607eade54661ff65243c4441a537f75188e361160a97344570bff90` |
| ztty-plugin-api | `0.1.0` | [`36b367ff`](https://github.com/LIghtJUNction/ztty/commit/36b367ffda4cea9b3b3dfbf38dc6375567912323) | `e00adc5df1eefa237ea1ccd8d837ceecabef1976141b78918901d95cb8e9d4a6` |
| zterm | `0.5.0` | [`13d5e2c7`](https://github.com/LIghtJUNction/zterm/commit/13d5e2c74571d35cccd920b1204798211b70c79f) | `5cad31f851ec516590643444f339eb6e0a2b7b034b68d190172233e9392364b1` |

生成的 consumer 库声明 `#![no_std]`，通过本机 Linux 工具链编译；可执行测试夹具使用 std。
这项检查不等于裸机运行、完整 API 覆盖或三平台原生窗口验收。
具体烟雾检查来自固定版本的
[verify_registry.py](https://github.com/LIghtJUNction/ztty/blob/36b367ffda4cea9b3b3dfbf38dc6375567912323/tools/verify_registry.py)：

| Consumer | 实际执行或编译检查 |
| --- | --- |
| zinput | Kitty Ctrl+C 编码与 Num Lock 数字键编码 |
| zgraphics | 接收 1×1 RGBA 图像、核对像素与 draw，再删除图像 |
| zgpui | capabilities 请求与返回类型 |
| zterm | Unicode 宽字符、DSR 响应、ZUI 场景及 reset |
| zpty | 尺寸类型；通过真实 PTY 启动 /bin/sh，读取固定输出并等待子进程成功退出 |
| ztty-plugin-api | 常量值与 TextAction::transform 泛型调用的类型检查 |
| 组合 | 同时执行上述消费路径，并核对 zterm 输入模式与独立 zinput 的类型兼容 |

API consumer 没有加载动态插件或创建插件沙箱；这些仍属于应用自己的插件验收。
zterm 的独立消费还解析到已发布的 zgpui、zgraphics、zinput，
其间接依赖来源也保留在完整结果中。

## 执行环境与资源

[启动记录](consumers-launch.json)和[完成回执](consumers-closure.json)绑定固定源码、验证器摘要和结果摘要。
本次从 01:33:07.807096 UTC 开始，完成耗时 **20.0293875530042 s**；
这是下载/编译/运行消费夹具的整段耗时，不是核心吞吐基准。

| 项目 | 记录值 |
| --- | --- |
| Rust 工具链 | 1.100.0 |
| 编译并发 | CARGO_BUILD_JOBS=1 |
| systemd 限额 | MemoryMax=768M、MemorySwapMax=0、CPUQuota=100%、Nice=10 |
| cgroup memory.peak | 322,183,168 B |
| memory.swap.peak | 0 B |
| memory.events | oom=0、oom_kill=0、oom_group_kill=0 |
| 单条 Cargo 超时 | 90 s |
| 源码、工作树、根锁文件 | 完成前后身份检查通过 |

cgroup 原始 cpu.stat 也完整保留，包括实际 throttling；这里没有把限额执行时间解读为性能改善。
消费者输出和失败字段保留在 [原始运行日志](consumers-run.log)及完整 JSON 中。

## 复核与复现

只核对本目录文件、归档身份、发布状态与七组结果，不访问网络或执行 Cargo：

```sh
python3 docs/reports/plugin-api-registry-20261120/verify-record.py
```

重新运行消费验证需使用固定源码及其子模块；以下会下载、编译并执行新的 consumer，
输出到新文件，不替代这里的历史记录：

```sh
git clone --recurse-submodules https://github.com/LIghtJUNction/ztty.git ztty-registry-recheck
cd ztty-registry-recheck
git checkout 36b367ffda4cea9b3b3dfbf38dc6375567912323
git submodule update --init --recursive
CARGO_BUILD_JOBS=1 RUSTUP_TOOLCHAIN=1.100.0 python3 tools/verify_registry.py \
  --cargo-timeout 90 --output artifacts/registry-consumer-recheck.json
```

验证器 SHA-256 为 `b1bae659a7f125309f8060e9cb3d8894a14665651770cbe3f5af18223925f51c`。
它读取该固定源码的六个版本号，再生成精确 registry 依赖；不会发布任何 crate。
上面的简短命令没有重建本次 systemd 资源容器，原限额参数另见启动记录。

## 原件与公开副本

[redaction.json](redaction.json)保存逐文件原 SHA、公开 SHA、字节数和修改字段。
仅将私有绝对工作目录替换为 `WORKTREE`，用户目录替换为 `USER_HOME`；
数值、字段顺序、退出状态、失败信息和资源值均保留。未触及路径的 API JSON 和索引响应保持原字节。
脱敏 launch 中的占位路径用于说明原运行环境，不是可直接执行的绝对路径。

原 consumers result SHA-256：

```text
d4803771883b01f6c2cbcffede8db1937dabd9e3c8c3ed90b88df7161de6b4e7
```

公开 consumers result SHA-256：`e14a551befde5f4c27e01c4636e2510ae8973ffc96347a4a69a11745911c1ded`。
原完成回执 SHA-256：
`560e324a50988e2db0274c57b1ee2fc14037aa3f431b82f06e26581d71a0f882`。
此次文档整理只做复制、必要路径替换和离线一致性核对，没有重复上传、编译或执行消费测试。
