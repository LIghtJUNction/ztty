#!/usr/bin/env python3
"""Real fcitx5/XIM restart and delayed startup on an owned Xvfb and D-Bus."""
import argparse
from contextlib import contextmanager
import ctypes
import hashlib
import json
import os
from pathlib import Path
import select
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import traceback

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from isolated_xvfb import start_xvfb
from input_gui import CHILD

PROFILE = """[Groups/0]
Name=Default
Default Layout=us
DefaultIM=pinyin

[Groups/0/Items/0]
Name=keyboard-us
Layout=

[Groups/0/Items/1]
Name=pinyin
Layout=

[GroupOrder]
0=Default
"""


def birth(pid):
    return Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()[19]


def descendants():
    found, pending = {}, [os.getpid()]
    while pending:
        parent = pending.pop()
        try:
            children = Path(f"/proc/{parent}/task/{parent}/children").read_text().split()
        except FileNotFoundError:
            continue
        for child in map(int, children):
            try:
                stat = Path(f"/proc/{child}/stat").read_text().rsplit(")", 1)[1].split()
                if int(stat[1]) == parent and child not in found:
                    found[child] = stat[19]
                    pending.append(child)
            except FileNotFoundError:
                pass
    return found


def wait(predicate, seconds=5):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        try:
            result = predicate()
            if result:
                return result
        except (FileNotFoundError, ConnectionRefusedError):
            pass
        time.sleep(0.025)
    return None


@contextmanager
def xim_handshakes(connection, window):
    """Record client-to-server XIM connects, including libX11's internal probes."""
    from Xlib import X, display
    from Xlib.ext import record
    atom = connection.intern_atom("_XIM_XCONNECT")
    assert connection.has_extension("RECORD"), "private Xvfb needs the RECORD extension"
    # Any XID owned by the client can select that connection. This excludes
    # the server's replies, the test driver, and brokers from earlier scenarios.
    context = connection.record_create_context(0, [window.id], [{
        "core_requests": (25, 25), "core_replies": (0, 0),  # SendEvent
        "ext_requests": (0, 0, 0, 0), "ext_replies": (0, 0, 0, 0),
        "delivered_events": (0, 0), "device_events": (0, 0), "errors": (0, 0),
        "client_started": False, "client_died": False,
    }])
    connection.sync()
    ready = threading.Event()
    handshakes, errors = [], []

    def capture(reply):
        if reply.category == record.StartOfData:
            ready.set()
        elif reply.category == record.FromClient:
            # A core SendEvent is a 12-byte request header and a 32-byte event.
            # All clients here run locally on the private display.
            assert not reply.client_swapped and len(reply.data) % 44 == 0
            for offset in range(0, len(reply.data), 44):
                event = reply.data[offset + 12:offset + 44]
                if (event[0] == X.ClientMessage and event[1] == 32
                        and int.from_bytes(event[8:12], sys.byteorder) == atom):
                    handshakes.append(int.from_bytes(event[4:8], sys.byteorder))

    def record_client():
        try:
            stream = display.Display(connection.get_display_name())
            try:
                stream.record_enable_context(context, capture)
            finally:
                stream.close()
        except Exception as error:
            errors.append(error)
            ready.set()

    reader = threading.Thread(target=record_client, daemon=True)
    reader.start()
    try:
        assert ready.wait(5), "X RECORD startup timed out"
        if errors:
            raise errors[0]
        yield handshakes
    finally:
        connection.record_disable_context(context)
        connection.sync()
        reader.join(5)
        connection.record_free_context(context)
        assert not reader.is_alive(), "X RECORD shutdown timed out"
        if errors:
            raise errors[0]


def main():
    from Xlib import X, XK, display
    from Xlib.ext import xtest
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--scenario", choices=("restart", "delayed", "callback", "creation", "all"), default="all")
    args = parser.parse_args()
    binary = args.binary.resolve()
    source = Path(__file__).resolve().parents[1]
    report = {"binary": str(binary), "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=source, text=True).strip(),
              "test_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "scope": "Owned Xvfb, private D-Bus/fcitx5, XTEST and raw PTY", "checks": [], "ok": False,
              "source_changes": subprocess.check_output(["git", "status", "--short"], cwd=source, text=True).splitlines(),
              "source_sha256": {"src/x11.rs": hashlib.sha256((source / "src/x11.rs").read_bytes()).hexdigest()}}
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    xvfb = connection = bus = fcitx = None
    def check(name, actual, expected):
        report["checks"].append({"name": name, "ok": actual == expected, "actual": actual, "expected": expected})
    with tempfile.TemporaryDirectory(prefix="ztty-xim-") as temporary:
        root = Path(temporary)
        log_path = root / "stderr.log"
        log = log_path.open("wb")
        try:
            xvfb = start_xvfb(screen="1000x700x24", stderr=log)
            report["xvfb"] = xvfb.evidence
            connection = display.Display(xvfb.display)
            assert connection.has_extension("XTEST")
            for folder in ("home", "config", "data", "cache", "runtime"):
                (root / folder).mkdir(mode=0o700)
            env = dict(os.environ, DISPLAY=xvfb.display, XMODIFIERS="@im=fcitx", LC_ALL="C.UTF-8",
                       HOME=str(root / "home"), XDG_CONFIG_HOME=str(root / "config"),
                       XDG_DATA_HOME=str(root / "data"), XDG_CACHE_HOME=str(root / "cache"),
                       XDG_RUNTIME_DIR=str(root / "runtime"))
            env.pop("WAYLAND_DISPLAY", None)
            env.pop("DBUS_SESSION_BUS_ADDRESS", None)
            env.pop("XAUTHORITY", None)
            bus = subprocess.Popen(["dbus-daemon", "--session", "--nofork", "--print-address=1",
                                    f"--address=unix:path={root / 'bus'}"], env=env, stdout=subprocess.PIPE, stderr=log)
            assert select.select([bus.stdout], [], [], 5)[0], "private D-Bus startup"
            env["DBUS_SESSION_BUS_ADDRESS"] = bus.stdout.readline().decode().strip()
            assert env["DBUS_SESSION_BUS_ADDRESS"].startswith(f"unix:path={root / 'bus'}")
            profile = root / "config/fcitx5/profile"
            profile.parent.mkdir()
            profile.write_text(PROFILE)
            config = root / "ztty.conf"
            config.write_text("font = monospace:size=11\nbackground = #000000\ncursor-blink = false\n")
            # Deterministically cover a server becoming available between the
            # first XOpenIM failure and callback registration. Only the owned
            # ztty process loads this shim; the real private fcitx is unchanged.
            shim = root / "first-open.so"
            shim_source = r"""
#include <X11/Xlib.h>
#include <X11/Xresource.h>
#include <dlfcn.h>
#include <fcntl.h>
#include <signal.h>
#include <stdarg.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
typedef XIM (*Open)(Display *, XrmDatabase, char *, char *);
typedef XIC (*Create)(XIM, ...);
XIM XOpenIM(Display *display, XrmDatabase db, char *name, char *class_name) {
    static int first = 1;
    Open next = (Open)dlsym(RTLD_NEXT, "XOpenIM");
    if (first && getenv("ZTTY_XIM_TEST_DEFER_OPEN")) { first = 0; return 0; }
    return next(display, db, name, class_name);
}
XIC XCreateIC(XIM im, ...) {
    static int first = 1;
    va_list args;
    va_start(args, im);
    const char *style_name = va_arg(args, const char *);
    XIMStyle style = va_arg(args, XIMStyle);
    const char *client_name = va_arg(args, const char *);
    Window client = va_arg(args, Window);
    const char *focus_name = va_arg(args, const char *);
    Window focus = va_arg(args, Window);
    const char *end = va_arg(args, const char *);
    va_end(args);
    // This fixture intentionally understands only ztty's fixed IC arguments.
    if (strcmp(style_name, XNInputStyle) || strcmp(client_name, XNClientWindow)
        || strcmp(focus_name, XNFocusWindow) || end) abort();
    Create next = (Create)dlsym(RTLD_NEXT, "XCreateIC");
    XIC ic = next(im, style_name, style, client_name, client, focus_name, focus, NULL);
    const char *pid_text = getenv("ZTTY_XIM_TEST_STOP_PID");
    if (first && ic && pid_text) {
        first = 0;
        Display *display = XDisplayOfIM(im);
        pid_t pid = (pid_t)strtol(pid_text, NULL, 10);
        if (pid <= 1 || kill(pid, SIGTERM)) abort();
        int filtered = 0;
        // Process only the test server's destroy event before returning its
        // now invalid IC. Ordinary focus/map/key events remain in the queue.
        for (int i = 0; i < 3000 && !filtered; ++i) {
            XEvent event;
            XSync(display, False);
            while (XCheckTypedEvent(display, DestroyNotify, &event))
                filtered += XFilterEvent(&event, None) != 0;
            if (!filtered) usleep(1000);
        }
        const char *marker = getenv("ZTTY_XIM_TEST_MARKER");
        int fd = marker ? open(marker, O_WRONLY | O_CREAT | O_EXCL, 0600) : -1;
        if (fd < 0 || write(fd, filtered ? "1" : "0", 1) != 1) abort();
        close(fd);
    }
    return ic;
}
"""
            subprocess.run(["cc", "-shared", "-fPIC", "-x", "c", "-", "-ldl", "-o", str(shim)],
                           input=shim_source, text=True, check=True, stdout=log, stderr=log, timeout=15)
            owner_atom = connection.intern_atom("@server=fcitx")
            def owner():
                value = connection.get_selection_owner(owner_atom)
                return value.id if value else 0
            def dbus(method, *values):
                return subprocess.run(["qdbus6", "org.fcitx.Fcitx5", "/controller",
                                       "org.fcitx.Fcitx.Controller1." + method, *values],
                                      env=env, text=True, capture_output=True, timeout=3)
            def contexts():
                return dbus("DebugInfo").stdout.count("frontend:xim")
            def start_im():
                nonlocal fcitx
                fcitx = subprocess.Popen(["fcitx5", "-D", "--disable", "all", "--enable",
                                          "xcb,xim,dbus,dbusfrontend,keyboard,pinyin,pinyinhelper,punctuation,fullwidth,chttrans"],
                                         env=env, stdout=log, stderr=log)
                assert wait(owner), "private XIM selection owner"
                assert wait(lambda: dbus("State").returncode == 0), "private Fcitx controller"
            def stop_im():
                nonlocal fcitx
                if fcitx:
                    fcitx.terminate()
                    fcitx.wait(timeout=5)
                    fcitx = None
                assert wait(lambda: not owner()), "private XIM owner exit"
            def rpc(request):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(4)
                    channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                    channel.sendall(json.dumps(request).encode() + b"\n")
                    data = bytearray()
                    while b"\n" not in data:
                        block = channel.recv(65536)
                        assert block, "private broker EOF"
                        data.extend(block)
                        assert len(data) <= 1048576
                value = json.loads(data.split(b"\n", 1)[0])
                assert value["ok"], value
                return value["result"]
            def launch(name):
                record, control = root / (name + ".bin"), root / (name + ".sock")
                launch_env = env.copy()
                if scenario in ("callback", "creation"):
                    launch_env["LD_PRELOAD"] = str(shim)
                if scenario == "callback":
                    launch_env["ZTTY_XIM_TEST_DEFER_OPEN"] = "1"
                if scenario == "creation" and name.endswith("-first"):
                    launch_env["ZTTY_XIM_TEST_STOP_PID"] = str(fcitx.pid)
                    launch_env["ZTTY_XIM_TEST_MARKER"] = str(root / "create-marker")
                subprocess.Popen([str(binary), "--name", name, "--config", str(config), "--geometry", "80x24",
                                  "-e", "/usr/bin/python3", "-c", CHILD, str(record), str(control)],
                                 env=launch_env, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)
                assert wait(control.exists), "private PTY ready"
                info = rpc({"op": "info", "session": name})
                view = connection.create_resource_object("window", info["window_id"])
                return {"name": name, "record": record, "control": control, "view": view, "info": info}
            def focus(window):
                window["view"].set_input_focus(X.RevertToParent, X.CurrentTime)
                connection.sync()
                time.sleep(0.06)
            def barrier(window, clear=False):
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                    channel.settimeout(4)
                    channel.connect(str(window["control"]))
                    channel.sendall(json.dumps({"clear": clear}).encode() + b"\n")
                    assert json.loads(channel.recv(4096))["ok"]
            def type_text(text):
                for letter in text:
                    code = connection.keysym_to_keycode(XK.string_to_keysym("space" if letter == " " else letter))
                    assert code, letter
                    xtest.fake_input(connection, X.KeyPress, code)
                    xtest.fake_input(connection, X.KeyRelease, code)
                    connection.sync()
                    time.sleep(0.025)
            def input_check(name, window, text, expected, chinese=False):
                focus(window)
                barrier(window, True)
                if chinese:
                    assert dbus("SetCurrentIM", "pinyin").returncode == 0
                    assert dbus("Activate").returncode == 0
                    time.sleep(0.08)
                type_text(text)
                wait(lambda: window["record"].stat().st_size > 0, 2)
                time.sleep(0.08)
                barrier(window)
                check(name, window["record"].read_bytes().hex(), expected.encode().hex())

            scenarios = ("restart", "delayed", "callback", "creation") if args.scenario == "all" else (args.scenario,)
            for scenario in scenarios:
                env["ZTTY_SOCKET"] = "@ztty-xim-" + root.name + "-" + scenario
                if scenario in ("restart", "callback", "creation"):
                    start_im()
                first = launch(scenario + "-first")
                focus(first)
                if scenario == "creation":
                    fcitx.wait(timeout=5)
                    fcitx = None
                    check("creation destroy handled before IC returned", (root / "create-marker").read_text(), "1")
                if scenario in ("restart", "callback"):
                    check(scenario + " initial XIM context", bool(wait(lambda: contexts() == 1)), True)
                    input_check(scenario + " initial Chinese commit", first, "nihao ", "你好", True)
                    stop_im()
                input_check(scenario + " ASCII without server", first, "abc", "abc")
                with xim_handshakes(connection, first["view"]) as handshakes:
                    start_im()
                    check(scenario + " existing context reconnects", bool(wait(lambda: contexts() == 1, 3)), True)
                check(scenario + " XIM connect requests for one recovery", len(handshakes), 1)
                input_check(scenario + " existing Chinese commit", first, "nihao ", "你好", True)
                second = launch(scenario + "-second")
                check(scenario + " same broker", second["info"]["broker_pid"], first["info"]["broker_pid"])
                input_check(scenario + " second window Chinese commit", second, "nihao ", "你好", True)
                input_check(scenario + " focus returns Chinese commit", first, "nihao ", "你好", True)
                stop_im()
                rpc({"op": "close", "session": second["name"]})
                start_im()
                check(scenario + " restart excludes closed context", bool(wait(lambda: contexts() == 1, 3)), True)
                input_check(scenario + " repeated restart Chinese commit", first, "nihao ", "你好", True)
                rpc({"op": "close", "session": first["name"]})
                stop_im()
            report["ok"] = all(item["ok"] for item in report["checks"])
        except Exception as error:
            report["error"] = str(error)
            report["traceback"] = traceback.format_exc()
            if "first" in locals():
                pid = first["info"]["broker_pid"]
                try:
                    reaped, status = os.waitpid(pid, os.WNOHANG)
                    report["broker_exit_before_cleanup"] = {"pid": pid, "reaped": reaped, "status": status,
                        "signal": os.WTERMSIG(status) if reaped and os.WIFSIGNALED(status) else None}
                except ChildProcessError:
                    report["broker_exit_before_cleanup"] = {"pid": pid, "not_a_child": True}
        finally:
            if connection:
                connection.close()
            for sig in (signal.SIGTERM, signal.SIGKILL):
                for pid, token in descendants().items():
                    try:
                        if birth(pid) == token:
                            os.kill(pid, sig)
                    except (FileNotFoundError, ProcessLookupError):
                        pass
                deadline = time.monotonic() + 1
                while time.monotonic() < deadline:
                    try:
                        pid, _ = os.waitpid(-1, os.WNOHANG)
                    except ChildProcessError:
                        break
                    if pid:
                        report.setdefault("reaped", []).append({"pid": pid, "status": _})
                    if pid == 0:
                        time.sleep(0.025)
            if xvfb:
                xvfb.close()
            report["cleanup"] = {"owned_processes_remaining": list(descendants())}
            report["ok"] = report["ok"] and not report["cleanup"]["owned_processes_remaining"]
            log.close()
            report["stderr"] = log_path.read_text(errors="replace")[-12000:]
            args.output.parent.mkdir(parents=True, exist_ok=True)
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({key: report[key] for key in ("ok", "binary_sha256", "checks", "cleanup")}, ensure_ascii=False))
    if not report["ok"]:
        raise SystemExit(1)


if __name__ == "__main__":
    main()
