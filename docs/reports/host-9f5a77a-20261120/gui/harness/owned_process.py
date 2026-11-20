"""Run one command in a private runtime and clean only descendant PID identities."""
import contextlib
import ctypes
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time


def enable_subreaper():
    if not hasattr(os, "pidfd_open") or not hasattr(signal, "pidfd_send_signal"):
        raise RuntimeError("PID-file-descriptor signalling is required")
    if ctypes.CDLL(None, use_errno=True).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError(ctypes.get_errno(), "PR_SET_CHILD_SUBREAPER failed")


def identity(pid):
    try:
        fields = Path(f"/proc/{pid}/stat").read_text().rpartition(")")[2].split()
        return {"pid": pid, "start_ticks": int(fields[19]), "state": fields[0]}
    except (FileNotFoundError, ProcessLookupError, PermissionError, IndexError):
        return None


def descendants():
    found = {}
    pending = [os.getpid()]
    while pending:
        parent = pending.pop()
        try:
            tasks = list(Path(f"/proc/{parent}/task").iterdir())
        except (FileNotFoundError, ProcessLookupError):
            continue
        for task in tasks:
            try:
                children = (task / "children").read_text().split()
            except (FileNotFoundError, ProcessLookupError):
                continue
            for text in children:
                pid = int(text)
                if pid in found:
                    continue
                item = identity(pid)
                if item:
                    found[pid] = item
                    pending.append(pid)
    return found


def same_process(item):
    current = identity(item["pid"])
    return current is not None and current["start_ticks"] == item["start_ticks"]


def cleanup_owned(process, observed):
    actions = []
    # Allow the harness and its private D-Bus wrapper to finish normal cleanup.
    quiet_deadline = time.monotonic() + 1.0
    while time.monotonic() < quiet_deadline:
        observed.update(descendants())
        live = [p for p in observed.values() if same_process(p)
                and (identity(p["pid"]) or {}).get("state") != "Z"]
        if not live:
            break
        time.sleep(0.05)
    for signum, grace in ((signal.SIGTERM, 3.0), (signal.SIGKILL, 3.0)):
        deadline = time.monotonic() + grace
        sent = set()
        while time.monotonic() < deadline:
            observed.update(descendants())
            live = [p for p in observed.values() if same_process(p)
                    and (identity(p["pid"]) or {}).get("state") != "Z"]
            if not live:
                break
            for item in live:
                token = (item["pid"], item["start_ticks"])
                if token in sent:
                    continue
                if same_process(item):
                    with contextlib.suppress(ProcessLookupError):
                        descriptor = os.pidfd_open(item["pid"])
                        try:
                            if same_process(item):
                                signal.pidfd_send_signal(descriptor, signum)
                                actions.append({**item, "signal": signal.Signals(signum).name})
                        finally:
                            os.close(descriptor)
                sent.add(token)
            time.sleep(0.05)
    with contextlib.suppress(subprocess.TimeoutExpired):
        process.wait(timeout=1)
    while True:
        try:
            pid, _status = os.waitpid(-1, os.WNOHANG)
        except ChildProcessError:
            break
        if pid == 0:
            break
    observed.update(descendants())
    remaining = [item for item in observed.values() if same_process(item)]
    return {"observed": list(observed.values()), "signals": actions, "remaining": remaining}


def private_environment(directory):
    env = os.environ.copy()
    removed = []
    exact = {
        "DISPLAY", "XAUTHORITY", "SWAYSOCK", "HYPRLAND_INSTANCE_SIGNATURE",
        "XDG_SESSION_ID", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP", "DESKTOP_SESSION",
        "GTK_IM_MODULE", "QT_IM_MODULE", "GDK_BACKEND", "QT_QPA_PLATFORM",
        "LD_PRELOAD", "LD_LIBRARY_PATH", "ZTTY_SOCKET",
        "ZTTY_X11_DIAGNOSTIC_CONTROL", "ZTTY_EGL_PIXEL_TRACE",
    }
    for key in list(env):
        if key in exact or key.startswith(("WAYLAND_", "DBUS_", "WLR_", "ZTTY_")):
            removed.append(key)
            env.pop(key)
    for name in ("runtime", "config", "cache", "data", "state", "tmp"):
        (directory / name).mkdir(mode=0o700)
    env.update(
        XDG_RUNTIME_DIR=str(directory / "runtime"),
        XDG_CONFIG_HOME=str(directory / "config"),
        XDG_CACHE_HOME=str(directory / "cache"),
        XDG_DATA_HOME=str(directory / "data"),
        XDG_STATE_HOME=str(directory / "state"),
        TMPDIR=str(directory / "tmp"),
        ZTTY_SOCKET="@ztty-final-" + directory.name,
        XMODIFIERS="@im=none", LC_ALL="C.UTF-8", LANG="C.UTF-8",
        LIBGL_ALWAYS_SOFTWARE="1", PYTHONUNBUFFERED="1", NO_COLOR="1",
    )
    return env, {
        "removed_session_variables": sorted(removed),
        "runtime": env["XDG_RUNTIME_DIR"], "temporary_directory": str(directory),
        "private_dbus": True, "LIBGL_ALWAYS_SOFTWARE": "1",
        "note": "Mesa software driver selection is distinct from ztty renderer selection",
    }


def run_owned(command, cwd, raw_log, timeout, stopped, heartbeat):
    receipt = {"command": ["dbus-run-session", "--", *command], "timeout_seconds": timeout}
    observed = {}
    started = time.monotonic()
    with tempfile.TemporaryDirectory(prefix="zfg-", dir="/tmp") as temporary:
        directory = Path(temporary)
        directory.chmod(0o700)
        env, receipt["environment"] = private_environment(directory)
        process = None
        try:
            with raw_log.open("wb") as log:
                process = subprocess.Popen(
                    receipt["command"], cwd=cwd, env=env, stdin=subprocess.DEVNULL,
                    stdout=log, stderr=subprocess.STDOUT, start_new_session=True,
                )
                receipt["launcher"] = identity(process.pid)
                last_heartbeat = 0.0
                while process.poll() is None:
                    now = time.monotonic()
                    if now - last_heartbeat >= 2:
                        observed.update(descendants())
                        last_heartbeat = now
                    if stopped():
                        receipt["cancelled"] = True
                        break
                    if now - started >= timeout:
                        receipt["timed_out"] = True
                        break
                    heartbeat(now - started)
                    time.sleep(0.2)
                receipt["exit_code"] = process.poll()
        finally:
            if process is not None:
                receipt["cleanup"] = cleanup_owned(process, observed)
                receipt["exit_code"] = process.poll()
            else:
                receipt["cleanup"] = {"observed": [], "signals": [], "remaining": []}
            receipt["seconds"] = time.monotonic() - started
    receipt["runtime_removed"] = not directory.exists()
    return receipt
