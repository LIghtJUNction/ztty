#!/usr/bin/env python3
"""Exercise native SSH tabs/splits against a private real localhost sshd.

All displays, SSH keys, brokers, processes and sockets belong to this test.
The screenshot is read from the actual native split window after both panes
have rendered. No user display or configured remote host is accessed.
"""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path
import shlex
import signal
import socket
import struct
import subprocess
import sys
import tempfile
import time
import traceback

from ssh_mux import Fixture, RpcError, check, require, same_session

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from gui_renderer import record_renderer


def main():
    from PIL import Image
    from Xlib import X, XK, display, protocol
    from Xlib.ext import xtest

    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--screenshot", type=Path, required=True)
    parser.add_argument("--renderer", choices=("auto", "software", "gpu"), default="software")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    report = {
        "ok": False,
        "checks": [],
        "binary": str(binary),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "test_script_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
        "platform": "Linux X11 / owned Xvfb / real localhost OpenSSH",
        "renderer": args.renderer,
    }
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.screenshot.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix="ztty-ssh-gui-") as temporary:
        root = Path(temporary)
        fixture = Fixture(root, binary, report)
        processes = []
        xvfb = connection = None
        log = (root / "gui.stderr").open("wb")
        local_socket = "@" + root.name + "-local"

        def local_rpc(request):
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as channel:
                channel.settimeout(5)
                channel.connect("\0" + local_socket[1:])
                peer, uid, _ = struct.unpack("3i", channel.getsockopt(
                    socket.SOL_SOCKET, socket.SO_PEERCRED, 12))
                require(uid == os.getuid(), "local GUI broker belongs to another user")
                fixture.own(peer)
                channel.sendall(json.dumps(request).encode() + b"\n")
                data = bytearray()
                while b"\n" not in data:
                    block = channel.recv(65536)
                    require(block, "local GUI broker closed before its reply")
                    data.extend(block)
                    require(len(data) <= 2 * 1024 * 1024, "oversized local GUI broker reply")
            envelope = json.loads(data.split(b"\n", 1)[0])
            if not envelope.get("ok"):
                raise RpcError(envelope.get("error", repr(envelope)))
            value = envelope["result"]
            if isinstance(value.get("pid"), int) and value["pid"] > 0:
                with contextlib.suppress(ProcessLookupError):
                    fixture.own(value["pid"])
            if "broker_pid" in value:
                require(value["broker_pid"] == peer, "local metadata differs from socket owner")
            return value

        def wait(predicate, label, timeout=15):
            deadline = time.monotonic() + timeout
            while time.monotonic() < deadline:
                try:
                    value = predicate()
                except (ConnectionRefusedError, FileNotFoundError):
                    value = None
                if value:
                    return value
                fixture.pump()
                time.sleep(.02)
            raise AssertionError({"timeout": label, "launcher_exits": [p.poll() for p in processes]})

        def local_sessions():
            return local_rpc({"op": "list"})["sessions"]

        def local_info(uri):
            return local_rpc({"op": "info", "session": uri})

        def remote_name(view):
            remote = view.get("remote")
            require(isinstance(remote, dict), "SSH GUI session lost its remote profile")
            require(remote.get("destination") == "ztty-fixture", remote)
            require(isinstance(remote.get("session"), str), remote)
            return remote["session"]

        def write_local(uri, data):
            return local_rpc({"op": "write", "session": uri, "data": data})

        def prime(view, label):
            marker = fixture.file(label + "-ready")
            write_local(view["uri"], "stty -echo; printf ready > " + shlex.quote(str(marker)) +
                        "; printf '\\033[2J\\033[H'\n")
            wait(lambda: fixture.written(label + "-ready", b"ready"), label + " remote shell ready")

        def send_keys(view, text):
            current = local_info(view["uri"])
            window = connection.create_resource_object("window", current["window_id"])
            window.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            shift = connection.keysym_to_keycode(XK.string_to_keysym("Shift_L"))
            for char in text:
                symbol = XK.string_to_keysym("Return") if char == "\n" else ord(char)
                code = connection.keysym_to_keycode(symbol)
                require(code != 0, "no XTEST keycode for " + repr(char))
                shifted = connection.keycode_to_keysym(code, 0) != symbol
                if shifted:
                    require(connection.keycode_to_keysym(code, 1) == symbol,
                            "key is not in the owned display's US keyboard map")
                    xtest.fake_input(connection, X.KeyPress, shift)
                xtest.fake_input(connection, X.KeyPress, code)
                xtest.fake_input(connection, X.KeyRelease, code)
                if shifted:
                    xtest.fake_input(connection, X.KeyRelease, shift)
            connection.sync()

        def action(view, name):
            previous = {entry["id"] for entry in local_sessions()}
            local_rpc({"op": "action", "session": view["uri"], "action": name})
            created = wait(lambda: next((entry for entry in local_sessions()
                                        if entry["id"] not in previous), None), name + " creates a local pane")
            created = local_info(created["uri"])
            name = remote_name(created)
            require(name.startswith("pane-"), "related SSH pane did not allocate a remote session name")
            wait(lambda: fixture.info(name), "related SSH session " + name)
            return created

        def close_window(identifier):
            window = connection.create_resource_object("window", identifier)
            message = protocol.event.ClientMessage(
                window=window, client_type=connection.intern_atom("WM_PROTOCOLS"),
                data=(32, [connection.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0]))
            window.send_event(message, event_mask=0)
            connection.flush()

        def window_gone(identifier):
            from Xlib.error import BadWindow
            try:
                window = connection.create_resource_object("window", identifier)
                window.get_attributes()
                return False
            except BadWindow:
                return True

        def decorated(view, number):
            expected_title = "Remote " + number
            expected_lines = {
                "REMOTE WORKSPACE " + number,
                "CONNECTED",
                "中文终端 · 持久会话",
                "独立 PTY · 断开后继续运行",
                "One SSH connection",
                "Native tabs and split panes",
            }
            snapshots = {
                "remote": fixture.output(remote_name(view)),
                "local": local_rpc({"op": "read", "session": view["uri"]}),
            }
            evidence = {}
            ready = True
            for location, snapshot in snapshots.items():
                screen = snapshot["screen"]
                lines = {line.strip() for line in screen.splitlines()}
                missing = sorted(expected_lines - lines)
                echoed = "\\033" in screen or "printf" in screen
                evidence[location] = {
                    "title": snapshot["title"],
                    "missing_lines": missing,
                    "command_echo": echoed,
                }
                ready &= snapshot["title"] == expected_title and not missing and not echoed
            report.setdefault("decoration_readiness", {})[expected_title] = evidence
            return evidence if ready else None

        def decorate(view, label, number):
            prime(view, label)
            # printf receives escaped control sequences through the real SSH
            # shell; the visible text never contains paths or account details.
            picture = (
                "\x1b[2J\x1b[H\x1b[?25l\x1b]2;Remote " + number + "\x07"
                "\r\n  ZTTY  /  SSH\r\n\r\n"
                "  \x1b[1;37mREMOTE WORKSPACE " + number + "\x1b[0m\r\n\r\n"
                "  \x1b[38;2;107;208;162mCONNECTED\x1b[0m\r\n\r\n"
                "  中文终端 · 持久会话\r\n"
                "  独立 PTY · 断开后继续运行\r\n\r\n"
                "  \x1b[38;2;133;165;220mOne SSH connection\x1b[0m\r\n"
                "  Native tabs and split panes\r\n"
            )
            format_text = picture.replace("\\", "\\\\").replace("%", "%%")
            format_text = format_text.replace("\x1b", "\\033").replace("\x07", "\\007")
            format_text = format_text.replace("\r", "\\r").replace("\n", "\\n")
            write_local(view["uri"], "printf " + shlex.quote(format_text) + "; sleep 300\n")
            wait(lambda: decorated(view, number), label + " executed decoration in remote and local terminals")

        try:
            config = root / "config" / "ztty" / "ztty.conf"
            config.parent.mkdir(parents=True, exist_ok=True)
            config.write_text("background = #000000\nforeground = #dce2ec\n"
                              "background-opacity = 100\nbackground-blur = false\n"
                              "cursor-blink = false\nwindow-padding = 24\n"
                              "font = monospace:size=11\nrenderer = " + args.renderer + "\n")
            fixture.start_sshd()
            xvfb = start_xvfb(screen="1600x1000x24", stderr=log)
            connection = display.Display(xvfb.display)
            report["xserver_ownership"] = xvfb.evidence
            environment = dict(fixture.env, DISPLAY=xvfb.display, ZTTY_SOCKET=local_socket,
                               XMODIFIERS="@im=none", LIBGL_ALWAYS_SOFTWARE="1", LP_NUM_THREADS="2")
            environment.pop("WAYLAND_DISPLAY", None)
            environment.pop("WAYLAND_SOCKET", None)

            def launch_main():
                process = subprocess.Popen(
                    [str(binary), "ssh", "-F", str(fixture.ssh_config), "--remote-bin",
                     str(fixture.remote_binary), "ztty-fixture", "main", "--", "/bin/sh", "-i"],
                    env=environment, stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                    stderr=log, start_new_session=True)
                processes.append(process)
                fixture.own(process.pid)
                view = wait(lambda: next((entry for entry in local_sessions()
                                          if entry.get("remote", {}).get("session") == "main"
                                          and entry.get("attached")), None), "native SSH main window")
                return local_info(view["uri"])

            with check(report, "native_ssh_window_and_real_keyboard_input") as row:
                main_view = launch_main()
                require(remote_name(main_view) == "main", main_view)
                initial = wait(lambda: fixture.info("main"), "remote main PTY")
                require(initial["broker_pid"] != main_view["broker_pid"], "remote PTY used the local GUI broker")
                prime(main_view, "main")
                send_keys(main_view, "echo guiinput\n")
                wait(lambda: "guiinput" in fixture.output("main")["screen"], "XTEST bytes reached remote shell")
                row.update(local_broker_pid=main_view["broker_pid"], remote_broker_pid=initial["broker_pid"],
                           remote_pty_pid=initial["pid"], xtest_input="echo guiinput + Return")

            with check(report, "new_tab_inherits_remote_host_and_creates_an_independent_pty") as row:
                tab_view = action(main_view, "new-tab")
                tab_remote = fixture.info(remote_name(tab_view))
                require(tab_remote["pid"] != initial["pid"], "new tab reused the first PTY")
                require(tab_remote["broker_pid"] == initial["broker_pid"], "new tab used another remote broker")
                current_main = local_info(main_view["uri"])
                tab_view = local_info(tab_view["uri"])
                require(current_main["workspace"] == tab_view["workspace"] is not None,
                        "new SSH tab did not enter the native workspace")
                require(not current_main["visible"] and tab_view["visible"], "native tab visibility did not change")
                row.update(remote_session=tab_remote["name"], remote_pty_pid=tab_remote["pid"])

            with check(report, "split_and_new_window_keep_the_ssh_profile_and_connection") as row:
                split_view = action(tab_view, "split-right")
                detached_view = action(split_view, "new-window")
                views = [main_view, tab_view, split_view, detached_view]
                remotes = {remote_name(view): fixture.info(remote_name(view)) for view in views}
                require(len({entry["pid"] for entry in remotes.values()}) == 4, "related panes did not own separate remote PTYs")
                require({entry["broker_pid"] for entry in remotes.values()} == {initial["broker_pid"]},
                        "related panes did not share the remote broker")
                tab_view, split_view, detached_view = [local_info(view["uri"]) for view in
                                                       (tab_view, split_view, detached_view)]
                require(tab_view["workspace"] == split_view["workspace"] is not None,
                        "right split did not join the native workspace")
                require(tab_view["visible"] and split_view["visible"], "split panes are not both visible")
                require(detached_view["workspace"] is None, "new-window reused the split workspace")
                masters = fixture.masters()
                require(len(masters) == 1, "native SSH panes created multiple ControlMasters")
                authentications = (root / "sshd.log").read_text().count("Accepted publickey for ")
                require(authentications == 1, "native SSH panes repeated public-key authentication")
                row.update(remote_sessions=list(remotes), remote_pty_pids=[entry["pid"] for entry in remotes.values()],
                           control_masters=1, ssh_authentications=authentications)
                close_window(detached_view["window_id"])
                wait(lambda: window_gone(detached_view["window_id"]), "standalone SSH window closes")
                same_session(remotes[remote_name(detached_view)], fixture.info(remote_name(detached_view)))

            with check(report, "native_split_pixels_and_chinese_colored_terminal_screenshot") as row:
                decorate(tab_view, "tab", "01")
                decorate(split_view, "split", "02")
                tab_view, split_view = [local_info(view["uri"]) for view in (tab_view, split_view)]
                parent = connection.create_resource_object("window", tab_view["workspace"])
                parent.configure(x=32, y=32, stack_mode=X.Above)
                connection.sync()
                report["renderers"] = [record_renderer(connection,
                    connection.create_resource_object("window", view["window_id"]), args.renderer)
                    for view in (tab_view, split_view)]

                def capture():
                    if not decorated(tab_view, "01") or not decorated(split_view, "02"):
                        return None
                    connection.sync()
                    geometry = parent.get_geometry()
                    raw = parent.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xFFFFFFFF)
                    picture = Image.frombytes("RGB", (geometry.width, geometry.height), raw.data, "raw", "BGRX")
                    pixels = {}
                    for label, view in (("left", tab_view), ("right", split_view)):
                        pane = connection.create_resource_object("window", view["window_id"]).get_geometry()
                        # Text parsing can precede the native paint. Require the
                        # custom-colored status/footer and final text row at
                        # their actual terminal positions, not arbitrary ink
                        # from an echoed command or a previous shell prompt.
                        cell_height = (pane.height - 48) // view["rows"]
                        require(cell_height > 0, "invalid terminal pixel geometry")

                        def text_pixels(first_row, last_row, color):
                            region = picture.crop((
                                pane.x + 24,
                                pane.y + 24 + first_row * cell_height,
                                pane.x + pane.width - 24,
                                pane.y + 24 + (last_row + 1) * cell_height,
                            ))
                            return sum(all(abs(actual - expected) <= 16
                                           for actual, expected in zip(pixel, color))
                                       for pixel in region.getdata())

                        pixels[label] = {
                            "green_status": text_pixels(5, 5, (107, 208, 162)),
                            "blue_footer": text_pixels(10, 10, (133, 165, 220)),
                            "chinese_rows": text_pixels(7, 8, (220, 226, 236)),
                            "final_line": text_pixels(11, 11, (220, 226, 236)),
                        }
                    row["native_pixels"] = pixels
                    if all(all(count >= 30 for count in pane.values()) for pane in pixels.values()):
                        return picture
                    return None

                screenshot = wait(capture, "finished colored content in both native SSH panes")
                screenshot.save(args.screenshot)
                row.update(screenshot=str(args.screenshot.resolve()), width=screenshot.width, height=screenshot.height,
                           screenshot_sha256=hashlib.sha256(args.screenshot.read_bytes()).hexdigest())
                report["screenshot"] = str(args.screenshot.resolve())

            with check(report, "closing_all_native_views_preserves_remote_sessions") as row:
                workspace = tab_view["workspace"]
                close_window(workspace)
                wait(lambda: window_gone(workspace), "SSH workspace closes")
                wait(lambda: all(fixture.info(name)["input_owner"] is None for name in remotes),
                     "closing views releases remote input ownership")
                for name, before in remotes.items():
                    same_session(before, fixture.info(name))
                row["preserved_remote_pty_pids"] = [entry["pid"] for entry in remotes.values()]

            with check(report, "reopening_named_remote_gui_keeps_the_same_shell") as row:
                restored = launch_main()
                same_session(initial, fixture.info("main"))
                wait(lambda: "guiinput" in local_rpc({"op": "read", "session": restored["uri"]})["screen"],
                     "reopened GUI restores remote screen")
                require(len(fixture.masters()) == 1, "reopening did not reuse the SSH transport")
                row.update(remote_pty_pid=fixture.info("main")["pid"], restored_window_id=restored["window_id"])
                close_window(restored["window_id"])
                wait(lambda: window_gone(restored["window_id"]), "reopened native window closes")
            report["ok"] = True
        except BaseException as error:
            report["error"] = str(error)
            report["traceback"] = traceback.format_exc()
        finally:
            local_errors = []
            try:
                for view in local_sessions():
                    local_rpc({"op": "close", "session": view["uri"]})
            except (ConnectionRefusedError, FileNotFoundError):
                pass
            except BaseException as error:
                local_errors.append("local GUI cleanup: " + repr(error))
            try:
                fixture.cleanup()
            except BaseException as error:
                local_errors.append("fixture cleanup: " + repr(error))
            for process in processes:
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=3)
                    local_errors.append("GUI launcher required final termination")
            if connection is not None:
                connection.close()
            if xvfb is not None:
                try:
                    xvfb.close()
                except BaseException as error:
                    local_errors.append("private Xvfb cleanup: " + repr(error))
            log.close()
            report.setdefault("cleanup_errors", []).extend(local_errors)
            if report["cleanup_errors"] or report.get("remaining_owned_pids"):
                report["ok"] = False
            if not report["ok"]:
                report["stderr"] = (root / "gui.stderr").read_text(errors="replace")[-6000:]
            args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]),
                      "output": str(args.output), "screenshot": report.get("screenshot")}, ensure_ascii=False))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
