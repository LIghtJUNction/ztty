# Command startup directory

`ztty --cwd PATH -e COMMAND [ARG...]` selects the command's initial directory.
The shared Linux broker applies the setting to the new session. Native macOS
and Windows single-PTY processes enter it once, before resolving the command,
starting PTY workers or initializing GUI frameworks.

```sh
ztty --cwd "/path/with spaces/项目" -e ./project-shell
```

Paths may be relative to the launching process. A relative command path is
resolved inside the selected directory. A relative `--config` path is loaded
before that switch and remains relative to the launcher's original directory.
Missing, non-directory or empty paths fail before starting the child and
report the original operating-system error number. The launching shell and
other ztty processes retain their own working directories.

On Linux and macOS the child environment's `PWD` reflects the physical
working directory. Darwin obtains that path with libSystem `getcwd`, copies
the bytes into the child environment and frees the C allocation with
libSystem `free`. Windows uses the Unicode `SetCurrentDirectoryW` API, with
UTF-16 conversion and NUL termination; the native ConPTY child inherits this
process directory.

The startup-only native implementation does not implement shared native
sessions. Future concurrent creation must pass cwd to each child spawn instead
of changing a running multithreaded process's global directory. Microsoft
documents this boundary for
[GetCurrentDirectory](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-getcurrentdirectory)
and [SetCurrentDirectory](https://learn.microsoft.com/en-us/windows/win32/api/winbase/nf-winbase-setcurrentdirectory).
Apple documents the allocating
[getcwd](https://developer.apple.com/library/archive/documentation/System/Conceptual/ManPages_iPhoneOS/man3/getcwd.3.html)
variant.

## Acceptance

Linux's real MCP/session tests cover independent cwd/argv/environment for
separate sessions. Four unprivileged libc tests cover cwd save/restore,
execute-only directories, invalid targets and descriptor exhaustion.

The Windows native harness adds missing/file/empty cwd rejection with a child
sentinel, relative native executable lookup, and a Unicode/space-containing
cwd while loading a relative appearance file. The original pixel vectors
remain unchanged and therefore also verify the selected configuration.
The macOS native harness checks actual cwd and `PWD` by file identity, the
relative config path, invalid cwd and the original eight PTY checks before
continuing to window pixels.

The native additions require execution on both CPU architectures. A Linux
cross-target `cargo check` proves type checking only. See the
[native candidate record](native-candidates-2026-11-18.md) for the latest
executed results and blockers.
