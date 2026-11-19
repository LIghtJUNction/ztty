#!/usr/bin/env python3
"""Actual xdg-shell/EGL/PTY acceptance on a private headless Sway compositor.

No X server, shared runtime directory, physical input device or host window is
used. Requires a Wayland-enabled ztty build, sway/swaymsg, wtype, grim and Pillow.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import shutil
import signal
import subprocess
import tempfile
import time
from PIL import Image

parser = argparse.ArgumentParser()
parser.add_argument("--binary", required=True)
parser.add_argument("--output", required=True)
parser.add_argument("--sway", default=shutil.which("sway"))
parser.add_argument("--swaymsg")
args = parser.parse_args()
binary = Path(args.binary).resolve()
output = Path(args.output).resolve()
output.mkdir(parents=True, exist_ok=True)
sway = Path(args.sway or "").resolve()
swaymsg = Path(args.swaymsg).resolve() if args.swaymsg else sway.with_name("swaymsg")
if not sway.is_file() or not swaymsg.is_file():
    parser.error("--sway and its swaymsg executable are required")
checks = []
processes = []
children = []
report = {"schema": 1, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
          "source": subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip(),
          "source_worktree_dirty": bool(subprocess.check_output(["git", "status", "--porcelain"], text=True).strip()),
          "scope": "Linux headless Sway, software Mesa EGL; no physical display or IME acceptance",
          "checks": checks}
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
               XKB_DEFAULT_LAYOUT="us")
    config = runtime / "sway.conf"
    config.write_text("xwayland disable\noutput HEADLESS-1 mode 1280x720\n"
                      "output * bg #111111 solid_color\nseat seat0 fallback true\n"
                      "input * xkb_layout us\ninput * repeat_delay 250\n"
                      "input * repeat_rate 25\ndefault_border none\n"
                      "focus_follows_mouse no\n")
    app_config = output / "appearance.conf"
    app_config.write_text("font=monospace:pixelsize=16\nbackground=#161a23\n"
                          "background-opacity=100\nwindow-padding=8\ncursor-blink=false\n")
    compositor_log = (output / "compositor.log").open("w")
    compositor = subprocess.Popen([str(sway), "--unsupported-gpu", "-c", str(config)],
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
                     if n.get("app_id") == "org.project-z.ztty" and n.get("pid") == proc.pid), None)
    def screenshot(name):
        path = output / (name + ".png")
        subprocess.run(["grim", "-o", "HEADLESS-1", str(path)], env=env,
                       check=True, capture_output=True, timeout=10)
        return Image.open(path).convert("RGB")
    def start_app(name):
        root = output / name
        root.mkdir(exist_ok=True)
        log = (root / "terminal.log").open("w")
        proc = subprocess.Popen([str(binary), "--backend", "wayland", "--config",
                                 str(app_config), "--geometry", "80x24", "--cwd", str(root),
                                 "-e", shutil.which("python3"),
                                 str(Path(__file__).with_name("wayland_child.py").resolve()), str(root)],
                                env=env, stdout=log, stderr=subprocess.STDOUT,
                                start_new_session=True)
        processes.append((proc, log))
        state = wait(lambda: read(root / "state.json"))
        children.append((state["pid"], state["start_ticks"]))
        view = wait(lambda: window(proc))
        ipc(f'[con_id={view["id"]}] floating enable, resize set width 800 px height 450 px, move position 80 px 70 px, focus')
        # Give configure/SIGWINCH a stable completion point after the IPC reply.
        time.sleep(0.12)
        wait(lambda: (s := read(root / "state.json")) and s["generation"] >= 3)
        return proc, root, view["id"]
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
        report["owned_children_remaining"] = [pid for pid,ticks in children if alive(pid,ticks)]
        report["passed"] = success and bool(checks) and all(c["passed"] for c in checks) and not report["owned_children_remaining"]
        report["elapsed_seconds"] = time.monotonic() - started
        (output/"validation.json").write_text(json.dumps(report, indent=2) + "\n")
print(json.dumps({"passed":report["passed"],"checks":len(checks),"output":str(output)}))
