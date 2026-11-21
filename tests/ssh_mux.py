#!/usr/bin/env python3
"""Exercise persistent terminal multiplexing over an owned localhost SSH server.

The server, keys, known_hosts, control sockets and broker namespace are private
to this run. The test never connects to a configured remote host or modifies an
SSH configuration. A missing sshd is a failed prerequisite, not a skipped pass.
"""
import argparse
import contextlib
import errno
import fcntl
import hashlib
import json
import os
from pathlib import Path
import pty
import pwd
import re
import select
import shlex
import shutil
import signal
import socket
import stat
import struct
import subprocess
import sys
import tempfile
import termios
import time


TIMEOUT = 12.0
TAIL_LIMIT = 256 * 1024


class RpcError(RuntimeError):
    pass


def require(condition, message):
    if not condition:
        raise AssertionError(message)


def run(command, *, env=None, timeout=TIMEOUT, check=True):
    result = subprocess.run(command, env=env, stdin=subprocess.DEVNULL,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            timeout=timeout)
    if check and result.returncode:
        raise AssertionError({"command": [str(value) for value in command],
                              "exit_code": result.returncode,
                              "stderr": result.stderr.decode(errors="replace")[-4096:],
                              "stdout": result.stdout.decode(errors="replace")[-4096:]})
    return result


class ProcessHandle:
    """A pidfd binds cleanup to the process observed during this test."""

    def __init__(self, pid):
        self.pid = pid
        self.fd = os.pidfd_open(pid)

    def alive(self):
        return not select.select([self.fd], [], [], 0)[0]

    def stop(self, sig=signal.SIGTERM):
        if self.alive():
            with contextlib.suppress(ProcessLookupError):
                signal.pidfd_send_signal(self.fd, sig)

    def wait(self, seconds):
        return bool(select.select([self.fd], [], [], seconds)[0])

    def close(self):
        os.close(self.fd)


class TerminalClient:
    """A real controlling terminal, including SIGWINCH and SSH raw mode."""

    def __init__(self, command, env, cols=80, rows=24):
        self.tail = bytearray()
        self.bytes_received = 0
        self.status = None
        self.closed = False
        self.started_at = time.monotonic()
        pid, master = pty.fork()
        if pid == 0:
            try:
                fcntl.ioctl(0, termios.TIOCSWINSZ, struct.pack("HHHH", rows, cols, 0, 0))
                os.execvpe(str(command[0]), [str(value) for value in command], env)
            except BaseException as error:
                os.write(2, ("test child exec failed: " + str(error) + "\n").encode())
                os._exit(127)
        self.pid, self.master = pid, master
        os.set_blocking(master, False)

    def poll(self):
        if self.status is None:
            child, status = os.waitpid(self.pid, os.WNOHANG)
            if child:
                self.status = os.waitstatus_to_exitcode(status)
        return self.status

    def pump(self, timeout=0):
        if self.closed:
            return
        if not select.select([self.master], [], [], timeout)[0]:
            return
        while True:
            try:
                data = os.read(self.master, 65536)
            except BlockingIOError:
                break
            except OSError as error:
                if error.errno == errno.EIO:
                    break
                raise
            if not data:
                break
            self.bytes_received += len(data)
            self.tail.extend(data)
            del self.tail[:-TAIL_LIMIT]

    def diagnostic(self):
        self.pump()
        return {"pid": self.pid, "exit_code": self.poll(),
                "tail": bytes(self.tail[-4096:]).decode(errors="replace")}

    def send(self, data):
        data = data.encode() if isinstance(data, str) else data
        deadline = time.monotonic() + TIMEOUT
        while data:
            require(self.poll() is None, self.diagnostic())
            try:
                written = os.write(self.master, data)
                data = data[written:]
            except BlockingIOError:
                require(time.monotonic() < deadline, "terminal input remained blocked")
                self.pump(.01)

    def resize(self, cols, rows):
        fcntl.ioctl(self.master, termios.TIOCSWINSZ,
                    struct.pack("HHHH", rows, cols, 0, 0))

    def wait(self, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while self.poll() is None:
            require(time.monotonic() < deadline, self.diagnostic())
            self.pump(.02)
        self.pump()
        return self.status

    def stop(self):
        if self.poll() is None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(self.pid, signal.SIGTERM)
            try:
                self.wait(3)
            except AssertionError:
                with contextlib.suppress(ProcessLookupError):
                    os.killpg(self.pid, signal.SIGKILL)
                self.wait(3)
        if not self.closed:
            os.close(self.master)
            self.closed = True


class Fixture:
    def __init__(self, root, binary, report):
        self.root, self.binary, self.report = root, binary, report
        self.runtime = root / "runtime"
        self.runtime.mkdir(mode=0o700)
        self.socket_name = "@" + root.name
        self.env = dict(os.environ, ZTTY_SOCKET=self.socket_name,
                        XDG_RUNTIME_DIR=str(self.runtime),
                        XDG_CONFIG_HOME=str(root / "config"), LC_ALL="C.UTF-8", PS1="",
                        SHELL="/bin/sh")
        for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "LD_PRELOAD"):
            self.env.pop(key, None)
        self.clients = []
        self.handles = {}
        self.sshd = None
        self.sshd_log = None
        self.ssh_config = root / "ssh_config"
        self.remote_binary = root / "remote ztty"

    def own(self, pid):
        if pid not in self.handles:
            self.handles[pid] = ProcessHandle(pid)
        return self.handles[pid]

    def pump(self):
        for client in self.clients:
            client.pump()

    def wait(self, predicate, description, timeout=TIMEOUT):
        deadline = time.monotonic() + timeout
        while True:
            self.pump()
            value = predicate()
            if value:
                return value
            if time.monotonic() >= deadline:
                raise AssertionError({"timeout": description,
                                      "clients": [client.diagnostic() for client in self.clients]})
            time.sleep(.01)

    def rpc(self, request):
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
            channel.settimeout(5)
            channel.connect("\0" + self.socket_name[1:])
            peer, uid, _ = struct.unpack("3i", channel.getsockopt(
                socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
            require(uid == os.getuid(), "fixture broker has another owner")
            self.own(peer)
            channel.sendall(json.dumps(request).encode() + b"\n")
            data = bytearray()
            while b"\n" not in data:
                block = channel.recv(65536)
                require(block, "fixture broker disconnected before its reply")
                data.extend(block)
                require(len(data) <= 2 * 1024 * 1024, "fixture broker reply exceeded its bound")
        envelope = json.loads(data.split(b"\n", 1)[0])
        if not envelope.get("ok"):
            raise RpcError(envelope.get("error", repr(envelope)))
        value = envelope["result"]
        if "broker_pid" in value:
            require(value["broker_pid"] == peer, "broker metadata differs from peer credentials")
        if isinstance(value.get("pid"), int) and value["pid"] > 0:
            with contextlib.suppress(ProcessLookupError):
                self.own(value["pid"])
        return value

    def info(self, name):
        try:
            return self.rpc({"op": "info", "session": name})
        except (ConnectionRefusedError, FileNotFoundError):
            return None
        except RpcError as error:
            if "session not found" in str(error):
                return None
            raise

    def output(self, name):
        return self.rpc({"op": "read", "session": name})

    def start(self, name, *, ssh=False, observe=False, takeover=False, command=None,
              cols=80, rows=24):
        if ssh:
            argv = [self.binary, "ssh", "--terminal", "-F", self.ssh_config,
                    "--remote-bin", self.remote_binary]
            if observe:
                argv.append("--observe")
            if takeover:
                argv.append("--takeover")
            argv.extend(["ztty-fixture", name])
        else:
            argv = [self.binary, "mux", "attach"]
            if observe:
                argv.append("--observe")
            if takeover:
                argv.append("--takeover")
            argv.append(name)
        if command:
            argv.extend(["--", *command])
        client = TerminalClient(argv, self.env, cols, rows)
        self.clients.append(client)
        return client

    def cli(self, action, name=None, ssh=False):
        if ssh:
            command = [self.binary, "ssh", "--" + action, "-F", self.ssh_config,
                       "--remote-bin", self.remote_binary, "ztty-fixture"]
        else:
            command = [self.binary, "mux", action]
        if name:
            command.append(name)
        result = run(command, env=self.env)
        return json.loads(result.stdout)

    def file(self, name):
        return self.root / name

    def written(self, name, expected=None):
        path = self.file(name)
        if not path.exists():
            return False
        value = path.read_bytes()
        return value if expected is None else value == expected

    def prime_shell(self, client, label):
        destination = shlex.quote(str(self.file(label + "-ready")))
        client.send("stty -echo; printf '%s' ready > " + destination + "\n")
        self.wait(lambda: self.written(label + "-ready", b"ready"), label + " shell ready")

    def start_sshd(self):
        sshd = shutil.which("sshd")
        if sshd is None and Path("/usr/sbin/sshd").is_file():
            sshd = "/usr/sbin/sshd"
        require(sshd and shutil.which("ssh") and shutil.which("ssh-keygen"),
                "real SSH acceptance requires ssh, ssh-keygen and sshd")
        for stem in ("host", "identity"):
            run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", self.root / stem])
        identity = (self.root / "identity.pub").read_text()
        (self.root / "authorized_keys").write_text(identity)
        host_type, host_key, *_ = (self.root / "host.pub").read_text().split()
        (self.root / "known_hosts").write_text(f"ztty-fixture-host {host_type} {host_key}\n")
        with socket.socket() as listener:
            listener.bind(("127.0.0.1", 0))
            port = listener.getsockname()[1]
        user = pwd.getpwuid(os.getuid()).pw_name
        config = self.root / "sshd_config"
        config.write_text("\n".join([
            f"Port {port}", "ListenAddress 127.0.0.1", f"HostKey {self.root / 'host'}",
            f"PidFile {self.root / 'sshd.pid'}", f"AuthorizedKeysFile {self.root / 'authorized_keys'}",
            "StrictModes no", "PasswordAuthentication no", "KbdInteractiveAuthentication no",
            "PubkeyAuthentication yes", "AuthenticationMethods publickey", "UsePAM no",
            "PermitRootLogin yes", f"AllowUsers {user}", "PrintMotd no", "PrintLastLog no",
            "ClientAliveInterval 1", "ClientAliveCountMax 2", "LogLevel VERBOSE", "",
        ]))
        self.ssh_config.write_text("\n".join([
            "Host ztty-fixture", "    HostName 127.0.0.1", f"    Port {port}",
            f"    User {user}", f"    IdentityFile {self.root / 'identity'}",
            "    IdentitiesOnly yes", "    BatchMode yes", "    ConnectTimeout 5",
            "    StrictHostKeyChecking yes", "    HostKeyAlias ztty-fixture-host",
            f"    UserKnownHostsFile {self.root / 'known_hosts'}",
            "    GlobalKnownHostsFile /dev/null", "",
        ]))
        exports = {"ZTTY_SOCKET": self.socket_name, "XDG_RUNTIME_DIR": str(self.runtime),
                   "XDG_CONFIG_HOME": str(self.root / "config"), "LC_ALL": "C.UTF-8", "PS1": "",
                   "SHELL": "/bin/sh"}
        self.remote_binary.write_text("#!/bin/sh\n" + "\n".join(
            "export " + key + "=" + shlex.quote(value) for key, value in exports.items()) +
            "\nunset DISPLAY WAYLAND_DISPLAY WAYLAND_SOCKET\nexec " +
            shlex.quote(str(self.binary)) + ' "$@"\n')
        self.remote_binary.chmod(0o700)
        run([sshd, "-t", "-f", config])
        self.sshd_log = (self.root / "sshd.log").open("wb")
        self.sshd = subprocess.Popen([sshd, "-D", "-e", "-f", str(config)],
                                     stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                                     stderr=self.sshd_log, start_new_session=True)
        self.own(self.sshd.pid)

        def ready():
            if self.sshd.poll() is not None:
                raise AssertionError((self.root / "sshd.log").read_text()[-4096:])
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=.2):
                    return True
            except OSError:
                return False

        self.wait(ready, "private sshd startup")

    def masters(self):
        found = []
        for path in self.runtime.rglob("*"):
            if not stat.S_ISSOCK(path.lstat().st_mode):
                continue
            checked = run(["ssh", "-F", self.ssh_config, "-S", path, "-O", "check",
                           "ztty-fixture"], env=self.env, check=False)
            if checked.returncode == 0:
                match = re.search(rb"pid=(\d+)", checked.stderr)
                require(match is not None, "SSH master check did not identify its process")
                self.own(int(match.group(1)))
                found.append(path)
        return found

    def disconnect_master(self, path):
        return run(["ssh", "-F", self.ssh_config, "-S", path, "-O", "exit",
                    "ztty-fixture"], env=self.env, check=False)

    def cleanup(self):
        errors = []
        for client in self.clients:
            try:
                client.stop()
            except BaseException as error:
                errors.append("terminal cleanup: " + repr(error))
        try:
            listed = self.rpc({"op": "list"})
            for session in listed["sessions"]:
                request = {"session": session["uri"], "_actor": "ssh-mux-test-cleanup"}
                self.rpc({"op": "control", "action": "acquire", "takeover": True, **request})
                self.rpc({"op": "close", **request})
        except (ConnectionRefusedError, FileNotFoundError):
            pass
        except BaseException as error:
            errors.append("session cleanup: " + repr(error))
        if self.ssh_config.exists():
            try:
                for path in self.masters():
                    self.disconnect_master(path)
            except BaseException as error:
                errors.append("SSH master cleanup: " + repr(error))
        if self.sshd is not None:
            # Record only descendants of our private sshd before stopping it.
            parents = {}
            for entry in Path("/proc").iterdir():
                if not entry.name.isdecimal():
                    continue
                try:
                    fields = (entry / "stat").read_text().rsplit(")", 1)[1].split()
                    parents[int(entry.name)] = int(fields[1])
                except (OSError, ValueError, IndexError):
                    continue
            owned = {self.sshd.pid}
            while True:
                added = {pid for pid, parent in parents.items() if parent in owned} - owned
                if not added:
                    break
                owned.update(added)
            for pid in owned:
                with contextlib.suppress(ProcessLookupError):
                    self.own(pid)
        for handle in self.handles.values():
            handle.stop()
        deadline = time.monotonic() + 3
        while time.monotonic() < deadline and any(handle.alive() for handle in self.handles.values()):
            time.sleep(.02)
        for handle in self.handles.values():
            handle.stop(signal.SIGKILL)
        remaining = []
        for handle in self.handles.values():
            if not handle.wait(3):
                remaining.append(handle.pid)
            handle.close()
        if self.sshd is not None:
            self.sshd.wait(timeout=3)
        if self.sshd_log is not None:
            self.sshd_log.close()
        self.report["cleanup_errors"] = errors
        self.report["remaining_owned_pids"] = remaining


@contextlib.contextmanager
def check(report, name):
    started = time.monotonic()
    row = {"name": name, "ok": False}
    report["checks"].append(row)
    try:
        yield row
        row["ok"] = True
    finally:
        row["elapsed_ms"] = round((time.monotonic() - started) * 1000, 3)


def same_session(before, after):
    require(after is not None, "persistent session disappeared")
    for key in ("id", "created_at", "uri", "pid", "broker_pid"):
        require(before[key] == after[key], f"persistent session changed {key}")
    require(after["running"], "persistent PTY stopped with its client")


def process_cpu_ticks(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def read_envelope(channel):
    data = bytearray()
    while b"\n" not in data:
        block = channel.recv(65536)
        require(block, "watch connection closed before its response")
        data.extend(block)
        require(len(data) <= 1024 * 1024, "watch response exceeded its bound")
    value = json.loads(data.split(b"\n", 1)[0])
    require(value.get("ok") is True, value)
    return value["result"]


def local_checks(fixture, report):
    with check(report, "local_mux_create_and_real_pty_input") as row:
        first = fixture.start("local", command=["/bin/sh", "-i"])
        initial = fixture.wait(lambda: fixture.info("local"), "local persistent shell")
        fixture.prime_shell(first, "local")
        row.update(pty_pid=initial["pid"], broker_pid=initial["broker_pid"])

    with check(report, "writer_exclusion_and_read_only_observer"):
        before = fixture.info("local")
        rejected = fixture.start("local")
        require(rejected.wait() != 0, "second input owner was accepted without takeover")
        require(any(word in bytes(rejected.tail).lower() for word in (b"control", b"owner")),
                rejected.diagnostic())
        observer = fixture.start("local", observe=True, cols=47, rows=13)
        fixture.wait(lambda: observer.bytes_received > 0, "observer initial frame")
        observer.send("printf observer > " + shlex.quote(str(fixture.file("observer-write"))) + "\n")
        first.send("printf owner > " + shlex.quote(str(fixture.file("owner-write"))) + "\n")
        fixture.wait(lambda: fixture.written("owner-write", b"owner"), "owner input during observation")
        require(not fixture.file("observer-write").exists(), "observer sent PTY input")
        current = fixture.info("local")
        require(current["input_owner"] == before["input_owner"], "observer acquired input control")
        require((current["cols"], current["rows"]) == (before["cols"], before["rows"]),
                "observer resized the writer's terminal")
        observer.send(b"\x1d")
        require(observer.wait() == 0, observer.diagnostic())

    with check(report, "explicit_takeover_revokes_previous_writer"):
        previous = fixture.info("local")["input_owner"]
        active = fixture.start("local", takeover=True)
        fixture.wait(lambda: fixture.info("local")["input_owner"] != previous, "explicit takeover")
        with contextlib.suppress(OSError, AssertionError):
            first.send("printf stale > " + shlex.quote(str(fixture.file("stale-write"))) + "\n")
        active.send("printf active > " + shlex.quote(str(fixture.file("active-write"))) + "\n")
        fixture.wait(lambda: fixture.written("active-write", b"active"), "new writer input")
        require(not fixture.file("stale-write").exists(), "revoked writer sent input")
        first.stop()

    with check(report, "resize_reaches_remote_pty"):
        active.resize(93, 31)
        fixture.wait(lambda: (fixture.info("local")["cols"], fixture.info("local")["rows"]) == (93, 31),
                     "TIOCSWINSZ propagation")
        active.send("stty size > " + shlex.quote(str(fixture.file("geometry"))) + "\n")
        fixture.wait(lambda: fixture.written("geometry", b"31 93\n"), "actual child PTY size")

    with check(report, "detach_preserves_session_and_explicit_close_ends_it"):
        active.send(b"\x1d")
        require(active.wait() == 0, active.diagnostic())
        current = fixture.info("local")
        same_session(initial, current)
        require(current["input_owner"] is None, "detached client retained input control")
        require(any(item["name"] == "local" for item in fixture.cli("list")["sessions"]),
                "CLI list omitted detached shell")
        fixture.cli("close", "local")
        fixture.wait(lambda: fixture.info("local") is None, "explicit local session close")
        require(fixture.handles[initial["pid"]].wait(5), "explicit close left its PTY child alive")


def ssh_checks(fixture, report):
    with check(report, "real_ssh_two_sessions_share_one_connection") as row:
        starts = fixture.file("alpha-starts")
        command = ["/bin/sh", "-c", "printf x >> " + shlex.quote(str(starts)) + "; exec /bin/sh -i"]
        alpha = fixture.start("alpha", ssh=True, command=command)
        alpha_info = fixture.wait(lambda: fixture.info("alpha"), "first SSH terminal")
        first_ms = (time.monotonic() - alpha.started_at) * 1000
        fixture.prime_shell(alpha, "alpha")
        beta = fixture.start("beta", ssh=True, command=["/bin/sh", "-i"])
        beta_info = fixture.wait(lambda: fixture.info("beta"), "second SSH terminal")
        reused_ms = (time.monotonic() - beta.started_at) * 1000
        fixture.prime_shell(beta, "beta")
        require(alpha_info["pid"] != beta_info["pid"], "two sessions used one PTY")
        require(alpha_info["broker_pid"] == beta_info["broker_pid"], "remote sessions did not share a broker")
        alpha.send("ZTTY_TEST_VALUE=ALPHA; printf ALPHA > " + shlex.quote(str(fixture.file("alpha-value"))) + "\n")
        beta.send("ZTTY_TEST_VALUE=BETA; printf BETA > " + shlex.quote(str(fixture.file("beta-value"))) + "\n")
        fixture.wait(lambda: fixture.written("alpha-value", b"ALPHA") and fixture.written("beta-value", b"BETA"),
                     "independent SSH input")
        listed = fixture.cli("list", ssh=True)
        require({item["name"] for item in listed["sessions"]} == {"alpha", "beta"}, listed)
        masters = fixture.masters()
        require(len(masters) == 1, "expected one SSH ControlMaster for two sessions")
        authenticated = (fixture.root / "sshd.log").read_text().count("Accepted publickey for ")
        require(authenticated == 1, f"expected one SSH authentication, observed {authenticated}")
        row.update(first_connect_ms=round(first_ms, 3), reused_connect_ms=round(reused_ms, 3),
                   ssh_authentications=authenticated, control_masters=len(masters),
                   distinct_pty_pids=[alpha_info["pid"], beta_info["pid"]])

    with check(report, "transport_loss_keeps_both_remote_shells_running"):
        alpha.send("(sleep 0.4; printf '\\nSSH_BACKGROUND_SURVIVED\\n'; printf done > " +
                   shlex.quote(str(fixture.file("background-done"))) + ") &\nprintf scheduled > " +
                   shlex.quote(str(fixture.file("background-scheduled"))) + "\n")
        fixture.wait(lambda: fixture.written("background-scheduled", b"scheduled"), "background job scheduled")
        require(fixture.disconnect_master(masters[0]).returncode == 0, "could not disconnect owned SSH master")
        alpha.wait()
        beta.wait()
        fixture.wait(lambda: fixture.written("background-done", b"done"), "work while SSH disconnected")
        same_session(alpha_info, fixture.info("alpha"))
        same_session(beta_info, fixture.info("beta"))
        require("SSH_BACKGROUND_SURVIVED" in fixture.output("alpha")["screen"],
                "remote screen lost output produced after disconnect")

    with check(report, "reconnect_restores_screen_and_existing_process_without_reexecution") as row:
        alpha_again = fixture.start("alpha", ssh=True, command=command)
        fixture.wait(lambda: b"SSH_BACKGROUND_SURVIVED" in alpha_again.tail, "reconnected terminal snapshot")
        reattach_ms = (time.monotonic() - alpha_again.started_at) * 1000
        snapshot_bytes = alpha_again.bytes_received
        beta_again = fixture.start("beta", ssh=True)
        fixture.wait(lambda: beta_again.bytes_received > 0, "second reconnect")
        same_session(alpha_info, fixture.info("alpha"))
        same_session(beta_info, fixture.info("beta"))
        require(starts.read_bytes() == b"x", "reattach executed the creation command again")
        alpha_again.send("printf '%s' \"$ZTTY_TEST_VALUE\" > " + shlex.quote(str(fixture.file("alpha-restored"))) + "\n")
        beta_again.send("printf '%s' \"$ZTTY_TEST_VALUE\" > " + shlex.quote(str(fixture.file("beta-restored"))) + "\n")
        fixture.wait(lambda: fixture.written("alpha-restored", b"ALPHA") and fixture.written("beta-restored", b"BETA"),
                     "shell state after SSH reconnect")
        row.update(reattach_ms=round(reattach_ms, 3), snapshot_bytes=snapshot_bytes)

    with check(report, "ssh_resize_reaches_same_child_pty"):
        alpha_again.resize(101, 29)
        fixture.wait(lambda: (fixture.info("alpha")["cols"], fixture.info("alpha")["rows"]) == (101, 29),
                     "SSH window-change propagation")
        alpha_again.send("stty size > " + shlex.quote(str(fixture.file("ssh-geometry"))) + "\n")
        fixture.wait(lambda: fixture.written("ssh-geometry", b"29 101\n"), "remote stty size")

    with check(report, "remote_argv_and_binary_path_preserve_literal_shell_characters"):
        injected = fixture.file("injected")
        quoted_file = fixture.file("quoted-arguments.json")
        arguments = ["literal space", "apostrophe ' quote", "$(printf injected > " + str(injected) + ")", "$HOME;*[]"]
        source = ("import json,os,sys;json.dump(sys.argv[2:],open(sys.argv[1],'w'));"
                  "print('QUOTED_ARGUMENTS_READY',flush=True);os.read(0,1)")
        quoted = fixture.start("quoted", ssh=True,
                               command=[sys.executable, "-c", source, str(quoted_file), *arguments])
        fixture.wait(lambda: quoted_file.exists() and b"QUOTED_ARGUMENTS_READY" in quoted.tail,
                     "quoted remote command")
        require(json.loads(quoted_file.read_text()) == arguments, "SSH changed literal command arguments")
        require(not injected.exists(), "a literal command argument executed shell syntax")

    with check(report, "ssh_close_terminates_only_the_selected_session"):
        quoted_info = fixture.info("quoted")
        fixture.cli("close", "quoted", ssh=True)
        fixture.wait(lambda: fixture.info("quoted") is None, "quoted session close")
        require(fixture.handles[quoted_info["pid"]].wait(5), "closed remote process remains alive")
        same_session(alpha_info, fixture.info("alpha"))
        same_session(beta_info, fixture.info("beta"))
        for client, name in ((alpha_again, "alpha"), (beta_again, "beta")):
            client.send(b"\x1d")
            require(client.wait() == 0, client.diagnostic())
            fixture.cli("close", name, ssh=True)
        require(fixture.cli("list", ssh=True)["sessions"] == [], "closed remote sessions remain listed")


def completed_command_checks(fixture, report):
    for remote in (False, True):
        transport = "ssh" if remote else "local"
        name = "completed-" + transport
        marker = ("FINAL_SCREEN_42_" + transport.upper()).encode()
        starts = fixture.file(name + "-starts")
        # Closing all slave descriptors produces EOF before waitpid can report
        # the final status. The client must drain the frame, then return 42.
        source = ("import os,sys,time;open(sys.argv[1],'ab',buffering=0).write(b'x');"
                  "os.write(1,bytes.fromhex(sys.argv[2]));"
                  "os.close(0);os.close(1);os.close(2);time.sleep(.15);os._exit(42)")
        command = [sys.executable, "-c", source, str(starts), marker.hex()]
        with check(report, transport + "_eof_before_exit_preserves_frame_and_status") as row:
            client = fixture.start(name, ssh=remote, command=command)
            before = fixture.wait(lambda: fixture.info(name), name + " creation")
            require(client.wait() == 42, client.diagnostic())
            require(marker in client.tail, "final terminal frame was lost before command exit")
            current = fixture.info(name)
            require(current is not None and not current["running"], "exited session was not retained")
            require(current["output_eof"] is True and current["exit_code"] == 42, current)
            require(marker.decode() in fixture.output(name)["screen"], "retained screen lost final output")
            listed = [item for item in fixture.cli("list", ssh=remote)["sessions"] if item["name"] == name]
            require(len(listed) == 1 and listed[0]["exit_code"] == 42 and not listed[0]["running"], listed)
            resumed = fixture.start(name, ssh=remote, command=command)
            require(resumed.wait() == 42, resumed.diagnostic())
            require(marker in resumed.tail, "reattach did not render the completed command's screen")
            after = fixture.info(name)
            for key in ("id", "created_at", "uri", "pid", "broker_pid"):
                require(before[key] == after[key], f"completed session changed {key} on reattach")
            require(starts.read_bytes() == b"x", "reattach reran the completed command")
            fixture.cli("close", name, ssh=remote)
            fixture.wait(lambda: fixture.info(name) is None, name + " explicit close")
            row.update(exit_code=42, retained_pty_pid=before["pid"], command_executions=1)


def binary_input_checks(fixture, report):
    # Exclude the documented detach byte (Ctrl+]); the payload deliberately
    # includes NUL, invalid UTF-8 and a UTF-8 suffix split across writes.
    payload = (b"\x00\xff\xfe\xc0\xaf\xf0\x9f\x92RAW\x80\xe2\x82" * 257) + b"\xac\x00END"
    for remote in (False, True):
        transport = "ssh" if remote else "local"
        name = "binary-" + transport
        destination = fixture.file(name + "-input")
        source = ("import os,sys,tty;tty.setraw(0);print('RAW_INPUT_READY',flush=True);"
                  "data=bytearray();limit=int(sys.argv[2]);"
                  "exec('while len(data)<limit:\\n data.extend(os.read(0,limit-len(data)))');"
                  "open(sys.argv[1],'wb').write(data);"
                  "print('\\r\\nRAW_BYTES_REACHED_CHILD\\r\\n',flush=True);os.read(0,1)")
        with check(report, transport + "_binary_input_and_event_driven_output") as row:
            client = fixture.start(name, ssh=remote,
                                   command=[sys.executable, "-c", source, str(destination), str(len(payload))])
            fixture.wait(lambda: b"RAW_INPUT_READY" in client.tail, name + " raw input readiness")
            info = fixture.info(name)
            observer = fixture.start(name, ssh=remote, observe=True)
            fixture.wait(lambda: b"RAW_INPUT_READY" in observer.tail, name + " observer snapshot")
            frame = fixture.rpc({"op": "terminal-frame", "session": name})
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as watch:
                watch.settimeout(3)
                watch.connect("\0" + fixture.socket_name[1:])
                watch.sendall(json.dumps({"op": "watch", "session": name,
                                           "revision": frame["revision"],
                                           "control_epoch": frame["control_epoch"],
                                           "wait_ms": 5000}).encode() + b"\n")
                owner_pid = int(info["input_owner"].split(":")[1])
                tracked = sorted({owner_pid, info["broker_pid"], client.pid, observer.pid})
                ticks_before = {pid: process_cpu_ticks(pid) for pid in tracked}
                idle_start = time.monotonic()
                require(not select.select([watch], [], [], .35)[0],
                        "unchanged terminal watch returned instead of parking")
                idle_seconds = time.monotonic() - idle_start
                tick_delta = {str(pid): process_cpu_ticks(pid) - ticks_before[pid] for pid in tracked}
                cpu_seconds = sum(tick_delta.values()) / os.sysconf("SC_CLK_TCK")
                require(cpu_seconds < max(.10, idle_seconds * .35),
                        {"idle_cpu_seconds": cpu_seconds, "elapsed_seconds": idle_seconds,
                         "cpu_ticks": tick_delta})
                observer.send(b"\x00\xff" + b"O" * (len(payload) - 2))
                require(not select.select([watch], [], [], .15)[0],
                        "read-only observer input changed the terminal")
                require(not destination.exists(), "observer completed the raw child's input")
                input_start = time.monotonic()
                client.send(payload[:-5])
                client.send(payload[-5:])
                require(select.select([watch], [], [], 2)[0], "new PTY output did not wake parked watch")
                awakened = read_envelope(watch)
                watch_latency_ms = (time.monotonic() - input_start) * 1000
                require(awakened["revision"] != frame["revision"], awakened)
                fixture.wait(lambda: destination.exists() and b"RAW_BYTES_REACHED_CHILD" in client.tail,
                             name + " binary payload and rendered result", timeout=2)
                rendered_latency_ms = (time.monotonic() - input_start) * 1000
            require(destination.read_bytes() == payload,
                    "PTY input changed NUL/invalid UTF-8 bytes or accepted observer input")
            for attached in (observer, client):
                attached.send(b"\x1d")
                require(attached.wait() == 0, attached.diagnostic())
            fixture.cli("close", name, ssh=remote)
            fixture.wait(lambda: fixture.info(name) is None, name + " close")
            row.update(payload_bytes=len(payload), idle_seconds=round(idle_seconds, 3),
                       idle_cpu_seconds=round(cpu_seconds, 6), idle_cpu_ticks=tick_delta,
                       watch_latency_ms=round(watch_latency_ms, 3),
                       rendered_latency_ms=round(rendered_latency_ms, 3))


def backpressure_checks(fixture, report):
    # The final partial input block and its detach byte share one stream. More
    # than half a MiB cannot fit in an unread PTY, so this exercises both the
    # bounded broker input queue and drain-before-release behavior.
    length = 512 * 1024 + 137
    pattern = b"backpressure-0123456789."
    payload = (pattern * (length // len(pattern) + 1))[:length]
    for remote in (False, True):
        transport = "ssh" if remote else "local"
        name = "backpressure-" + transport
        destination = fixture.file(name + "-input")
        source = ("import os,sys,time,tty;tty.setraw(0);"
                  "print('BACKPRESSURE_READY',flush=True);time.sleep(.5);"
                  "data=bytearray();limit=int(sys.argv[2]);"
                  "exec('while len(data)<limit:\\n part=os.read(0,limit-len(data))"
                  "\\n if not part: sys.exit(99)\\n data.extend(part)');"
                  "open(sys.argv[1],'wb').write(data);"
                  "print('\\r\\nBACKPRESSURE_COMPLETE\\r\\n',flush=True);os.read(0,1)")
        with check(report, transport + "_paste_then_detach_drains_all_queued_input") as row:
            client = fixture.start(name, ssh=remote,
                                   command=[sys.executable, "-c", source, str(destination), str(len(payload))])
            fixture.wait(lambda: b"BACKPRESSURE_READY" in client.tail, name + " delayed reader")
            before = fixture.info(name)
            started = time.monotonic()
            client.send(payload + b"\x1d")
            submitted = time.monotonic()
            require(client.wait() == 0, client.diagnostic())
            detached = time.monotonic()
            fixture.wait(lambda: destination.exists() and destination.stat().st_size == len(payload),
                         name + " complete input after detach")
            require(destination.read_bytes() == payload, "detach discarded or changed queued paste bytes")
            after = fixture.info(name)
            same_session(before, after)
            require(after["input_owner"] is None, "completed detach retained input ownership")
            fixture.cli("close", name, ssh=remote)
            fixture.wait(lambda: fixture.info(name) is None, name + " explicit close")
            row.update(payload_bytes=len(payload), reader_pause_ms=500,
                       input_submit_ms=round((submitted - started) * 1000, 3),
                       detach_ms=round((detached - started) * 1000, 3),
                       retained_pty_pid=before["pid"])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {"ok": False, "binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "checks": [], "cleanup_errors": [], "remaining_owned_pids": []}
    try:
        require(sys.platform == "linux" and hasattr(os, "pidfd_open") and hasattr(signal, "pidfd_send_signal"),
                "SSH mux acceptance requires Linux pidfd process ownership")
        with tempfile.TemporaryDirectory(prefix="ztty-ssh-mux-") as temporary:
            fixture = Fixture(Path(temporary), binary, report)
            try:
                with check(report, "private_ssh_server_and_pinned_host_key"):
                    fixture.start_sshd()
                local_checks(fixture, report)
                ssh_checks(fixture, report)
                completed_command_checks(fixture, report)
                binary_input_checks(fixture, report)
                backpressure_checks(fixture, report)
            except BaseException:
                log = fixture.root / "sshd.log"
                if log.exists():
                    # Authentication keys and complete server logs remain temporary.
                    report["sshd_diagnostic"] = log.read_text(errors="replace")[-4096:]
                raise
            finally:
                fixture.cleanup()
        report["ok"] = all(row["ok"] for row in report["checks"]) and not (
            report["cleanup_errors"] or report["remaining_owned_pids"])
    except BaseException as error:
        report["error"] = repr(error)
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps(report, ensure_ascii=False, indent=2))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
