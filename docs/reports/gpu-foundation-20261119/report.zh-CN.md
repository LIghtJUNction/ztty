# GPU 渲染与文字整形：基础层验证

## 完成范围

新增 Linux EGL / OpenGL ES 渲染模块，以及 Fontconfig、FreeType、HarfBuzz 字体桥接。X11 和 Wayland 共用绘制 API，终端内容仍由 zterm 持有。这一提交完成基础层；正常窗口的渲染选择、菜单和输入接线属于后续提交。

文字按完整字符簇选择字体，再按字体和文字脚本整形。终端文本保留固定单元格位置，界面文本使用字体字宽。原生探针检查了连字、阿拉伯文、天城文、肤色及家庭 ZWJ、国旗彩色 emoji。这里不声称实现了完整 Unicode 双向排版算法。

GPU 合成字形、矩形和图片。每个原生显示连接共用一个 EGL Display，各窗口拥有自己的 context 和 surface。切换窗口、销毁其中一个窗口后继续绘制、缩放、裁切、图片替换及透明度都做了实际像素回读验证。

## 资源边界

- 字形图集固定 1024 × 1024 RGBA，约 4 MiB；最多 4096 个缓存项，清空前提交引用旧坐标的批次。
- 图片纹理缓存最多 32 MiB、64 项，按版本和使用顺序回收。这不是进程总内存上限。
- 单次文字整形最多 16 KiB、8192 个输出字形；字体最多 64 个 face，后备字体键另有限额。
- 单个 surface 最大 8192 像素边长、16,777,216 像素。驱动自己的 framebuffer 内存另计。
- 原生字体栅格最多 1024 × 1024；彩色 bitmap strike 按目标字号缩放。
- 模块不自行启动轮询；呈现时机由宿主事件循环决定。

## 验证

执行目录：`artifacts/gpu-text-20261119/artifacts/gpu-gate-4/`。

```sh
cargo fmt --all -- --check
cargo test --offline --locked -p ztty --lib gpu:: -- --ignored --test-threads=1 --show-output
cargo clippy --offline --locked -p ztty --lib --tests -- -D warnings
```

4 个测试实际运行，全部通过。OpenGL renderer 为 **NVIDIA GeForce RTX 4060 Laptop GPU/PCIe/SSE2**。这些测试需要显式运行；普通默认 test 调用会跳过，不能把编译成功当作执行成功。

测试覆盖原生字体整形、GL 像素输出、多窗口资源生命周期、字符簇绘制和缓存复用。验证期间源码前后摘要相同。完整测试名、源码摘要和每步日志摘要见同目录的 `validation.json`。

本轮尚未覆盖正常 X11 / Wayland 窗口的完整用户流程，也不是最终发布性能基准。

## 保留的失败记录

初次字体探针错误选择 emoji face，导致 ASCII 缺字。原因是同一 Fontconfig pattern 被重复执行配置替换；修正为保留原始 pattern，每次匹配只替换一次。随后 6 组原生字体检查通过。原始失败与重测分别保存在 `font-probe` 和 `font-probe-2`。

GPU gate 1 在锁文件检查停止，未执行图形测试。gate 2 和 gate 3 的 4 个图形测试均通过，但严格 Clippy 分别指出生产代码和测试的固定长度像素分块写法。统一使用数组分块后，gate 4 全部通过。没有放宽 lint，也没有删除失败日志。

本提交不更换正在运行的终端进程，不发布版本，不声称所有工作负载都更快。窗口集成后会用最终候选重新测量并记录性能回退。
