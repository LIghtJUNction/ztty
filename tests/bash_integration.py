#!/usr/bin/env python3
"""Check the production Bash integration in isolated, real interactive PTYs."""
import argparse
import errno
import hashlib
import json
import os
from pathlib import Path
import pty
import re
import select
import shlex
import shutil
import signal
import subprocess
import tempfile
import termios
import time
from urllib.parse import unquote_to_bytes, urlsplit

PROMPT = b"__ZTTY_TEST_PROMPT__ "
PROMPT_END = b"\x1b]133;B\x07"
OSC = re.compile(rb"\x1b\](.*?)(?:\x07|\x1b\\)", re.S)


class Bash:
    def __init__(self, binary, home):
        self.transcript = bytearray()
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            attributes = termios.tcgetattr(0)
            attributes[3] &= ~termios.ECHO
            termios.tcsetattr(0, termios.TCSANOW, attributes)
            os.chdir(home)
            environment = {
                "PATH": os.defpath,
                "HOME": str(home),
                "TERM": "xterm-256color",
                "PS1": PROMPT.decode(),
                "PS2": "__ZTTY_TEST_CONTINUE__ ",
                "HISTFILE": "/dev/null",
                "INPUTRC": "/dev/null",
                "LC_ALL": "C.UTF-8",
                "HOSTNAME": os.uname().nodename,
            }
            os.execve(binary, [binary, "--noprofile", "--norc", "--noediting", "-i"], environment)
        self.read_prompt(False)

    def read_prompt(self, protocol=True):
        chunk = bytearray()
        deadline = time.monotonic() + 8
        ending = PROMPT_END if protocol else PROMPT
        while time.monotonic() < deadline:
            if not select.select([self.fd], [], [], 0.05)[0]:
                continue
            try:
                data = os.read(self.fd, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    raise RuntimeError("Bash exited before its prompt") from error
                raise
            if not data:
                raise RuntimeError("Bash closed its PTY before its prompt")
            chunk.extend(data)
            self.transcript.extend(data)
            if len(self.transcript) > 1024 * 1024:
                raise RuntimeError("Bash transcript exceeded 1 MiB")
            if chunk.endswith(ending):
                return bytes(chunk)
        raise TimeoutError("Bash prompt timed out: " + repr(chunk[-800:]))

    def command(self, text, protocol=True):
        os.write(self.fd, text.encode() + b"\n")
        return self.read_prompt(protocol)

    def close(self):
        try:
            os.kill(self.pid, signal.SIGHUP)
        except ProcessLookupError:
            pass
        os.close(self.fd)
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline:
            if os.waitpid(self.pid, os.WNOHANG)[0]:
                return
            time.sleep(0.01)
        os.kill(self.pid, signal.SIGKILL)
        os.waitpid(self.pid, 0)


def markers(data, prefix):
    return [value for value in OSC.findall(data) if value.startswith(prefix)]


def install(session, source, repeat=False):
    label = "REPEAT" if repeat else "FIRST"
    segment = session.command(
        f"source {shlex.quote(str(source))}; _ztty_source_result=$?; "
        f"printf '__ZTTY_SOURCE_{label}__:%s\\n' \"$_ztty_source_result\""
    )
    assert f"__ZTTY_SOURCE_{label}__:0\r\n".encode() in segment, segment
    assert b"syntax error" not in segment and b"command not found" not in segment, segment
    return segment


def check_command(session, command, status, form):
    segment = session.command(command)
    assert markers(segment, b"133;C") == [b"133;C"], segment
    assert markers(segment, b"133;D") == [f"133;D;{status}".encode()], segment
    assert markers(segment, b"133;A") == [b"133;A"], segment
    assert markers(segment, b"133;B") == [b"133;B"], segment
    assert segment.count(b"__ZTTY_OLD_PS0__") == 1, segment
    if form != "unset":
        assert segment.count(f"__ZTTY_HOOK_ONE__:{status}\r\n".encode()) == 1, segment
        second_status = 17 if form == "string" else status
        assert segment.count(f"__ZTTY_HOOK_TWO__:{second_status}\r\n".encode()) == 1, segment
    return segment


def scenario(form, binary, source, artifacts, checks):
    with tempfile.TemporaryDirectory(prefix="ztty-bash-") as temporary:
        home = Path(temporary)
        session = Bash(binary, home)
        try:
            one = "_ztty_original_one"
            two = "_ztty_original_two"
            definitions = (
                f"{one}() {{ local result=$?; printf '__ZTTY_HOOK_ONE__:%s\\n' \"$result\"; return 17; }}; "
                f"{two}() {{ local result=$?; printf '__ZTTY_HOOK_TWO__:%s\\n' \"$result\"; "
                f"PS1={shlex.quote(PROMPT.decode())}; return 23; }}; "
            )
            if form == "array":
                assignment = f"PROMPT_COMMAND=({one} {two})"
            elif form == "string":
                # Keep a complete multiline string, including its trailing comment.
                assignment = "PROMPT_COMMAND=" + shlex.quote(one + "\n" + two + " # existing hook")
            else:
                assignment = "unset PROMPT_COMMAND"
            session.command(
                definitions + assignment + "; PS0='__ZTTY_OLD_PS0__'; "
                "trap ': # ztty-existing-debug-trap' DEBUG",
                protocol=False,
            )
            baseline = session.command("/bin/sh -c 'exit 7'", protocol=False)
            if form != "unset":
                assert b"__ZTTY_HOOK_ONE__:7\r\n" in baseline, baseline
                second_status = 17 if form == "string" else 7
                assert f"__ZTTY_HOOK_TWO__:{second_status}\r\n".encode() in baseline, baseline
            install(session, source)
            repeated = install(session, source, repeat=True)
            assert markers(repeated, b"133;C") == [b"133;C"], repeated
            check_command(session, "true", 0, form)
            check_command(session, "/bin/sh -c 'exit 7'", 7, form)
            trap = check_command(session, "trap -p DEBUG", 0, form)
            assert b"ztty-existing-debug-trap" in trap, trap
            checks.append(f"{form}: successful first/repeated source; one start/end marker; original PS0, prompt, DEBUG trap and exit status retained")
            if form != "unset":
                checks.append(f"{form}: both existing prompt hooks run once; nonzero hook/command statuses match the original Bash behavior")

            destination = home / "with space-界#%?;$"
            destination.mkdir()
            moved = check_command(session, "cd " + shlex.quote(str(destination)), 0, form)
            directories = markers(moved, b"7;")
            assert len(directories) == 1, moved
            uri = directories[0][2:].decode()
            parsed = urlsplit(uri)
            assert parsed.scheme == "file" and parsed.netloc == os.uname().nodename, uri
            assert unquote_to_bytes(parsed.path) == os.fsencode(destination), uri
            assert not parsed.query and not parsed.fragment, uri
            for encoded in ["%20", "%E7%95%8C", "%23", "%25", "%3F", "%3B", "%24"]:
                assert encoded in parsed.path, uri
            checks.append(f"{form}: OSC 7 preserves spaces, Unicode, percent, question mark, hash, semicolon and dollar via URI escaping")

            if form == "array":
                # A nested interactive Bash receives exported PS0 verbatim.
                session.command("export PS0")
                entered = session.command(
                    f"PS1={shlex.quote(PROMPT.decode())} {shlex.quote(binary)} "
                    f"--noprofile --rcfile {shlex.quote(str(source))} --noediting -i"
                )
                assert b"syntax error" not in entered, entered
                nested = session.command("printf '__ZTTY_NESTED__\\n'")
                assert b"__ZTTY_NESTED__\r\n" in nested, nested
                assert markers(nested, b"133;C") == [b"133;C"], nested
                assert nested.count(b"__ZTTY_OLD_PS0__") == 1, nested
                install(session, source, repeat=True)
                session.command("exit 0")
                check_command(session, "/bin/sh -c 'exit 7'", 7, "array")
                checks.append("nested: exported PS0 receives one execution marker; re-source is idempotent; parent hooks/status survive child exit")
        finally:
            artifacts.mkdir(parents=True, exist_ok=True)
            (artifacts / (form + ".pty")).write_bytes(session.transcript)
            session.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--source", type=Path, default=Path(__file__).resolve().parents[1] / "shell/ztty.bash")
    parser.add_argument("--bash", default=shutil.which("bash"))
    parser.add_argument("--output", type=Path, default=Path("artifacts/shell-recovery-20261118/bash.json"))
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    binary = str(Path(args.bash).resolve(strict=True))
    artifacts = args.output.parent / "bash-pty"
    report = {
        "schema": "ztty.bash-integration.v1",
        "ok": False,
        "platform": os.uname().sysname,
        "source_path": str(source),
        "source_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
        "test_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "bash_version": subprocess.check_output([binary, "--version"], text=True).splitlines()[0],
        "checks": [],
    }
    try:
        syntax = subprocess.run([binary, "-n", str(source)], capture_output=True, text=True, env={**os.environ, "LC_ALL": "C"})
        report["syntax"] = {"exit_code": syntax.returncode, "stdout": syntax.stdout, "stderr": syntax.stderr}
        assert syntax.returncode == 0, syntax.stderr
        report["checks"].append("production script passes bash -n before any PTY is started")
        for form in ["array", "string", "unset"]:
            scenario(form, binary, source, artifacts, report["checks"])
        plain = subprocess.run(
            [binary, "--noprofile", "--norc", "-c",
             "PROMPT_COMMAND=original; PS0=original; source \"$1\"; "
             "[[ $? == 0 && $PROMPT_COMMAND == original && $PS0 == original ]] && "
             "! declare -F _ztty_bash_precmd && printf 'UNCHANGED\\n'", "bash", str(source)],
            capture_output=True, text=True, env={"PATH": os.defpath, "LC_ALL": "C.UTF-8"},
        )
        assert plain.returncode == 0 and plain.stdout == "UNCHANGED\n", (plain.stdout, plain.stderr)
        report["checks"].append("noninteractive sourcing is inert and emits no terminal controls")
        report["ok"] = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        report["transcripts"] = {
            path.name: {"bytes": path.stat().st_size, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
            for path in sorted(artifacts.glob("*.pty"))
        }
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
        print(json.dumps(report, ensure_ascii=False, indent=2))
    raise SystemExit(0 if report["ok"] else 1)


if __name__ == "__main__":
    main()
