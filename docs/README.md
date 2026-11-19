# Documentation

The READMEs cover installation and common controls:
[English](../README.md) · [中文](../README.zh-CN.md).

## Use ztty

| Guide | Contents |
| --- | --- |
| [Configuration](configuration.md) | Fonts, colors, X11 renderer, Wayland setup, keys and text actions |
| [Native Wayland](native-wayland.zh-CN.md) | Explicit experimental single window and current platform limits |
| [Daily workflow](daily-workflow.md) | Search, command palette, live configuration and shell commands |
| [Workspaces](workspaces.md) | Tabs, splits, pane movement and saved layouts |
| [Sessions and MCP](sessions.md) | Shared sessions, attach, input ownership and MCP clients |
| [Observer mode](ssmcp-observer.md) | Read-only windows and ssmcp integration |
| [Shell integration](../shell/README.md) | Bash, Zsh, Fish and Nushell setup |
| [Startup directory](startup-directory.md) | Shell selection and working-directory rules |
| [Input compatibility](input-compatibility.md) | Keypad, keyboard protocols and mouse behavior |
| [Text extensions](text-plugins.zh-CN.md) | Trusted native actions and worker lifecycle |
| [Dedent example](../examples/dedent-plugin/README.md) | Build a text action independently |

## Implementation and measurement

| Reference | Contents |
| --- | --- |
| [Architecture](architecture.md) | Terminal model, libraries and resource ownership |
| [Reusable libraries](../README.md#reusable-libraries) | Independent repositories, component responsibilities and release status |
| [Protocols](protocols.md) | Implemented sequences, graphics and platform acceptance |
| [Session core](session-core.md) | PTY reads, input queues, sizing and ownership |
| [Native output bytes](native-output-bytes.zh-CN.md) | Lossless MCP output and readable text |
| [Darwin PTY helper](darwin-threaded-pty.md) | Native macOS process creation |
| [Current benchmark report](benchmarks/resume-2026-11-20.md) | Candidate identity, workloads and raw measurements |
| [Benchmark instructions](benchmarks/README.md) | Workloads, accounting and reproduction |
| [Performance](performance.md) | Design and dated measurements |
| [Releasing](releasing.md) | Native builds, packages and published versions |

## Dated verification records

These reports retain the source, executable and environment used for each run.

| Area | Record |
| --- | --- |
| X11 rendering | [GPU text and complete frames](reports/x11-gpu-20261119/report.zh-CN.md) |
| Native Wayland | [Single-window acceptance](measurements/native-wayland-2026-11-19/README.md) |
| Kitty and OSC 52 | [Linux GPU/software integration](reports/graphics-extensions-20261120/report.zh-CN.md) |
| Input method | [XIM lifecycle and recovery](xim-recovery-2026-11-19.zh-CN.md) |
| Input and controls | [Keypad routing](keypad-input-2026-11-19.zh-CN.md), [shortcut verification](shortcuts-validation-2026-11-19.zh-CN.md) |
| Clipboard | [Copy/paste behavior](copy-paste-2026-11-19.zh-CN.md) |
| Terminal output | [2026-11-19 throughput](output-throughput-2026-11-19.zh-CN.md), [native raw output](native-output-bytes.zh-CN.md) |
| History and scrolling | [Performance and function checks](performance-function-2026-11-19.zh-CN.md) |
| Shell and sessions | [Shell resume](shell-resume-2026-11-18.md), [Unicode integration](integration-unicode-2026-11-18.md) |
| Daily workflow | [Experience verification](experience-release-2026-11-18.md) |
| Platform coverage | [Native candidates](native-candidates-2026-11-18.md), [native UI](native-ui-progress-2026-11-19.zh-CN.md) |
| Images | [Sixel GUI acceptance](sixel-gui-acceptance.md) |
| Release 0.4.1 | [Keypad release and installation](numpad-release-0.4.1-2026-11-19.zh-CN.md) |
| Earlier comparison | [Shared idle windows](benchmarks/guarded-idle-1-10-2026-11-18.md), [raw JSON](benchmarks/guarded-idle-1-10-2026-11-18.json), [chart](benchmarks/guarded-idle-1-10-2026-11-18.svg) |
| Earlier output tests | [Throughput](benchmarks/throughput-2026-11-18.md), [redraw batching](benchmarks/repaint-ab-2026-11-18.md) |

Reports describe their own revisions. Their results stay separate from a later
source checkout or release package.
