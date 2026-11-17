# ssmcp observation on Linux

`tools/ztty-observer` opens ssmcp command output in a read-only ztty window without requesting keyboard focus. Sequential commands reuse the live ssmcp viewer and window. Closing a window created for that viewer retires the viewer child; the next ssmcp command can open another observation window.

This integration currently uses Linux X11 or XWayland. The recorded acceptance run used a separately owned Xvfb server, not the user's desktop. It does not establish native Wayland, macOS, Windows, physical input-device or compositor latency acceptance.

## Start an observing connection

Use an already authorized ssmcp account and an executable ztty build:

```sh
ZTTY_BIN="$PWD/target/release/ztty" \
  ssmcp --tty "$PWD/tools/ztty-observer" --name ztty-observed
```

The adapter is a small executable shell script. It selects `${ZTTY_BIN:-ztty}` and passes `--observe` before the terminal arguments supplied by ssmcp. The Rust implementation contains no ssmcp-specific process names or command parsing. See the [adapter](../tools/ztty-observer), [CLI parsing](../src/main.rs) and [Linux session implementation](../src/session.rs).

ssmcp starts the terminal with `-e ssmcp __tty-view DIRECTORY SLOT`. That helper reads the output retained by ssmcp for the real command. There are two PTYs with different jobs: the command's PTY produces stdout/stderr; the viewer's PTY carries the resulting bytes into ztty. Opening the observation window does not execute the command a second time.

## Input and lifetime rules

| Operation | Input and focus | Closing the observing window |
| --- | --- | --- |
| `ztty --observe -e COMMAND ...` | Starts a fresh read-only window; does not request focus. Typing, paste and mouse reporting do not write to its child. Resizing does not resize the observed PTY. | Retires the child created for this window while its input still belongs to the local actor. |
| `ztty attach --observe NAME` | Observes the existing session; attaching to an existing view also cancels queued local input and asynchronous paste. | Detaches the view and preserves the existing PTY, including sessions whose input still belongs to the local actor. |
| Input has been acquired by another actor | The local view cannot operate that actor's PTY. | Preserves the other actor's PTY even if this view originally created it. |
| `ztty close NAME` | Checks the current input owner. | Explicitly retires the named session when the caller owns it. |

`--observe` is a standalone-window option; combining it with `--tab`, `--split-right` or `--split-down` is rejected. A PTY still has one GUI view. This adapter does not add concurrent independent GUI observers for the same PTY.

The difference between creation and attachment is necessary for reusable command viewers. If closing a freshly created observation window only detaches it, the ssmcp viewer keeps sending its heartbeat while its window is hidden. ssmcp can then reuse that invisible viewer for a later command. The first real integration run reproduced this failure; the session code now remembers whether a view created its child, and clears that ownership when the view becomes an attachment.

## Recorded acceptance

The [pre-fix report](measurements/ssmcp-observer-before-fix-2026-11-18.json) preserves the failure. The first two real native exec commands executed once each and reused the same viewer/window. After `WM_DELETE_WINDOW`, the third command completed with exit 0, but the session remained detached and no window became visible during the 12-second observation interval. The run cleaned up all its processes.

The [final corrected run, run-03](measurements/ssmcp-observer-x11-2026-11-18.json), executes three actual native exec commands and records ten passing checks. It uses the candidate containing both the observation-window lifetime fix and the configuration-watch fix. Its hashes are:

| Artifact | SHA-256 |
| --- | --- |
| ztty candidate | `a339a7d258ad313379b5cfea40d8b125aa8c810ecaebd40005b489413a1b6d4d` |
| Installed ssmcp executable | `e7e3d84b6afc869c5e340fbf9cf0c8c4092e60f6ce68c85379b4b64e8cc0c048` |
| Executed `tests/ssmcp_observer.py` | `370abf802ea5edddaf93106f2b45f6b7291787b775d0e3d42587a1722a764d0d` |

The executed test hash matches the source-controlled script, including the new-session incarnation assertions and the second screenshot capture. The passing checks are:

1. Reused existing account authorization without provisioning an account or reading credentials.
2. Displayed the actual command's PTY output in a fresh read-only window without taking X11 focus.
3. Verified that ztty ran the installed `ssmcp __tty-view` process for the exact native exec slot.
4. Injected keyboard, paste-shortcut and mouse events with XTEST; the viewer PTY input queue stayed empty and ztty did not request clipboard contents.
5. Resized the observation window and checked that the viewer PTY dimensions stayed unchanged.
6. Ran a second command in the same ssmcp slot, viewer process and ztty window without taking focus.
7. Recorded exactly one execution per command and confirmed that both stdout and stderr were actual PTYs.
8. Verified both commands' exit codes and their exact combined PTY output.
9. Closed the window and verified that a third real command obtained a visible read-only observer without taking focus or rerunning the command.
10. Closed the private observation session explicitly and verified that no test-owned processes remained.

X11 resource numbers may be reused after a display connection closes. Compare the broker PID, session creation time and viewer PID when distinguishing the reopened window from the previous one; the numeric `window_id` alone is insufficient.

The [first screenshot](assets/ssmcp-observer-x11-2026-11-18.png) captures actual pixels from the first two commands. The [reopened-window screenshot](assets/ssmcp-observer-reopened-x11-2026-11-18.png) captures the third command after closing and recreating its observer. Both were inspected for readable text, natural wrapping and the intended black background with padding. The JSON separately records `new_session_incarnation=true`, the three actual executions and an empty remaining-process list.

The independent `tests/daily_workflow.py` suite also exercises both lifetime branches without requiring an ssmcp account: closing an observer attached to an already visible, locally owned session preserves the same functioning PTY; closing a fresh `--observe` window removes its own session and reaps its child. Existing checks continue to cover an agent-owned PTY and explicit local takeover. The [final daily-workflow report](measurements/daily-workflow-observer-config-watch-2026-11-18.json) records fourteen passing checks against the same ztty candidate hash, including actual configuration-directory rename and recreation.

An [earlier daily-workflow failure](measurements/config-watch-before-fix-2026-11-18.json) is retained separately: the lifecycle-fixed candidate timed out while waiting for the new background after configuration-directory recreation. It had completed seven checks and had not reached the new observer lifecycle assertions. The final report uses the subsequent configuration-watch fix; no timeout or pre-existing assertion was relaxed to obtain its result.

The reports retain binary hashes and source-file hashes. An acceptance report made from uncommitted source records that state explicitly; it is not a substitute for the clean-source native release gate or cross-platform acceptance.

## Reproduce the real connection test

Requirements are Linux, an installed and already authorized ssmcp client with `--tty`, Xvfb, Python with `python-xlib` and Pillow, and the candidate ztty binary. The test refuses to provision authorization. It creates an ephemeral connection with `--no-browser --no-setup --no-watchdog`, uses an owned random display through `tools/isolated_xvfb.py`, and creates a private broker socket. It never invokes an ssmcp model or agent tool.

Start the coordinator from the source tree. Use a new control directory for every run:

```sh
python3 tests/ssmcp_observer.py \
  --binary target/release/ztty \
  --source-root . \
  --ssmcp /absolute/path/to/ssmcp \
  --control-dir artifacts/ssmcp-observer-run \
  --output artifacts/ssmcp-observer.json \
  --screenshot artifacts/ssmcp-observer.png
```

Use ssmcp's authorized client listing to find the unique `client_name` written to `ready.json`. Call only that new client's native `ssmcp.exec.start`, `ssmcp.exec.wait` and `ssmcp.exec.read` tools. Run each command array exactly as supplied; the command appends a single execution record, prints harmless markers and waits for the coordinator's completion signal.

| Coordinator file | Controller action | Receipt written into the control directory |
| --- | --- | --- |
| `ready.json` | Start `first_command` on the matching ephemeral client. | `first-start.json`: `job_id`, `output_mode`, `tty_slot` from the start result. |
| `await-second.json` | Wait/read the first job, then start `second_command`. | `first-done.json`: `job_id`, `exit_code`, `running`, `stdout`, `stderr`; then `second-start.json` with the three start fields. |
| `await-finish.json` | Wait/read the second job. | `second-done.json` with the five completion fields. |
| `await-third.json` | Start `third_command` after the coordinator has closed the original window. | `third-start.json` with the three start fields. |
| `await-third-finish.json` | Wait/read the third job. | `third-done.json` with the five completion fields. |
| `finished.json` | Read the completed report and inspect its screenshot. | No further calls on the ephemeral client; the coordinator disconnects it. |

Write receipts through the original authorized development connection or directly on the test machine. Do not run extra receipt-writing commands on the observed client: those commands would themselves create observations and invalidate the exact command count. Receipts contain only the fields listed above; no account configuration, authorization URL or credential is copied into evidence.

The coordinator has a bounded overall deadline. On success, failure or interruption it stops only its own processes, checks process birth tokens before sending signals, releases its owned Xvfb and deletes private account connection logs. A failure remains a failing report; a screenshot of an earlier passing stage does not override it.

The account-independent GUI regressions run with:

```sh
python3 tests/daily_workflow.py \
  --binary target/release/ztty \
  --output artifacts/daily-workflow.json \
  --screenshots artifacts/daily-workflow
```

These acceptance tools are source-controlled. Real account integration stays separate from the unattended release gate because it requires an existing external account session; the corresponding local observer lifecycle checks run in the native Linux GUI gate.
