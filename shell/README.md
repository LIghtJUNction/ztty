# Shell integration

Shell integration lets ztty locate prompts, distinguish typed commands from their
output, record exit status, and track the shell's working directory. It is opt-in:
source the matching file in an interactive shell or its startup configuration.
The scripts do not edit configuration files, install SSH/sudo wrappers, or run
commands on the user's behalf.

## Enable it

From the ztty checkout, use the command for the current shell:

| Shell | Command | Implementation |
| --- | --- | --- |
| Bash 4.4+ | `source /absolute/path/to/ztty/shell/ztty.bash` | PS0 execution marker and preserved PROMPT_COMMAND hooks |
| Zsh | `source /absolute/path/to/ztty/shell/ztty.zsh` | Existing precmd/preexec hook arrays plus prompt markers |
| Fish 4+ | `source /absolute/path/to/ztty/shell/ztty.fish` | Fish's native OSC 133 marks plus an independent OSC 7 hook |
| Nushell 0.94+ | `source /absolute/path/to/ztty/shell/ztty.nu` | Native `shell_integration.osc133` and `osc7` settings |

Use the same source command in a nested interactive shell that needs integration.
Bash/Zsh/Fish installation is idempotent per shell; Nushell sets only its two
native integration options. Bash's DEBUG trap, existing prompt commands, Zsh
hook functions, Fish prompt functions and Nushell hooks remain in place.
Prompt themes can change PS1 in the existing Bash/Zsh prompt hooks; ztty applies
its nonprinting markers afterwards.

Bash 4.4 introduced the PS0 hook used here. Older Bash versions, including the
system Bash on some macOS installations, receive an explicit unsupported-version
message. They remain usable as ordinary shells. Fish versions before 4 receive
the same explicit boundary. The checked-in acceptance report records exact
versions actually tested.

## Markers and working directories

The shell writes OSC 133 A before the prompt, B before command input, C before
execution, and D after completion. D may include an unsigned exit status.
Secondary A markers with `k=s` remain part of the same multiline command.
A primary prompt without a preceding D closes the previous output region with
an unknown exit status. Repeated prompt redraws do not create completed commands.

OSC 7 carries a percent-encoded file URI. Bash/Zsh/Fish preserve the shell's
hostname so a remote directory cannot silently become a local path; Nushell
uses its native URI implementation. These markers are untrusted terminal
metadata. They do not authenticate a shell, grant control, or authorize access
to a directory. A frontend must validate the URI host and decoded absolute path
before inheriting a working directory.

## Terminal model API

`zterm::Terminal` exposes:

| API | Result |
| --- | --- |
| `commands()` | Records from oldest to newest; also supports reverse iteration |
| `shell_phase()` | Unknown, prompt, input, or output |
| `command_scroll_offset(id)` | A scrollback offset for a retained prompt, or None |
| `command_input(id, max_bytes)` | Rendered input between B and C |
| `command_output(id, max_bytes)` | Rendered output between C and D/the next prompt |

A record contains a stable session-local ID, absolute line/column boundaries,
an optional exit status, and an availability flag. A text result contains
`text`, `complete`, and `truncated`.
The terminal retains at most 128 metadata records and allocates no record
buffer until a marker arrives. Text reads reuse the existing primary screen
and scrollback, allocate only their result, and return at most 65,536 UTF-8
bytes without splitting a character.

Extraction follows physical screen lines and trims trailing blank cells.
It is the current rendered text, not an immutable stdout archive. History
eviction and byte limits report truncation. A width change or destructive
screen operation that invalidates a recorded range returns empty text with
truncation rather than selecting unrelated cells. Vertical scrolling and
height-only resizing preserve retained anchors. Reset clears all records
without reusing their IDs. Alternate-screen OSC 133 messages are ignored;
a TUI's alternate-screen contents are not included in a primary command range.

The terminal does not infer an enclosing-command hierarchy for nested shells.
Each source installation reports the visible shell's markers. SSH, sudo,
containers, process restoration, and asynchronous prompt frameworks require
their own end-to-end acceptance before their corresponding roadmap items
can be marked complete.

## Verify

Build the probe before running the PTY acceptance test:

```sh
cargo test --locked -p zterm --lib
cargo build --locked -p zterm --example shell_probe
python3 tests/shell_integration.py --report tests/shell_integration_linux.json
```

The Python runner starts fresh Bash/Zsh/Fish/Nushell processes in private PTYs
without loading the user's startup files or writing command history. It sources
the adapter twice, preserves a pre-existing prompt/hook, runs successful and
failing commands, checks UTF-8 output, and changes into a directory containing
spaces and Unicode. Its terminal replies come from the live production zterm
parser, including accurate cursor-position reports. A stale probe is rejected
before starting shells.

The report records each shell's status and version plus the size and SHA-256
of its PTY transcript. Raw transcripts remain under the ignored
`artifacts/shell-integration/` directory. Missing shell executables are
reported as blocked, and the runner exits unsuccessfully until every requested
shell passes. This test exercises real Linux PTYs; it does not replace native
macOS/Windows GUI, input-method, SSH, or container acceptance.

## Protocol sources

- [Kitty shell integration marker protocol](https://sw.kovidgoyal.net/kitty/shell-integration/#notes-for-shell-developers)
- [GNU Bash prompt variables](https://www.gnu.org/software/bash/manual/html_node/Bash-Variables.html)
- [Zsh hook ordering and exit-status rules](https://zsh.sourceforge.io/Doc/Release/Functions.html)
- [Fish terminal compatibility](https://fishshell.com/docs/current/terminal-compatibility.html)
- [Nushell terminal integration configuration](https://www.nushell.sh/book/config_reference.html#terminal-integration)
