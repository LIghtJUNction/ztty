#!/usr/bin/env python3
"""Check default GUI broker upgrades on owned Xvfb and isolated Unix addresses."""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from measure_runtime import process_stat, same_process
from measure_multiwindow import cleanup
from repaint_gui import add_owned_children, receive, send, wait

# Only the test's processes load this shim. The real UID and peer credentials
# are unchanged; the user's default abstract socket is never contacted.
SHIM = r"""
#define _GNU_SOURCE
#include <dlfcn.h>
#include <errno.h>
#include <stddef.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/socket.h>
#include <sys/un.h>
#include <unistd.h>

int ztty_upgrade_redirect_active(void) {
    return getenv("ZTTY_UPGRADE_TEST_DEFAULT") ? 0x5054 : 0;
}

static int redirect(const struct sockaddr *address, socklen_t size,
                    struct sockaddr_un *out, socklen_t *out_size) {
    if (!address || address->sa_family != AF_UNIX
        || size < offsetof(struct sockaddr_un, sun_path) + 1)
        return 0;
    const struct sockaddr_un *local = (const struct sockaddr_un *)address;
    char expected[64];
    int length = snprintf(expected, sizeof(expected), "ztty-%u", (unsigned)getuid());
    if (local->sun_path[0] != 0
        || size != offsetof(struct sockaddr_un, sun_path) + 1 + (size_t)length
        || memcmp(local->sun_path + 1, expected, length) != 0)
        return 0;
    const char *target = getenv("ZTTY_UPGRADE_TEST_DEFAULT");
    if (!target || !*target || strlen(target) >= sizeof(out->sun_path) - 1) {
        errno = EINVAL;
        return -1;
    }
    memset(out, 0, sizeof(*out));
    out->sun_family = AF_UNIX;
    memcpy(out->sun_path + 1, target, strlen(target));
    *out_size = offsetof(struct sockaddr_un, sun_path) + 1 + strlen(target);
    return 1;
}

int connect(int fd, const struct sockaddr *address, socklen_t size) {
    int (*real)(int, const struct sockaddr *, socklen_t) = dlsym(RTLD_NEXT, "connect");
    struct sockaddr_un redirected;
    socklen_t redirected_size;
    int changed = redirect(address, size, &redirected, &redirected_size);
    if (changed < 0) return -1;
    return real(fd, changed ? (const struct sockaddr *)&redirected : address,
                changed ? redirected_size : size);
}

int bind(int fd, const struct sockaddr *address, socklen_t size) {
    int (*real)(int, const struct sockaddr *, socklen_t) = dlsym(RTLD_NEXT, "bind");
    struct sockaddr_un redirected;
    socklen_t redirected_size;
    int changed = redirect(address, size, &redirected, &redirected_size);
    if (changed < 0) return -1;
    return real(fd, changed ? (const struct sockaddr *)&redirected : address,
                changed ? redirected_size : size);
}
"""


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def child(control, title):
    channel = socket.socket(socket.AF_UNIX)
    channel.connect(control)
    os.write(1, ("\x1b]2;" + title + "\x07ready\r\n").encode())
    send(channel, {"pid": os.getpid(), "broker": os.getppid(),
                   "socket": os.environ.get("ZTTY_SOCKET")})
    while True:
        request = receive(channel)
        if request["op"] == "quit":
            return
        send(channel, {"ok": True})


def rpc(address, request):
    with socket.socket(socket.AF_UNIX) as channel:
        channel.settimeout(5)
        channel.connect("\0" + address.lstrip("@"))
        channel.sendall(json.dumps(request).encode() + b"\n")
        result = receive(channel)
        assert result["ok"], result
        return result["result"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, default=Path("artifacts/broker-upgrade.json"))
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {
        "ok": False, "binary_sha256": digest(binary), "checks": [], "sessions": [],
        "scope": "Owned Xvfb, real IPC/PTY/processes; test-only default-address redirection",
        "source_sha256": {name: digest(ROOT / name) for name in (
            "src/broker.rs", "src/platform/linux.rs", "tests/broker_upgrade.py")},
    }
    if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
        raise OSError("PR_SET_CHILD_SUBREAPER failed")
    identities, launchers, channels = {}, [], []
    xserver = listener = None
    with tempfile.TemporaryDirectory(prefix="ztty-broker-upgrade-") as temporary:
        directory = Path(temporary)
        log_path = directory / "stderr.log"
        with log_path.open("wb") as log:
            try:
                shim_source = directory / "isolate.c"
                shim_source.write_text(SHIM)
                shim = directory / "isolate.so"
                subprocess.run(["cc", "-shared", "-fPIC", "-O2", "-Wall", "-Wextra", "-Werror",
                                str(shim_source), "-o", str(shim), "-ldl"], check=True,
                               stdout=log, stderr=log)
                xserver = start_xvfb(screen="1200x800x24", dpi=96, stderr=log, timeout=10)
                report["display_ownership"] = xserver.evidence
                default = directory.name + "-default"
                env = dict(os.environ, DISPLAY=xserver.display, XMODIFIERS="@im=none",
                           LC_ALL="C.UTF-8", XDG_RUNTIME_DIR=temporary,
                           XDG_CONFIG_HOME=str(directory / "config"), LD_PRELOAD=str(shim),
                           ZTTY_UPGRADE_TEST_DEFAULT=default, ZTTY_UPGRADE_TEST=directory.name)
                env.pop("ZTTY_SOCKET", None)
                env.pop("WAYLAND_DISPLAY", None)
                env.pop("ZTTY_X11_DIAGNOSTIC_CONTROL", None)
                # Verify this subprocess can see the loaded isolation shim
                # before launching any candidate process.
                subprocess.run(
                    [sys.executable, "-c",
                     "import ctypes;assert ctypes.CDLL(None).ztty_upgrade_redirect_active()==0x5054"],
                    env=env, check=True, stdout=log, stderr=log,
                )
                builds = {}
                for generation in ("old", "new"):
                    path = directory / ("ztty-" + generation)
                    # ELF loaders ignore the suffix. Different suffixes provide
                    # real distinct executable bytes without a second build.
                    path.write_bytes(binary.read_bytes() + (
                        "\nztty-upgrade-test:" + directory.name + ":" + generation + "\n").encode())
                    path.chmod(0o755)
                    copy = directory / ("ztty-" + generation + "-copy")
                    shutil.copy2(path, copy)
                    builds[generation] = path
                    builds[generation + "-copy"] = copy
                assert digest(builds["old"]) != digest(builds["new"])
                assert builds["new"].stat().st_ino != builds["new-copy"].stat().st_ino
                config = directory / "appearance.conf"
                config.write_text("font = monospace:size=11\ncursor-blink = false\n"
                                  "background-opacity = 100\nbackground-blur = false\n")
                control = directory / "children.sock"
                listener = socket.socket(socket.AF_UNIX)
                listener.bind(str(control))
                listener.listen(12)
                listener.settimeout(10)

                def launch(generation, name, *, explicit=None, target=None):
                    run_env = env.copy()
                    if explicit is not None:
                        run_env["ZTTY_SOCKET"] = explicit
                    command = [str(builds[generation]), "--name", name, "--config", str(config),
                               "--geometry", "40x8"]
                    if target is not None:
                        command.extend(target)
                    command.extend(["-e", sys.executable, str(Path(__file__).resolve()),
                                    "--child", str(control), name])
                    started = time.monotonic()
                    process = subprocess.Popen(command, env=run_env, stdout=log,
                                               stderr=log, start_new_session=True)
                    launchers.append(process)
                    identities[process.pid] = process_stat(process.pid)["start_ticks"]
                    channel, _ = listener.accept()
                    channels.append(channel)
                    channel.settimeout(5)
                    hello = receive(channel)
                    for pid in (hello["pid"], hello["broker"]):
                        marker = ("ZTTY_UPGRADE_TEST=" + directory.name).encode()
                        assert marker in Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
                        identities[pid] = process_stat(pid)["start_ticks"]
                    owner = Path(f"/proc/{hello['broker']}/exe").resolve()
                    assert owner.parent == directory, "broker belongs to another process tree"
                    hello.update(name=name, requested=generation,
                                 executable=owner.name, elapsed_ms=(time.monotonic() - started) * 1000)
                    report["sessions"].append(hello)
                    return hello

                old = launch("old", "old-one")
                assert old["socket"] is None
                assert rpc(default, {"op": "info", "session": "old-one"})["broker_pid"] == old["broker"]
                report["checks"].append("an empty default starts the current executable normally")
                old_copy = launch("old-copy", "old-copy")
                assert old_copy["broker"] == old["broker"]
                report["checks"].append("same executable bytes at different paths share the default broker")
                builds["old"].unlink()
                assert str(Path(f"/proc/{old['broker']}/exe").readlink()).endswith(" (deleted)")
                retained = launch("old-copy", "old-deleted-copy")
                assert retained["broker"] == old["broker"]
                report["checks"].append("an unlinked running image remains readable and identical copies still share it")

                fresh = launch("new", "fresh-one")
                assert fresh["broker"] != old["broker"]
                assert fresh["socket"] and fresh["socket"].startswith(f"@ztty-{os.getuid()}-image-")
                assert same_process(old["broker"], identities[old["broker"]])
                names = {item["name"] for item in rpc(default, {"op": "list"})["sessions"]}
                assert names == {"old-one", "old-copy", "old-deleted-copy"}, names
                report["checks"].append("a changed build leaves the old broker and its sessions intact")
                assert rpc(fresh["socket"], {"op": "info", "session": "fresh-one"})["broker_pid"] == fresh["broker"]
                report["checks"].append("the selected socket is inherited by the new PTY")

                fresh_copy = launch("new-copy", "fresh-copy")
                assert (fresh_copy["broker"], fresh_copy["socket"]) == (fresh["broker"], fresh["socket"])
                report["checks"].append("same-byte copies share the upgraded broker")
                explicit = launch("new", "explicit-old", explicit=f"@ztty-{os.getuid()}")
                assert explicit["broker"] == old["broker"]
                report["checks"].append("an explicit socket keeps the caller's older target")
                tab = launch("new", "tab-old", target=["--tab", "old-one"])
                assert tab["broker"] == old["broker"]
                split = launch("new", "split-old", target=["--split-right", "old-one"])
                assert split["broker"] == old["broker"]
                report["checks"].append("tab and split requests stay in the addressed older workspace")

                listed = subprocess.run([str(builds["new"]), "list"], env=env, check=True,
                                        capture_output=True, text=True, timeout=5)
                assert "old-one" in listed.stdout and "fresh-one" not in listed.stdout, listed.stdout
                report["checks"].append("ordinary control commands keep their existing default routing")

                for name in ("old-copy", "old-deleted-copy", "explicit-old", "tab-old", "split-old", "old-one"):
                    rpc(default, {"op": "close", "session": name})
                def older_exited():
                    with contextlib.suppress(ChildProcessError):
                        os.waitpid(old["broker"], os.WNOHANG)
                    return not same_process(old["broker"], identities[old["broker"]])

                wait(older_exited, "owned older broker exits after its last session",
                     timeout=6, interval=0.05)
                after = launch("new-copy", "fresh-after-old-exit")
                assert (after["broker"], after["socket"]) == (fresh["broker"], fresh["socket"])
                report["checks"].append("the upgraded pool remains shared after the old default exits")
                assert digest(binary) == report["binary_sha256"]
                assert all(digest(ROOT / name) == value for name, value in report["source_sha256"].items())
                report["ok"] = True
            except Exception as error:
                report["error"] = str(error)
            finally:
                for channel in channels:
                    with contextlib.suppress(Exception):
                        send(channel, {"op": "quit"})
                    channel.close()
                if listener is not None:
                    listener.close()
                add_owned_children(identities, [os.getpid(), *identities])
                if xserver is not None:
                    identities.pop(xserver.process.pid, None)
                report["remaining_owned_pids"] = cleanup(identities)
                for process in launchers:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        process.wait(timeout=1)
                if xserver is not None:
                    number = xserver.number
                    xserver.close()
                    report["display_cleanup"] = {
                        "exit_code": xserver.process.returncode,
                        "pid_lock_exists": Path(f"/tmp/.X{number}-lock").exists(),
                        "socket_exists": Path(f"/tmp/.X11-unix/X{number}").exists(),
                        "lease_exists": Path(f"/tmp/ztty-xvfb-{number}.lease").exists(),
                    }
                if report["remaining_owned_pids"]:
                    report["ok"] = False
        report["stderr_tail"] = log_path.read_text(errors="replace")[-4000:]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]),
                      "error": report.get("error"), "output": str(args.output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    if len(sys.argv) == 4 and sys.argv[1] == "--child":
        child(sys.argv[2], sys.argv[3])
    else:
        raise SystemExit(main())
