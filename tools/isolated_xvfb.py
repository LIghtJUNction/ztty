"""Launch an owned Xvfb without auto-selecting the user's native display.

No third-party Python dependencies. The caller owns all clients it connects and
must close them before closing the returned server. Example:

    server = start_xvfb(screen="1600x1000x24", stderr=log)
    try:
        environment["DISPLAY"] = server.display
        # Start only the test's clients using that environment.
    finally:
        server.close()

Display numbers are randomly chosen from 100..30000. Filesystem locks, ordinary
and abstract Unix sockets, and an atomic per-test lease guard allocation. After
startup both Xvfb's PID lock and its actual kernel listening socket ownership
must match the created process. This module never removes an X11 lock/socket or
terminates a process that it did not start.
"""
import os
from pathlib import Path
import re
import secrets
import socket
import stat
import struct
import subprocess
import time


def display_endpoints(number):
    """Read-only evidence for one display, including abstract socket addresses."""
    number = int(number)
    base = f"/tmp/.X11-unix/X{number}"
    names = {base, "@"+base, base+"_", "@"+base+"_"}
    result = {"display":f":{number}", "files":[], "listeners":[]}
    for name in (f"/tmp/.X{number}-lock", base, base+"_"):
        try:
            value = os.lstat(name)
            item = {"path":name, "exists":True, "device":value.st_dev,
                    "inode":value.st_ino, "ctime_unix":value.st_ctime,
                    "socket":stat.S_ISSOCK(value.st_mode), "symlink":stat.S_ISLNK(value.st_mode)}
        except FileNotFoundError:
            item = {"path":name, "exists":False}
        result["files"].append(item)
    for line in Path("/proc/net/unix").read_text().splitlines()[1:]:
        fields = line.split()
        if len(fields) >= 8 and fields[7] in names and fields[3] == "00010000":
            result["listeners"].append({"path":fields[7], "inode":int(fields[6]),
                                        "state":fields[5], "type":fields[4]})
    return result


def _unavailable(number):
    evidence = display_endpoints(number)
    if any(item["exists"] for item in evidence["files"]):
        return True
    # Also reserve a name referenced by a live connected socket after unlinking
    # its filesystem entry. An absent pathname alone does not make it safe.
    names = {f"/tmp/.X11-unix/X{number}", f"@/tmp/.X11-unix/X{number}",
             f"/tmp/.X11-unix/X{number}_", f"@/tmp/.X11-unix/X{number}_"}
    return any(len(fields := line.split()) >= 8 and fields[7] in names
               for line in Path("/proc/net/unix").read_text().splitlines()[1:])


def _pid_lock(number):
    try:
        fd = os.open(f"/tmp/.X{number}-lock", os.O_RDONLY | os.O_CLOEXEC | os.O_NOFOLLOW)
    except FileNotFoundError:
        return None
    try:
        text = os.read(fd, 64).strip()
        if os.read(fd, 1):
            raise RuntimeError("oversized Xvfb PID lock")
        return int(text) if text.isdigit() else None
    finally:
        os.close(fd)


def _filesystem_peer_pid(number):
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
        channel.settimeout(1)
        channel.connect(f"/tmp/.X11-unix/X{number}")
        return struct.unpack("3i", channel.getsockopt(socket.SOL_SOCKET, socket.SO_PEERCRED, 12))[0]


def _owned_listeners(pid, number):
    inodes = set()
    try:
        entries = list(Path(f"/proc/{pid}/fd").iterdir())
    except FileNotFoundError:
        return []
    for entry in entries:
        try:
            target = os.readlink(entry)
        except FileNotFoundError:
            continue
        match = re.fullmatch(r"socket:\[([0-9]+)\]", target)
        if match:
            inodes.add(int(match[1]))
    return [item for item in display_endpoints(number)["listeners"] if item["inode"] in inodes]


def _stop(process):
    if process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=4)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=4)


def _release_lease(path, identity):
    try:
        current = path.lstat()
    except FileNotFoundError:
        return
    if (current.st_dev, current.st_ino) != identity:
        raise RuntimeError("private Xvfb lease changed ownership")
    path.unlink()


class PrivateXvfb:
    def __init__(self, process, number, lease, lease_identity, listeners):
        self.process = process
        self.display = f":{number}"
        self.number = number
        self._lease = lease
        self._lease_identity = lease_identity
        self.evidence = {"display":self.display, "pid":process.pid,
                         "pid_lock_verified":True, "owned_listeners":listeners,
                         "filesystem_peer_pid_verified":True,
                         "allocation":"guarded explicit random display 100..30000"}

    def close(self):
        if self._lease is None:
            return
        try:
            _stop(self.process)
        finally:
            _release_lease(self._lease, self._lease_identity)
            self._lease = None

    def __enter__(self):
        return self

    def __exit__(self, _kind, _value, _traceback):
        self.close()


def start_xvfb(*, screen="1600x1000x24", stderr=None, dpi=None, timeout=8.0, env=None, disable_composite=False):
    """Return a verified PrivateXvfb; fail safely if a startup/race check fails."""
    match = re.fullmatch(r"([1-9][0-9]*)x([1-9][0-9]*)x(16|24|32)", screen)
    if not match or any(int(value) > 16384 for value in match.groups()[:2]):
        raise ValueError("invalid private Xvfb screen geometry")
    if dpi is not None and not 36 <= int(dpi) <= 1000:
        raise ValueError("invalid private Xvfb DPI")
    if not 0 < timeout <= 60:
        raise ValueError("startup timeout must be within 0..60 seconds")
    native = re.search(r":([0-9]+)(?:\.[0-9]+)?$", os.environ.get("DISPLAY", ""))
    native_number = int(native[1]) if native else None
    tried = set()
    for _ in range(64):
        number = 100 + secrets.randbelow(29901)
        if number in tried or number == native_number:
            continue
        tried.add(number)
        if _unavailable(number):
            continue
        lease = Path(f"/tmp/ztty-xvfb-{number}.lease")
        try:
            fd = os.open(lease, os.O_WRONLY | os.O_CREAT | os.O_EXCL | os.O_CLOEXEC | os.O_NOFOLLOW, 0o600)
        except FileExistsError:
            continue
        try:
            identity = os.fstat(fd)
            lease_identity = (identity.st_dev, identity.st_ino)
            os.write(fd, f"{os.getpid()}\n".encode())
        finally:
            os.close(fd)
        process = None
        try:
            if _unavailable(number):
                _release_lease(lease, lease_identity)
                continue
            command = ["Xvfb",f":{number}","-screen","0",screen,"-nolisten","tcp"]
            if disable_composite:
                command.extend(["-extension", "COMPOSITE"])
            if dpi is not None:
                command.extend(["-dpi",str(int(dpi))])
            process = subprocess.Popen(command, stdout=subprocess.DEVNULL, stderr=stderr,
                                       env=env, start_new_session=True)
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                if process.poll() is not None:
                    raise RuntimeError(f"owned Xvfb {process.pid} exited with {process.returncode} on :{number}")
                lock_pid = _pid_lock(number)
                if lock_pid is not None and lock_pid != process.pid:
                    raise RuntimeError(f"Xvfb display allocation raced on :{number}; foreign PID lock {lock_pid}")
                listeners = _owned_listeners(process.pid, number)
                socket_path = Path(f"/tmp/.X11-unix/X{number}")
                if lock_pid == process.pid and listeners and socket_path.is_socket():
                    if _filesystem_peer_pid(number) != process.pid:
                        raise RuntimeError(f"Xvfb filesystem socket ownership raced on :{number}")
                    return PrivateXvfb(process, number, lease, lease_identity, listeners)
                time.sleep(0.02)
            raise TimeoutError(f"owned Xvfb :{number} did not acquire its PID lock and listening socket")
        except BaseException:
            if process is not None:
                _stop(process)
            _release_lease(lease, lease_identity)
            raise
    raise RuntimeError("could not reserve an unused private Xvfb display")
