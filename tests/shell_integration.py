#!/usr/bin/env python3
"""Run real interactive shells in private PTYs, then parse their output with zterm."""

import argparse
import errno
import fcntl
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
import struct
import subprocess
import tempfile
import termios
import time
from urllib.parse import unquote, urlsplit


PROMPT = re.compile(rb"\x1b\]133;B(?:;[^\x07\x1b]*)?(?:\x07|\x1b\\)")
FINISHED = re.compile(rb"\x1b\]133;D(?:;[^\x07\x1b]*)?(?:\x07|\x1b\\)")


class Shell:
    def __init__(self, argv, cwd, probe):
        self.transcript = bytearray()
        self.model = subprocess.Popen([str(probe), "--live"], stdin=subprocess.PIPE, stdout=subprocess.PIPE)
        self.pid, self.fd = pty.fork()
        if self.pid == 0:
            os.chdir(cwd)
            env = dict(os.environ, TERM="xterm-256color", PS1="ZT> ", HISTFILE="/dev/null", LC_ALL="C.UTF-8")
            os.execvpe(argv[0], argv, env)
        fcntl.ioctl(self.fd, termios.TIOCSWINSZ, struct.pack("HHHH", 40, 120, 0, 0))

    def read(self, timeout=0.05):
        if not select.select([self.fd], [], [], timeout)[0]:
            return False
        try:
            chunk = os.read(self.fd, 65536)
        except OSError as error:
            if error.errno == errno.EIO:
                raise RuntimeError("shell exited before returning to its prompt") from error
            raise
        if not chunk:
            raise RuntimeError("shell closed its PTY")
        self.transcript.extend(chunk)
        if len(self.transcript) > 1024 * 1024:
            raise RuntimeError("shell transcript exceeded 1 MiB")
        self.model.stdin.write(struct.pack("<I", len(chunk)) + chunk)
        self.model.stdin.flush()
        header = self.model.stdout.read(4)
        if len(header) != 4:
            raise RuntimeError("production terminal probe exited")
        size = struct.unpack("<I", header)[0]
        response = self.model.stdout.read(size)
        if response:
            os.write(self.fd, response)
        return True

    def settle(self, seconds=0.3):
        deadline = time.monotonic() + seconds
        while time.monotonic() < deadline:
            self.read(min(0.05, max(0, deadline - time.monotonic())))

    def command(self, text):
        start = len(self.transcript)
        os.write(self.fd, text.encode() + b"\r")
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            self.read()
            finished = FINISHED.search(self.transcript[start:])
            if finished and PROMPT.search(self.transcript[start + finished.end():]):
                self.settle(0.08)
                return bytes(self.transcript[start:])
        raise RuntimeError("no OSC 133 B prompt after " + repr(text) + ": " + repr(self.transcript[-600:]))

    def close(self):
        try:
            os.kill(self.pid, signal.SIGHUP)
        except ProcessLookupError:
            pass
        os.close(self.fd)
        os.waitpid(self.pid, 0)
        if self.model.poll() is None:
            self.model.terminate()
            self.model.wait(timeout=3)

    def finish_model(self):
        self.model.stdin.write(struct.pack("<I", 0))
        self.model.stdin.close()
        result = self.model.stdout.read()
        assert self.model.wait(timeout=3) == 0
        return result


def run_shell(name, root, probe, artifact_dir):
    binary = shutil.which(name)
    if not binary:
        return {"shell": name, "status": "blocked", "reason": "executable not installed"}
    args = {
        "bash": [binary, "--noprofile", "--norc", "-i"],
        "zsh": [binary, "-f"],
        "fish": [binary, "--no-config", "--private", "--interactive"],
        "nu": [binary, "--no-config-file", "--no-history", "--interactive"],
    }[name]
    script = root / "shell" / ("ztty." + name)
    source = shlex.quote(str(script))
    if name == "bash":
        syntax = subprocess.run([binary, "-n", str(script)], capture_output=True, text=True, timeout=3)
        if syntax.returncode:
            raise RuntimeError("Bash integration syntax failed: " + syntax.stderr)
        setup = "_ztty_test_hook(){ printf '__ZTTY_EXISTING_HOOK__\\n'; PS1='ZT> '; }; PROMPT_COMMAND=(_ztty_test_hook); trap 'true # ztty-existing-trap' DEBUG; "
    elif name == "zsh":
        setup = "_ztty_test_hook(){ printf '__ZTTY_EXISTING_HOOK__\\n'; PS1='ZT> '; }; precmd_functions=(_ztty_test_hook); RPROMPT=''; "
    elif name == "fish":
        setup = "function fish_prompt; printf 'ZT> '; end; function ztty_old_hook --on-event fish_preexec; printf '__ZTTY_EXISTING_HOOK__\\n'; end; "
    else:
        setup = "$env.PROMPT_COMMAND = { 'ZT> ' }; $env.PROMPT_COMMAND_RIGHT = { '' }; $env.PROMPT_INDICATOR = ''; $env.config.hooks.pre_prompt = [{ print '__ZTTY_EXISTING_HOOK__' }]; "
    session = Shell(args, root, probe)
    with tempfile.TemporaryDirectory(prefix="ztty-shell-") as temp:
        destination = Path(temp) / "with space-界"
        destination.mkdir()
        try:
            session.settle()
            if name == "bash":
                loaded = session.command(
                    setup + f"source {source}; _ztty_source_status=$?; "
                    "printf '__ZTTY_FIRST_SOURCE__:%s\\n' \"$_ztty_source_status\"; "
                    f"source {source}; _ztty_source_status=$?; "
                    "printf '__ZTTY_REPEAT_SOURCE__:%s\\n' \"$_ztty_source_status\""
                )
                assert b"__ZTTY_FIRST_SOURCE__:0\r\n" in loaded, loaded
                assert b"__ZTTY_REPEAT_SOURCE__:0\r\n" in loaded, loaded
            else:
                session.command(setup + f"source {source}; source {source}")
            start = len(session.transcript)
            success = "print '__ZTTY_SUCCESS__'" if name == "nu" else "printf '__ZTTY_SUCCESS__\\n'"
            segment = session.command(success)
            assert len(re.findall(rb"\x1b\]133;C(?:;[^\x07\x1b]*)?(?:\x07|\x1b\\)", segment)) == 1, "duplicate or absent command-start marker"
            session.command(("^" if name == "nu" else "") + "/bin/sh -c 'exit 7'")
            session.command("print '__ZTTY_UTF8_界__'" if name == "nu" else "printf '__ZTTY_UTF8_界__\\n'")
            session.command("cd " + shlex.quote(str(destination)))
            if name == "bash":
                trap = session.command("trap -p DEBUG")
                assert b"ztty-existing-trap" in trap, "existing DEBUG trap was replaced"
                session.command(shlex.quote(binary) + " --noprofile --rcfile " + source + " -i")
                nested = session.command("printf '__ZTTY_NESTED__\\n'")
                assert len(re.findall(rb"\x1b\]133;C(?:\x07|\x1b\\)", nested)) == 1, "nested shell duplicated execution hooks"
                session.command("exit 0")
            transcript = bytes(session.transcript)
            assert b"__ZTTY_EXISTING_HOOK__" in transcript[start:], "existing shell hook did not run"
            assert b"ZT> " in transcript[start:], "custom prompt was replaced"
            uris = re.findall(rb"\x1b\]7;([^\x07\x1b]+)(?:\x07|\x1b\\)", transcript)
            assert any(unquote(urlsplit(uri.decode()).path) == str(destination) for uri in uris), "OSC 7 directory encoding mismatch"
            parsed = session.finish_model().decode()
            records = []
            for line in parsed.splitlines():
                fields = line.split("\t")
                if fields[0] == "command":
                    records.append({"id": int(fields[1]), "exit_status": None if fields[2] == "-" else int(fields[2]), "complete": fields[3] == "true", "truncated": fields[4] == "true", "output": bytes.fromhex(fields[5]).decode()})
            successful = [record for record in records if "__ZTTY_SUCCESS__" in record["output"]]
            assert len(successful) == 1 and successful[0]["exit_status"] == 0, parsed
            assert successful[0]["complete"] and not successful[0]["truncated"], parsed
            if name in ("bash", "zsh"):
                assert "__ZTTY_EXISTING_HOOK__" not in successful[0]["output"], "prompt hook leaked into command output"
            assert any(record["exit_status"] == 7 for record in records), "exit status 7 was lost: " + parsed
            assert any("__ZTTY_UTF8_界__" in record["output"] and not record["truncated"] for record in records), parsed
            if name == "bash":
                assert any("__ZTTY_NESTED__" in record["output"] and record["exit_status"] == 0 for record in records), parsed
            artifact_dir.mkdir(parents=True, exist_ok=True)
            (artifact_dir / (name + ".pty")).write_bytes(transcript)
            return {"shell": name, "status": "passed", "version": subprocess.run([binary, "--version"], capture_output=True, text=True, check=True).stdout.splitlines()[0], "records": len(records), "transcript_bytes": len(transcript), "transcript_sha256": hashlib.sha256(transcript).hexdigest(), "checks": ["real interactive PTY", "source twice without duplicate execution marks", "existing hooks and prompt preserved", "successful output range", "failure exit status 7", "UTF-8 output", "OSC 7 URI for Unicode and spaces"] + (["bash -n syntax gate", "first and repeated source each exit zero"] if name == "bash" else [])}
        finally:
            artifact_dir.mkdir(parents=True, exist_ok=True)
            (artifact_dir / (name + ".pty")).write_bytes(session.transcript)
            session.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--shell", action="append", choices=["bash", "zsh", "fish", "nu"])
    parser.add_argument("--probe", type=Path, default=Path("target/debug/examples/shell_probe"))
    parser.add_argument("--artifacts", type=Path, default=Path("artifacts/shell-integration"))
    parser.add_argument("--report", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    protocol = subprocess.run([str(args.probe.resolve()), "--protocol"], input=b"", capture_output=True, timeout=3, check=True).stdout
    if protocol.strip() != b"zterm-shell-probe-v1":
        parser.error("build the current shell_probe example before running this test")
    sources = [*sorted((root / "crates/zterm/src").glob("*.rs")), root / "crates/zterm/examples/shell_probe.rs", *sorted((root / "shell").glob("ztty.*")), Path(__file__).resolve()]
    result = {"schema": "ztty.shell-integration.v1", "platform": os.uname().sysname, "source_sha256": {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sources}, "checks": []}
    for name in args.shell or ["bash", "zsh", "fish", "nu"]:
        try:
            item = run_shell(name, root, args.probe.resolve(), args.artifacts.resolve())
        except Exception as error:
            item = {"shell": name, "status": "failed", "error": str(error)}
        result["checks"].append(item)
        print(json.dumps(item, ensure_ascii=False), flush=True)
    if args.report:
        args.report.parent.mkdir(parents=True, exist_ok=True)
        args.report.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n")
    raise SystemExit(0 if all(item["status"] == "passed" for item in result["checks"]) else 1)


if __name__ == "__main__":
    main()
