#!/usr/bin/env python3
"""Actual xdg-shell/EGL/PTY acceptance on a private headless Sway compositor.

Pure Wayland checks use no X server. Only the mixed-backend checks use an
owned Xvfb. No shared display, physical input device or host window is used. Requires a Wayland-enabled ztty build, sway/swaymsg, wtype, grim,
wl-clipboard, Xvfb, Pillow, python-xlib and the Wayland C development tools.
The test automatically builds its private input-method-v2 peer.
"""
import argparse
import ctypes
import secrets
import select
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from PIL import Image
sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from isolated_xvfb import start_xvfb

parser = argparse.ArgumentParser()
parser.add_argument("--binary", required=True)
parser.add_argument("--output", required=True)
parser.add_argument("--sway", default=shutil.which("sway"))
parser.add_argument("--swaymsg")
parser.add_argument("--ime-peer")
args = parser.parse_args()
binary = Path(args.binary).resolve()
output = Path(args.output).resolve()
output.mkdir(parents=True, exist_ok=True)
if not args.ime_peer:
    peer_dir = output / "ime-peer"
    subprocess.run([sys.executable, str(Path(__file__).with_name("build_wayland_ime_peer.py")),
                    "--output", str(peer_dir)], check=True)
    args.ime_peer = str(peer_dir / "ime-peer")
sway = Path(args.sway or "").resolve()
swaymsg = Path(args.swaymsg).resolve() if args.swaymsg else sway.with_name("swaymsg")
if not sway.is_file() or not swaymsg.is_file():
    parser.error("--sway and its swaymsg executable are required")
checks = []
processes = []
children = []
brokers = {}
views = {}
assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0, "cannot own/reap test broker descendants"
report = {"schema": 1, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
          "source": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
          "source_worktree_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], text=True).strip()),
          "scope": "Linux headless Sway, software Mesa EGL, private shared broker; no physical display",
          "checks": checks}
report["test_source_sha256"] = {
    name: hashlib.sha256((Path(__file__).resolve().parents[1] / name).read_bytes()).hexdigest()
    for name in ["tests/wayland_gui.py", "tests/wayland_child.py",
                 "tests/build_wayland_ime_peer.py", "tests/wayland_ime_peer.c",
                 "tests/protocols/input-method-unstable-v2.xml", "tools/isolated_xvfb.py"]
}
started = time.monotonic()
def pixels(image):
    # Pillow 10.x (Ubuntu 24.04) exposes getdata; newer Pillow replaces it.
    flattened = getattr(image, "get_flattened_data", None)
    return flattened() if flattened else image.getdata()

def check(name, passed, **details):
    checks.append({"name": name, "passed": bool(passed), **details})
    if not passed:
        raise AssertionError(name + ": " + repr(details))
def wait(predicate, seconds=10):
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(0.02)
    raise TimeoutError("condition did not become true")
def read(path):
    try:
        return json.loads(path.read_text())
    except (FileNotFoundError, json.JSONDecodeError):
        return None
def alive(pid, ticks):
    try:
        data = Path(f"/proc/{pid}/stat").read_text().split()
        return data[21] == ticks and data[2] != "Z"
    except FileNotFoundError:
        return False
def nodes(tree):
    yield tree
    for node in tree.get("nodes", []) + tree.get("floating_nodes", []):
        yield from nodes(node)

with tempfile.TemporaryDirectory(prefix="ztty-wayland-") as temp:
    runtime = Path(temp)
    os.chmod(runtime, 0o700)
    env = os.environ.copy()
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "SWAYSOCK"):
        env.pop(key, None)
    env.update(XDG_RUNTIME_DIR=str(runtime), WLR_BACKENDS="headless",
               WLR_HEADLESS_OUTPUTS="1", WLR_RENDERER="pixman",
               WLR_LIBINPUT_NO_DEVICES="1", LIBGL_ALWAYS_SOFTWARE="1",
               XDG_CONFIG_HOME=str(runtime / "config"), XKB_DEFAULT_LAYOUT="us", ZTTY_SOCKET="@ztty-wayland-test-" + secrets.token_hex(12))
    config = runtime / "sway.conf"
    config.write_text("xwayland disable\noutput HEADLESS-1 mode 1280x720\n"
                      "output * bg #111111 solid_color\nseat seat0 fallback true\n"
                      "input * xkb_layout us\ninput * repeat_delay 250\n"
                      "input * repeat_rate 25\ndefault_border none\n"
                      "focus_follows_mouse no\n")
    app_config = output / "appearance.conf"
    app_config.write_text("font=monospace:pixelsize=16\nbackground=#161a23\n"
                          "background-opacity=100\nwindow-padding=8\ncursor-blink=false\n")
    (runtime / "config/ztty").mkdir(parents=True)
    (runtime / "config/ztty/config").write_text(app_config.read_text())
    compositor_log = (output / "compositor.log").open("w")
    compositor = subprocess.Popen([str(sway), "--debug", "--unsupported-gpu", "-c", str(config)],
                                  env=env, stdout=compositor_log, stderr=subprocess.STDOUT,
                                  start_new_session=True)
    def ipc(message, kind=None):
        cmd = [str(swaymsg), "-r"]
        if kind:
            cmd += ["-t", kind]
        cmd += [message] if message else []
        result = subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=5)
        if result.returncode:
            raise RuntimeError(result.stderr)
        value = json.loads(result.stdout)
        if not kind and isinstance(value, list) and any(not x.get("success") for x in value):
            raise RuntimeError(repr(value))
        return value
    def window(proc):
        return next((n for n in nodes(ipc("", "get_tree"))
                     if n.get("app_id") == "org.project-z.ztty" and n.get("id") == views.get(proc.pid)), None)
    def cli(*arguments):
        r = subprocess.run([str(binary), *arguments], env=env, capture_output=True, text=True, timeout=8)
        if r.returncode:
            raise RuntimeError(r.stderr or r.stdout)
        return json.loads(r.stdout)
    def remember_broker(info):
        pid = info["broker_pid"]
        if pid not in brokers:
            assert Path(f"/proc/{pid}/exe").resolve() == binary, "unexpected broker executable"
            brokers[pid] = Path(f"/proc/{pid}/stat").read_text().split()[21]
        return pid
    def screenshot(name):
        path = output / (name + ".png")
        subprocess.run(["grim", "-o", "HEADLESS-1", str(path)], env=env,
                       check=True, capture_output=True, timeout=10)
        return Image.open(path).convert("RGB")
    def start_app(name):
        root = output / name
        root.mkdir(exist_ok=True)
        log = (root / "terminal.log").open("w")
        existing = {n.get("id") for n in nodes(ipc("", "get_tree"))}
        proc = subprocess.Popen([str(binary), "--backend", "wayland", "--name", name, "--config",
                                 str(app_config), "--geometry", "80x24", "--cwd", str(root),
                                 "-e", sys.executable,
                                 str(Path(__file__).with_name("wayland_child.py").resolve()), str(root)],
                                env=env, stdout=log, stderr=subprocess.STDOUT,
                                start_new_session=True)
        processes.append((proc, log))
        state = wait(lambda: read(root / "state.json"))
        children.append((state["pid"], state["start_ticks"]))
        info = cli("info", name)
        broker_pid = remember_broker(info)
        view = wait(lambda: next((n for n in nodes(ipc("", "get_tree"))
                     if n.get("app_id") == "org.project-z.ztty" and n.get("pid") == broker_pid
                     and n.get("id") not in existing), None))
        views[proc.pid] = view["id"]
        initial_generation = state["generation"]
        ipc(f'[con_id={view["id"]}] floating enable, resize set width 800 px height 450 px, move position 80 px 70 px, focus')
        # Give configure/SIGWINCH a stable completion point after the IPC reply.
        time.sleep(0.12)
        wait(lambda: (s := read(root / "state.json")) and s["generation"] > initial_generation)
        return proc, root, view["id"]
    def start_x11(name, xconnection):
        root = output / name
        root.mkdir()
        log = (root / "terminal.log").open("w")
        proc = subprocess.Popen([str(binary), "--name", name, "--config", str(app_config),
                                 "--geometry", "80x24", "--cwd", str(root), "-e",
                                 sys.executable,
                                 str(Path(__file__).with_name("wayland_child.py").resolve()), str(root)],
                                env=env, stdout=log, stderr=subprocess.STDOUT,
                                start_new_session=True)
        processes.append((proc, log))
        state = wait(lambda: read(root / "state.json"))
        children.append((state["pid"], state["start_ticks"]))
        info = cli("info", name)
        broker_pid = remember_broker(info)
        atom = xconnection.intern_atom("_NET_WM_PID")
        def actual_window():
            for item in xconnection.screen().root.query_tree().children:
                value = item.get_full_property(atom, 0)
                if value is not None and int(value.value[0]) == broker_pid:
                    return item.get_attributes().map_state == 2
            return False
        wait(actual_window)
        check(name + "_actual_x11_window", info["backend"] == "x11" and info["attached"])
        return proc, root, None
    def mixed_backends():
        from Xlib import display as xdisplay
        saved = {key: env.get(key) for key in ("DISPLAY", "XMODIFIERS", "ZTTY_SOCKET")}
        log = (output / "mixed-xvfb.log").open("w")
        server = start_xvfb(screen="1280x720x24", stderr=log)
        connection = None
        report["mixed_xvfb"] = server.evidence
        try:
            connection = xdisplay.Display(server.display)
            env["DISPLAY"] = server.display
            env["XMODIFIERS"] = "@im=none"
            for first, second in [("x11", "wayland"), ("wayland", "x11")]:
                env["ZTTY_SOCKET"] = "@ztty-wayland-mixed-" + secrets.token_hex(12)
                opened = []
                for backend in (first, second):
                    name = f"mixed-{first}-first-{backend}"
                    opened.append((name, start_app(name) if backend == "wayland"
                                   else start_x11(name, connection)))
                left = cli("info", opened[0][0])
                right = cli("info", opened[1][0])
                check(first + "_first_routes_both_backends_in_one_broker",
                      left["backend"] == first and right["backend"] == second
                      and left["broker_pid"] == right["broker_pid"])
                check(first + "_first_keeps_separate_ptys", left["pid"] != right["pid"])
                for name, (app, root, _) in opened:
                    cli("close", name)
                    app.wait(timeout=5)
                    state = read(root / "state.json")
                    wait(lambda: not alive(state["pid"], state["start_ticks"]))
                check(first + "_first_mixed_sessions_closed", True)
        finally:
            for key, value in saved.items():
                if value is None: env.pop(key, None)
                else: env[key] = value
            if connection is not None:
                connection.close()
            server.close()
            log.close()
        ipc(f"[con_id={con_id}] focus")
    helpers = []
    helper_logs = []
    def helper(command, label):
        log = (output / (label + ".log")).open("wb")
        helper_logs.append(log)
        p = subprocess.Popen(command, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             stderr=log, start_new_session=True, bufsize=0)
        helpers.append(p)
        return p
    def received_line(p, seconds=5):
        if not select.select([p.stdout], [], [], seconds)[0]:
            raise TimeoutError("test peer did not answer")
        data = p.stdout.readline()
        if not data:
            raise RuntimeError("test peer exited: " + repr(p.poll()))
        return data.decode().strip()
    class Mcp:
        def __init__(self):
            self.proc = helper([str(binary), "mcp"], "mcp")
            self.ident = 0
            self.rpc("initialize", {"protocolVersion":"2025-03-26", "capabilities":{},
                                     "clientInfo":{"name":"native-wayland-test", "version":"1"}})
            self.proc.stdin.write(b'{"jsonrpc":"2.0","method":"notifications/initialized"}\n')
            self.proc.stdin.flush()
        def rpc(self, method, params):
            self.ident += 1
            value = {"jsonrpc":"2.0", "id":self.ident, "method":method, "params":params}
            self.proc.stdin.write(json.dumps(value).encode() + b"\n")
            self.proc.stdin.flush()
            while True:
                response = json.loads(received_line(self.proc))
                if response.get("id") == self.ident:
                    assert "result" in response, response
                    return response["result"]
        def tool(self, name, arguments, error=False):
            value = self.rpc("tools/call", {"name":name, "arguments":arguments})
            assert bool(value.get("isError")) == error, value
            return value.get("structuredContent")
    def clipboard_owner(payload, primary=False):
        p = helper(["wl-copy", "--foreground", "--type", "text/plain;charset=utf-8"]
                   + (["--primary"] if primary else []), "clipboard-owner-" + str(len(helpers)))
        p.stdin.write(payload)
        p.stdin.close()
        deadline = time.monotonic() + 5
        while time.monotonic() < deadline:
            # The first read can still target the previous owner while the
            # new wl-copy connects. A stopped old owner must not abort this
            # readiness check; the successful payload remains exact.
            try:
                r = subprocess.run(["wl-paste", "--no-newline"] + (["--primary"] if primary else []),
                                   env=env, capture_output=True, timeout=.5)
                if r.returncode == 0 and r.stdout == payload:
                    return p
            except subprocess.TimeoutExpired:
                pass
            if p.poll() is not None:
                raise RuntimeError("clipboard owner exited before becoming ready")
            time.sleep(.02)
        raise TimeoutError("clipboard offer did not become ready")
    def clipboard_read(primary=False):
        return subprocess.run(["wl-paste", "--no-newline"] + (["--primary"] if primary else []),
                              env=env, capture_output=True, check=True, timeout=4).stdout
    def attach_app(selector, observe=False):
        existing = {n.get("id") for n in nodes(ipc("", "get_tree"))}
        log = (output / ("attach-" + str(len(processes)) + ".log")).open("w")
        p = subprocess.Popen([str(binary), "attach", "--backend", "wayland"]
                             + (["--observe"] if observe else []) + [selector], env=env,
                             stdout=log, stderr=subprocess.STDOUT, start_new_session=True)
        processes.append((p, log))
        view = wait(lambda: next((n for n in nodes(ipc("", "get_tree"))
                     if n.get("app_id") == "org.project-z.ztty" and n.get("id") not in existing), None))
        views[p.pid] = view["id"]
        ipc(f'[con_id={view["id"]}] floating enable, resize set width 800 px height 450 px, move position 80 px 70 px, focus')
        time.sleep(.12)
        return p, view["id"]
    def ime_command(peer, text):
        peer.stdin.write(text.encode() + b"\n")
        peer.stdin.flush()
        while not (line := received_line(peer)).startswith("SENT "):
            assert not line.startswith(("UNAVAILABLE", "INVALID")), line
    command_id = 0
    def command(root, data=b"", exit_code=None):
        global command_id
        command_id += 1
        value = {"id": command_id, "bytes": base64.b64encode(data).decode()}
        if exit_code is not None:
            value["exit"] = exit_code
        temp = root / "command.tmp"
        temp.write_text(json.dumps(value))
        temp.replace(root / "command.json")
        wait(lambda: (s := read(root / "state.json")) and s["command"] == command_id)
    def input_bytes(root):
        path = root / "input.bin"
        return path.read_bytes() if path.exists() else b""
    def keyboard(root, arguments, expected, name):
        before = len(input_bytes(root))
        # wtype adds a fresh virtual device: allow the seat capability and
        # wl_keyboard subscription roundtrip before injecting its first key.
        subprocess.run(["wtype", "-s", "100", *arguments], env=env, check=True, capture_output=True, timeout=10)
        wait(lambda: len(input_bytes(root)) >= before + len(expected))
        time.sleep(0.03)
        got = input_bytes(root)[before:]
        check(name, got == expected, expected_hex=expected.hex(), actual_hex=got.hex())
    success = False
    try:
        socket = wait(lambda: next(runtime.glob("sway-ipc.*.sock"), None))
        display = wait(lambda: next((x for x in runtime.glob("wayland-*") if not x.name.endswith(".lock")), None))
        env.update(SWAYSOCK=str(socket), WAYLAND_DISPLAY=display.name)
        report["compositor_version"] = ipc("", "get_version")["human_readable"]
        proc, root, con_id = start_app("interaction")
        state = read(root / "state.json")
        check("native_surface_and_pty", proc.poll() is None and bool(window(proc)), app_id="org.project-z.ztty")
        check("cwd_and_terminal_environment", state["pwd_matches_cwd"] and state["term"] == "xterm-256color"
              and state["program"] == "ztty", state={k:state[k] for k in ("cols","rows","term","program","pwd_matches_cwd")})
        time.sleep(0.15)
        image = screenshot("initial")
        red = sum(1 for p in pixels(image) if p == (192,32,48))
        check("actual_egl_pixels", red > 300, red_pixels=red)
        keyboard(root, ["-d","8","Hello ztty!"], b"Hello ztty!", "ascii_keyboard")
        keyboard(root, ["中文"], "中文".encode(), "unicode_keyboard")
        keyboard(root, ["-M","ctrl","c","-m","ctrl"], b"\x03", "control_key")
        keyboard(root, ["-k","Up","-k","Left"], b"\x1b[A\x1b[D", "navigation_keys")
        keyboard(root, ["-M","alt","x","-m","alt"], b"\x1bx", "alt_key")
        before_repeat = len(input_bytes(root))
        subprocess.run(["wtype","-s","100","-P","r","-s","520","-p","r"], env=env, check=True, capture_output=True, timeout=5)
        time.sleep(0.04)
        repeated = input_bytes(root)[before_repeat:]
        check("keyboard_repeat", len(repeated) >= 3 and set(repeated) == {ord("r")}, bytes=len(repeated))
        length = len(input_bytes(root))
        time.sleep(0.15)
        check("repeat_stops_on_release", len(input_bytes(root)) == length)
        command(root, b"\x1b[>11u")
        keyboard(root, ["-k","a"], b"\x1b[97;1:1u\x1b[97;1:3u", "kitty_press_release")
        command(root, b"\x1b[<u\x1b]2;Wayland title verified\x07")
        wait(lambda: (v := window(proc)) and v.get("name") == "Wayland title verified")
        check("osc_title_reaches_compositor", window(proc)["name"] == "Wayland title verified")

        # Each compositor configure must resize the real child terminal.
        sizes = []
        for width,height in [(620,330),(990,550),(410,240),(800,450)]:
            old = read(root / "state.json")["generation"]
            ipc(f'[con_id={con_id}] resize set width {width} px height {height} px')
            state = wait(lambda: (s := read(root / "state.json")) and s["generation"] > old and s)
            sizes.append([width,height,state["cols"],state["rows"]])
        check("configure_resizes_real_pty", len({(s[2],s[3]) for s in sizes}) == 4, sizes=sizes)
        time.sleep(0.08)
        image = screenshot("resized")
        check("text_survives_resize", sum(1 for p in pixels(image) if p == (192,32,48)) > 300)
        command(root, b"\x1b[2J\x1b[H" + b"".join(f"history-{i:04d}\r\n".encode() for i in range(100)))
        time.sleep(0.08)
        live = screenshot("history-live")
        subprocess.run(["wtype","-s","100","-M","shift","-k","Page_Up","-m","shift"], env=env, check=True, capture_output=True)
        time.sleep(0.08)
        history = screenshot("history-scrolled")
        check("history_keyboard_scroll", live.tobytes() != history.tobytes())
        subprocess.run(["wtype","-s","100","-M","shift","-k","Page_Down","-m","shift"], env=env, check=True, capture_output=True)
        time.sleep(0.08)
        check("history_returns_live", screenshot("history-return").tobytes() == live.tobytes())
        command(root, b"\x1b[2J\x1b[H\x1b[48;2;192;32;48m" + b" "*20 + b"\x1b[0m")
        time.sleep(0.08)
        before = screenshot("sync-before")
        sync_started = time.monotonic()
        command(root, b"\x1b[?2026h\x1b[2J\x1b[H\x1b[48;2;32;192;48m" + b" "*20 + b"\x1b[0m")
        time.sleep(0.04)
        during = screenshot("sync-during")
        elapsed = time.monotonic() - sync_started
        command(root, b"\x1b[?2026l")
        time.sleep(0.08)
        after = screenshot("sync-after")
        check("synchronized_full_frame", elapsed < .25 and before.tobytes() == during.tobytes()
              and after.tobytes() != before.tobytes(), capture_seconds=elapsed)
        check("green_frame_visible", sum(1 for p in pixels(after) if p == (32,192,48)) > 300)
        info = cli("info", "interaction")
        check("native_window_uses_shared_broker", info["backend"] == "wayland" and info["pid"] == state["pid"]
              and info["broker_pid"] != proc.pid and len(brokers) == 1,
              backend=info["backend"], pty_pid=info["pid"], broker_pid=info["broker_pid"], launcher_pid=proc.pid)
        original_info = cli("info", "interaction")
        sibling, sibling_root, sibling_view = start_app("sibling")
        sibling_info = cli("info", "sibling")
        check("concurrent_native_windows_share_broker_not_pty", sibling_info["broker_pid"] == original_info["broker_pid"]
              and sibling_info["pid"] != original_info["pid"] and window(proc) is not None and window(sibling) is not None)
        cli("close", "sibling")
        check("closing_sibling_preserves_other_native_window", sibling.wait(timeout=5) == 0 and window(proc) is not None
              and cli("info", "interaction")["pid"] == original_info["pid"])
        ipc(f"[con_id={con_id}] focus")
        mcp = Mcp()
        shared = mcp.tool("session_read", {"session":"interaction", "max_bytes":65536})
        check("mcp_reads_same_native_pty", shared["pid"] == read(root / "state.json")["pid"]
              and shared["backend"] == "wayland" and shared["broker_pid"] in brokers)
        mcp.tool("session_write", {"session":"interaction", "data":"forbidden"}, error=True)
        check("mcp_preserves_local_input_owner", cli("info", "interaction")["input_owner"] == "local")
        mcp.tool("session_control", {"session":"interaction", "action":"acquire", "takeover":True})
        before_input = len(input_bytes(root))
        mcp.tool("session_write", {"session":"interaction", "data":"MCP-SAME-PTY"})
        wait(lambda: input_bytes(root)[before_input:] == b"MCP-SAME-PTY")
        check("mcp_write_reaches_same_pty", input_bytes(root)[before_input:] == b"MCP-SAME-PTY")
        keyboard(root, ["q"], b"", "local_keyboard_blocked_while_mcp_owns")
        cli("control", "interaction", "acquire", "--takeover")
        keyboard(root, ["y"], b"y", "local_control_reacquired")

        # Paste owns a complete asynchronous transfer; it must not block input.
        payload = "Wayland clipboard · 中文\nline two".encode()
        clipboard_owner(payload)
        command(root, b"\x1b[?2004h")
        keyboard(root, ["-M","ctrl","-M","shift","v","-m","shift","-m","ctrl"],
                 b"\x1b[200~" + payload + b"\x1b[201~", "native_clipboard_utf8_bracketed_paste")
        primary = "primary selection 中文".encode()
        clipboard_owner(primary, primary=True)
        before_input = len(input_bytes(root))
        cli("action", "interaction", "paste-primary")
        wait(lambda: len(input_bytes(root)) >= before_input + len(primary) + 12)
        check("native_primary_paste", input_bytes(root)[before_input:] == b"\x1b[200~" + primary + b"\x1b[201~")
        command(root, b"\x1b[?2004l")
        exported = "native OSC52 clipboard · UTF8".encode()
        command(root, b"\x1b]52;c;" + base64.b64encode(exported) + b"\x07")
        wait(lambda: clipboard_read() == exported)
        check("native_clipboard_export", clipboard_read() == exported)
        exported_primary = "native primary export".encode()
        command(root, b"\x1b]52;p;" + base64.b64encode(exported_primary) + b"\x07")
        time.sleep(.12)
        report["primary_export_before_new_input"] = clipboard_read(True).decode("utf-8", "replace")
        # wlroots rejects selection claims superseded by another owner's newer
        # serial. A real user action supplies a valid serial; never fabricate it.
        keyboard(root, ["-M","shift","-m","shift"], b"", "primary_copy_uses_real_input_serial")
        command(root, b"\x1b]52;p;" + base64.b64encode(exported_primary) + b"\x07")
        wait(lambda: clipboard_read(True) == exported_primary)
        check("native_primary_export", clipboard_read(True) == exported_primary)
        before_input = len(input_bytes(root))
        command(root, b"\x1b]52;c;?\x07")
        expected = b"\x1b]52;c;" + base64.b64encode(exported) + b"\x07"
        wait(lambda: len(input_bytes(root)) >= before_input + len(expected))
        check("native_osc52_reads_clipboard", input_bytes(root)[before_input:] == expected)

        stalled = clipboard_owner(b"late clipboard must not enter next paste")
        os.kill(stalled.pid, signal.SIGSTOP)
        before_input = len(input_bytes(root))
        cli("action", "interaction", "paste")
        stamp = time.monotonic()
        keyboard(root, ["n"], b"n", "slow_clipboard_does_not_block_keyboard")
        check("slow_clipboard_input_latency", time.monotonic() - stamp < 1.0)
        time.sleep(2.15)
        check("stalled_clipboard_times_out_without_partial_paste", input_bytes(root)[before_input:] == b"n")
        replacement = b"replacement clipboard survives late writer"
        clipboard_owner(replacement)
        os.kill(stalled.pid, signal.SIGCONT)
        keyboard(root, ["-M","ctrl","-M","shift","v","-m","shift","-m","ctrl"],
                 replacement, "clipboard_recovers_after_timeout")

        if args.ime_peer:
            # Real input-method-v2 peer -> compositor -> ztty text-input-v3.
            command(root, b"\x1b[2J\x1b[H\x1b[?25h")
            time.sleep(.1)
            # Each ordinary wtype invocation removes its virtual keyboard on
            # exit. A real desktop retains a keyboard while the IME is active;
            # hold an owned, idle virtual keyboard for this protocol check.
            ime_keyboard = helper(["wtype", "-s", "60000"], "ime-keyboard")
            time.sleep(.12)
            ipc(f"[con_id={con_id}] focus")
            peer = helper([str(Path(args.ime_peer).resolve())], "input-method")
            active = None
            report["ime_states"] = []
            while active is None:
                line = received_line(peer)
                report["ime_states"].append(line)
                assert line != "UNAVAILABLE", line
                if line.startswith("STATE 1 "):
                    active = line.split()
            check("ime_requests_terminal_without_fake_surrounding", active[3:] == ["13","0"], state=line)
            before_input = len(input_bytes(root))
            rectangle = window(proc)["rect"]
            crop = (rectangle["x"], rectangle["y"], rectangle["x"] + min(rectangle["width"], 600), rectangle["y"] + 40)
            before_pixels = screenshot("ime-before").crop(crop).tobytes()
            ime_command(peer, "preedit 拼音预编辑")
            wait(lambda: screenshot("ime-preedit").crop(crop).tobytes() != before_pixels)
            check("ime_preedit_is_visible_without_pty_input", input_bytes(root)[before_input:] == b"")
            committed = "中文输入法 👩🏽‍💻 café"
            ime_command(peer, "commit " + committed)
            wait(lambda: len(input_bytes(root)) >= before_input + len(committed.encode()))
            check("ime_commit_exact_utf8_once", input_bytes(root)[before_input:] == committed.encode())
            before_input = len(input_bytes(root))
            long_commit = "中文é👩🏽‍💻" * 8
            ime_command(peer, "commit " + long_commit)
            wait(lambda: len(input_bytes(root)) >= before_input + len(long_commit.encode()))
            check("ime_chunk_boundaries_preserve_utf8", input_bytes(root)[before_input:] == long_commit.encode())
            before_input = len(input_bytes(root))
            ime_command(peer, "delete")
            time.sleep(.12)
            check("ime_cannot_delete_unowned_shell_buffer", input_bytes(root)[before_input:] == b"")
            ime_command(peer, "commit after-rejected-delete")
            wait(lambda: input_bytes(root)[before_input:] == b"after-rejected-delete")
            check("ime_recovers_after_invalid_delete", input_bytes(root)[before_input:] == b"after-rejected-delete")
            peer.stdin.write(b"quit\n"); peer.stdin.flush()
            check("ime_peer_clean_exit", peer.wait(timeout=5) == 0)
            ime_keyboard.terminate()
            ime_keyboard.wait(timeout=5)

        mixed_backends()
        check("mixed_backend_tests_preserve_original_session",
              cli("info", "interaction")["pid"] == read(root / "state.json")["pid"])
        original = cli("info", "interaction")
        cli("detach", "interaction")
        check("detach_keeps_native_pty_alive", proc.wait(timeout=5) == 0 and alive(original["pid"], children[0][1]))
        proc, con_id = attach_app("interaction", observe=True)
        observed = cli("info", "interaction")
        check("native_observer_reuses_same_pty", observed["pid"] == original["pid"] and observed["read_only"]
              and observed["backend"] == "wayland" and observed["broker_pid"] == original["broker_pid"])
        keyboard(root, ["x"], b"", "observer_keyboard_cannot_write")
        cli("detach", "interaction")
        check("observer_detach_keeps_pty_alive", proc.wait(timeout=5) == 0 and alive(original["pid"], children[0][1]))
        proc, con_id = attach_app("interaction")
        cli("control", "interaction", "acquire", "--takeover")
        keyboard(root, ["z"], b"z", "reattached_native_view_controls_same_pty")
        check("reattach_identity_stays_stable", cli("info", "interaction")["pid"] == original["pid"])
        # Detach makes a session persistent. Closing its view preserves that
        # contract, then the explicit close below owns the child cleanup.
        ipc(f"[con_id={con_id}] kill")
        check("persistent_native_window_close_detaches", proc.wait(timeout=5) == 0 and alive(original["pid"], children[0][1]))
        cli("close", "interaction")
        wait(lambda: not alive(original["pid"], children[0][1]))
        check("explicit_close_reaps_persistent_pty", not alive(original["pid"], children[0][1]))
        proc, root, con_id = start_app("ordinary-close")
        # A long-lived child is closed through xdg_toplevel.close, not a process kill.
        ipc(f"[con_id={con_id}] kill")
        check("compositor_close_exits_cleanly", proc.wait(timeout=5) == 0)
        wait(lambda: not alive(*children[-1]))
        check("closed_window_reaps_child", not alive(*children[-1]))
        proc2, root2, _ = start_app("child-exit")
        command(root2, exit_code=37)
        check("child_exit_code_propagates", proc2.wait(timeout=5) == 37)
        proc3, root3, _ = start_app("disconnect")
        compositor.terminate()
        compositor.wait(timeout=5)
        check("lost_compositor_reports_error", proc3.wait(timeout=5) == 1)
        wait(lambda: not alive(*children[-1]))
        check("lost_compositor_reaps_child", not alive(*children[-1]))
        success = True
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
        raise
    finally:
        if compositor.poll() is None:
            try:
                ipc("exit")
                compositor.wait(timeout=3)
            except (Exception,):
                compositor.terminate()
        for helper_proc in helpers:
            if helper_proc.poll() is None:
                # Some intentional slow-owner checks stop an owned helper.
                try: os.kill(helper_proc.pid, signal.SIGCONT)
                except ProcessLookupError: pass
                helper_proc.terminate()
                try: helper_proc.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    helper_proc.kill(); helper_proc.wait(timeout=3)
            for stream in (helper_proc.stdin, helper_proc.stdout):
                if stream and not stream.closed: stream.close()
        for helper_log in helper_logs: helper_log.close()
        for proc, log in processes:
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait(timeout=3)
            log.close()
        for pid,ticks in children:
            if alive(pid,ticks):
                os.kill(pid, signal.SIGKILL)
        if compositor.poll() is None:
            compositor.kill()
        compositor.wait(timeout=5)
        compositor_log.close()
        for pid,ticks in brokers.items():
            deadline = time.monotonic() + 5
            while alive(pid,ticks) and time.monotonic() < deadline:
                time.sleep(.02)
            if alive(pid,ticks):
                fd = os.pidfd_open(pid)
                signal.pidfd_send_signal(fd, signal.SIGTERM)
                os.close(fd)
            try:
                os.waitpid(pid, 0)
            except ChildProcessError:
                pass
        report["owned_brokers_remaining"] = [pid for pid,ticks in brokers.items() if alive(pid,ticks)]
        report["owned_children_remaining"] = [pid for pid,ticks in children if alive(pid,ticks)]
        report["owned_helpers_remaining"] = [p.pid for p in helpers if p.poll() is None]
        report["passed"] = success and bool(checks) and all(c["passed"] for c in checks) and not report["owned_children_remaining"] and not report["owned_brokers_remaining"] and not report["owned_helpers_remaining"]
        report["elapsed_seconds"] = time.monotonic() - started
        (output/"validation.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps({"passed":report["passed"],"checks":len(checks),"output":str(output)}))
