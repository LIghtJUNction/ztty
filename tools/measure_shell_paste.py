#!/usr/bin/env python3
"""Separate PTY delivery from ZLE processing using synthetic, nonexecuted text.

This uses owned PTYs, not ztty or a clipboard. Compare its application timings
with paste_pipeline_gui.py's real clipboard/raw-PTY timings. The zsh fixture
edits a variable with vared; pasted contents are never evaluated as commands.
"""
import argparse
import contextlib
from decimal import Decimal
import fcntl
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import subprocess
import sys
import tempfile
import termios
import time
import tty


START = b"\x1b[200~"
END = b"\x1b[201~"
ZSH_FIXTURE = r'''
_paste_probe_run() {
  typeset -g _PASTE_PROBE_DIR=$1 _PASTE_PROBE_PENDING=0 _PASTE_PROBE_VALUE=''
  zmodload zsh/datetime
  zmodload zsh/zleparameter
  autoload -Uz add-zle-hook-widget
  print -r -- "${widgets[bracketed-paste]-unknown}" > "$_PASTE_PROBE_DIR/widget.txt"
  print -r -- "${widgets[self-insert]-unknown}" > "$_PASTE_PROBE_DIR/self-insert-widget.txt"
  print -r -- "$ZSH_VERSION" > "$_PASTE_PROBE_DIR/zsh-version.txt"
  zle -A bracketed-paste _paste_probe_original
  zle -N bracketed-paste _paste_probe_bracketed
  zle -N _paste_probe_finish
  bindkey '^]' _paste_probe_finish
  add-zle-hook-widget line-init _paste_probe_init
  add-zle-hook-widget line-pre-redraw _paste_probe_redraw
  vared -p '' _PASTE_PROBE_VALUE
}
_paste_probe_init() {
  print -r -- "$EPOCHREALTIME" > "$_PASTE_PROBE_DIR/ready"
}
_paste_probe_bracketed() {
  typeset -g _PASTE_PROBE_ENTER=$EPOCHREALTIME
  zle _paste_probe_original -- "$@"
  typeset -g _PASTE_PROBE_RETURN=$EPOCHREALTIME _PASTE_PROBE_PENDING=1
}
_paste_probe_redraw() {
  if (( _PASTE_PROBE_PENDING )); then
    local hooks_ready=$EPOCHREALTIME
    print -rn -- "$BUFFER" > "$_PASTE_PROBE_DIR/buffer"
    print -r -- "$_PASTE_PROBE_ENTER $_PASTE_PROBE_RETURN $hooks_ready" > "$_PASTE_PROBE_DIR/application-ready"
    _PASTE_PROBE_PENDING=0
  fi
}
_paste_probe_finish() { zle .accept-line }
'''


def record_identity(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return {"pid": pid, "start_ticks": int(fields[19])}


def receiver(directory, expected_file):
    tty.setraw(0)
    expected = expected_file.read_bytes()
    received = bytearray()
    first = last = None
    identity = record_identity(os.getpid())
    (directory / "ready").write_text(str(time.monotonic_ns()))
    while len(received) < len(expected):
        chunk = os.read(0, min(65536, len(expected) - len(received)))
        if not chunk:
            raise RuntimeError("PTY closed before complete input")
        stamp = time.monotonic_ns()
        first = stamp if first is None else first
        last = stamp
        received.extend(chunk)
    result = {**identity, "first_read_ns": first, "last_read_ns": last,
              "received_bytes": len(received), "byte_exact": received == expected,
              "sha256": hashlib.sha256(received).hexdigest()}
    (directory / "application-ready").write_text(json.dumps(result))
    return 0 if result["byte_exact"] else 1


def payload(size, unicode):
    line = ("# synthetic paste 中文边界 0123456789\n" if unicode
            else "# synthetic paste ascii 0123456789\n").encode()
    return (line * (size // len(line) + 1))[:size].decode("utf-8", "ignore").encode()


def run_case(root, variant, size, unicode, args):
    name = f"{variant}-{'utf8' if unicode else 'ascii'}-{size}"
    directory = root / name
    directory.mkdir()
    text = payload(size, unicode)
    expected = START + text + END
    expected_file = directory / "expected-wire.bin"
    expected_file.write_bytes(expected)
    env = os.environ.copy()
    env.pop("LD_PRELOAD", None)
    env.update(TERM="xterm-256color", LC_ALL="C.UTF-8", HISTFILE="/dev/null",
               XDG_STATE_HOME=str(directory / "state"),
               XDG_CACHE_HOME=str(directory / "cache"), LP_NUM_THREADS="2")
    if variant == "raw":
        command = [sys.executable, str(Path(__file__).resolve()), "--receiver",
                   str(directory), str(expected_file)]
    else:
        setup = 'source "$1"; '
        if variant == "zshard":
            setup += 'source "$3/zshard.zsh"; '
        setup += '_paste_probe_run "$2"'
        if variant == "suite":
            command = [str(args.suite_root / "bin/omztty"), "shell", "-i", "-c", setup,
                       "paste-probe", str(root / "fixture.zsh"), str(directory)]
        else:
            command = [str(args.zsh), "-d", "-f", "-i", "-c", setup,
                       "paste-probe", str(root / "fixture.zsh"), str(directory)]
            if variant == "zshard":
                command.append(str(args.zshard_root))
    master, slave = os.openpty()
    fcntl.ioctl(slave, termios.TIOCSWINSZ, (24).to_bytes(2, sys.byteorder)
                + (100).to_bytes(2, sys.byteorder) + b"\0" * 4)

    def establish_terminal():
        os.setsid()
        fcntl.ioctl(0, termios.TIOCSCTTY, 0)

    process = None
    result = {"name": name, "variant": variant, "payload_bytes": len(text),
              "wire_bytes": len(expected), "payload_sha256": hashlib.sha256(text).hexdigest(),
              "passed": False, "scope": "owned kernel PTY; excludes ztty and clipboard"}
    output = bytearray()
    try:
        process = subprocess.Popen(command, stdin=slave, stdout=slave, stderr=slave,
                                   env=env, preexec_fn=establish_terminal)
        os.close(slave)
        slave = -1
        result["identity"] = record_identity(process.pid)
        os.set_blocking(master, False)
        deadline = time.monotonic() + args.timeout
        cursor = 0
        first_write = last_write = None
        started = False
        epoch_offset = time.time_ns() - time.monotonic_ns()
        while time.monotonic() < deadline:
            if not started and (directory / "ready").is_file():
                started = True
                result["trigger_ns"] = time.monotonic_ns()
            readable, writable, _ = select.select(
                [master], [master] if started and cursor < len(expected) else [], [], 0.01)
            if writable:
                try:
                    count = os.write(master, expected[cursor:cursor + 65536])
                    now = time.monotonic_ns()
                    first_write = now if first_write is None else first_write
                    cursor += count
                    if cursor == len(expected):
                        last_write = now
                except BlockingIOError:
                    pass
            if readable:
                try:
                    chunk = os.read(master, 65536)
                    if len(output) < 8192:
                        output.extend(chunk[:8192 - len(output)])
                except (BlockingIOError, OSError):
                    pass
            ready = directory / "application-ready"
            if cursor == len(expected) and ready.is_file() and ready.stat().st_size:
                if variant == "raw":
                    application = json.loads(ready.read_text())
                    result["application"] = application
                    result["byte_exact"] = application["byte_exact"]
                    app_ns = application["last_read_ns"]
                else:
                    stamps = ready.read_text().split()
                    if len(stamps) != 3:
                        continue
                    parsed = [int(Decimal(stamp) * 1000000000) - epoch_offset for stamp in stamps]
                    result["application"] = dict(zip(
                        ("zle_widget_enter_ns", "zle_widget_return_ns", "zle_redraw_hooks_ready_ns"), parsed))
                    result["clock_note"] = "ZLE EPOCHREALTIME mapped once to monotonic; does not measure visible render completion"
                    result["widget"] = (directory / "widget.txt").read_text().strip()
                    result["self_insert_widget"] = (directory / "self-insert-widget.txt").read_text().strip()
                    result["zsh_version"] = (directory / "zsh-version.txt").read_text().strip()
                    actual = (directory / "buffer").read_bytes()
                    result["received_payload_bytes"] = len(actual)
                    result["byte_exact"] = actual == text
                    result["received_payload_sha256"] = hashlib.sha256(actual).hexdigest()
                    app_ns = parsed[-1]
                    os.write(master, b"\x1d")
                result.update(first_master_write_ns=first_write, last_master_write_ns=last_write,
                              wire_write_ms=(last_write - first_write) / 1000000,
                              trigger_to_application_ms=(app_ns - result["trigger_ns"]) / 1000000,
                              last_write_to_application_ms=(app_ns - last_write) / 1000000)
                result["passed"] = result["byte_exact"]
                break
            if process.poll() is not None:
                raise RuntimeError(f"receiver exited before result: {process.returncode}")
        else:
            raise TimeoutError("PTY/application stage timed out")
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
        result["startup_output"] = output.decode("utf-8", "replace")[-2048:]
    finally:
        if process is not None:
            with contextlib.suppress(subprocess.TimeoutExpired):
                process.wait(timeout=0.3)
            if process.poll() is None:
                os.killpg(process.pid, signal.SIGTERM)
                try:
                    process.wait(timeout=2)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid, signal.SIGKILL)
                    process.wait(timeout=2)
            result["exit_code"] = process.returncode
        os.close(master)
        if slave >= 0:
            os.close(slave)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--zsh", type=Path, default=Path("/usr/bin/zsh"))
    parser.add_argument("--suite-root", type=Path)
    parser.add_argument("--zshard-root", type=Path)
    parser.add_argument("--sizes", nargs="+", type=int, default=[65536, 262144])
    parser.add_argument("--timeout", type=float, default=20)
    args = parser.parse_args()
    for name in ("suite_root", "zshard_root"):
        if getattr(args, name) is not None:
            setattr(args, name, getattr(args, name).resolve())
    args.output.mkdir(parents=True, exist_ok=True)
    variants = ["raw", "clean-zsh"]
    if args.zshard_root is not None:
        variants.append("zshard")
    if args.suite_root is not None:
        variants.append("suite")
    report = {"schema": 1, "passed": False, "checks": [],
              "test_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "limits": ["No ztty or clipboard timing in this script",
                         "vared stores the pasted text in a variable without execution",
                         "ZLE redraw hook completion is not visible display completion",
                         "master write completion means kernel acceptance, not child receipt"]}
    with tempfile.TemporaryDirectory(prefix="ztty-paste-shell-") as temporary:
        root = Path(temporary)
        (root / "fixture.zsh").write_text(ZSH_FIXTURE)
        for size in args.sizes:
            for unicode in (False, True):
                for variant in variants:
                    case = run_case(root, variant, size, unicode, args)
                    report["checks"].append(case)
                    print(json.dumps({key: case[key] for key in
                                      ("name", "passed", "trigger_to_application_ms", "error") if key in case}), flush=True)
    report["passed"] = all(case["passed"] for case in report["checks"])
    (args.output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--receiver":
        raise SystemExit(receiver(Path(sys.argv[2]), Path(sys.argv[3])))
    raise SystemExit(main())
