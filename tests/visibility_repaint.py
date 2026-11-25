#!/usr/bin/env python3
"""Exercise quiet X11 windows on an owned Xvfb. Simulated surface loss is explicit."""
import argparse
import contextlib
import ctypes
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

from Xlib import X, display, error, protocol

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from gui_renderer import record_renderer
from isolated_xvfb import start_xvfb
from measure_multiwindow import cleanup
from repaint_gui import add_owned_children
from tab_switch_pixels import wait


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--renderer", required=True, choices=("software", "gpu"))
    parser.add_argument("--expect", choices=("repaired", "baseline"), default="repaired")
    parser.add_argument("--disable-composite", action="store_true",
                        help="start the private Xvfb with -extension COMPOSITE")
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    with (tempfile.TemporaryDirectory(prefix="ztty-visibility-") as temporary,
          start_xvfb(dpi=96, disable_composite=args.disable_composite) as server):
        folder = Path(temporary)
        env = dict(os.environ, DISPLAY=server.display, XMODIFIERS="@im=none",
                   XDG_RUNTIME_DIR=temporary, XDG_CONFIG_HOME=str(folder / "config"),
                   XDG_CACHE_HOME=str(folder / "cache"), XDG_STATE_HOME=str(folder / "state"),
                   ZTTY_SOCKET="@" + folder.name)
        env.pop("WAYLAND_DISPLAY", None)
        config = folder / "ztty.conf"
        config.write_text(f"renderer = {args.renderer}\ncursor-blink = false\n"
                          "background = #253749\nbackground-opacity = 100\n")
        connection = display.Display(server.display)
        root = connection.screen().root
        child = "printf '\\033[?25l\\033[31mQuiet terminal\\033[0m\\n'; exec sleep 45"
        launcher = subprocess.Popen([str(binary), "--config", str(config), "--name", "quiet",
                                     "--geometry", "40x8", "-e", "/bin/sh", "-c", child],
                                    env=env, start_new_session=True)
        def rpc(op):
            with socket.socket(socket.AF_UNIX) as channel:
                channel.settimeout(1)
                channel.connect("\0" + env["ZTTY_SOCKET"][1:])
                channel.sendall(json.dumps({"op": op, "session": "quiet"}).encode() + b"\n")
                with channel.makefile("rb") as reader:
                    reply = json.loads(reader.readline())
                return reply["result"] if reply["ok"] else None
        try:
            info = wait(lambda: rpc("info"), "owned session")
            view = connection.create_resource_object("window", info["window_id"])
            wait(lambda: "Quiet terminal" in (rpc("read") or {}).get("screen", ""), "quiet test content")
            renderer = record_renderer(connection, view, args.renderer)
            def selection_owner(name):
                owner = connection.get_selection_owner(connection.intern_atom(name))
                return getattr(owner, "id", owner)
            def redirect_probe():
                if not composite_available:
                    return "no_composite_extension"
                # Naming the current storage succeeds only for redirected windows.
                trap = error.CatchError()
                pixmap = view.composite_name_window_pixmap(onerror=trap)
                connection.sync()
                if trap.get_error() is not None:
                    return type(trap.get_error()).__name__
                pixmap.free()
                return "redirected"
            composite_available = connection.query_extension("Composite") is not None
            version = connection.composite_query_version() if composite_available else None
            server_info = {"composite_available": composite_available,
                           "composite_version": [version.major_version, version.minor_version] if version else None,
                           "wm_owner": selection_owner("WM_S0"),
                           "compositor_owner": selection_owner("_NET_WM_CM_S0"),
                           "before_backing_store": redirect_probe()}
            if args.disable_composite:
                assert not composite_available, server_info
            view.change_attributes(background_pixel=0, backing_store=X.Always,
                                   event_mask=X.ExposureMask | X.StructureNotifyMask | X.VisibilityChangeMask)
            root.set_input_focus(X.RevertToParent, X.CurrentTime)
            connection.sync()
            server_info["with_backing_store"] = redirect_probe()
            geometry = view.get_geometry()
            cover = root.create_window(0, 0, root.get_geometry().width, root.get_geometry().height,
                                       0, X.CopyFromParent, X.InputOutput, X.CopyFromParent,
                                       background_pixel=0, override_redirect=1)
            def pixels():
                image = view.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
                assert image.depth == 24 and len(image.data) == geometry.width * geometry.height * 4
                rgb = bytearray(image.data)
                del rgb[3::4]  # The X byte is not a visible color channel.
                return rgb
            def events():
                result = []
                while connection.pending_events():
                    result.append(connection.next_event())
                return result
            def drain():
                connection.sync()
                events()
            # Obtain a settled reference, including the real terminal text.
            view.clear_area(exposures=True)
            connection.sync()
            wait(lambda: len(set(pixels())) > 5, "rendered reference")
            time.sleep(0.15)
            reference = pixels()
            stages = []
            for action in ("unmap_map", "cover_uncover", "visibility_notification"):
                view.clear_area(exposures=True)
                connection.sync()
                wait(lambda: pixels() == reference, "reference before " + action)
                # Model the compositor returning black retained pixels. Do not
                # issue an Expose or write to the PTY while checking recovery.
                view.clear_area(exposures=False)
                connection.sync()
                black = pixels()
                assert all(b == 0 for b in black), "test did not produce a black drawable"
                drain()
                observed = []
                evidence = {}
                if action == "unmap_map":
                    view.unmap()
                    connection.sync()
                    view.map()
                elif action == "cover_uncover":
                    cover.map()
                    cover.configure(stack_mode=X.Above)
                    connection.sync()
                    covered = events()
                    covered_states = [event.state for event in covered
                                      if event.type == X.VisibilityNotify]
                    stack = [window.id for window in root.query_tree().children]
                    cover.unmap()
                    connection.sync()
                    uncovered = events()
                    uncovered_states = [event.state for event in uncovered
                                        if event.type == X.VisibilityNotify]
                    known = (X.VisibilityFullyObscured in covered_states
                             and X.VisibilityUnobscured in uncovered_states)
                    evidence = {"coverage": "confirmed" if known else "indeterminate",
                                "covered_states": covered_states, "uncovered_states": uncovered_states,
                                "bottom_to_top": stack, "cover_id": cover.id, "view_id": view.id}
                    observed.extend(covered + uncovered)
                else:
                    # Controlled case: deliver visibility, with no companion
                    # Expose, to clients that selected VisibilityChangeMask.
                    view.send_event(protocol.event.VisibilityNotify(
                        window=view, state=X.VisibilityUnobscured), event_mask=X.VisibilityChangeMask)
                connection.sync()
                until = time.monotonic() + 1
                while pixels() != reference and time.monotonic() < until:
                    time.sleep(0.01)
                final = pixels()
                observed.extend(events())
                received = [event.type for event in observed]
                states = [event.state for event in observed if event.type == X.VisibilityNotify]
                stages.append({"stage": action, "full_frame_restored": final == reference,
                               "still_black": final == black, "expose_events": received.count(X.Expose),
                               "map_events": received.count(X.MapNotify),
                               "visibility_events": received.count(X.VisibilityNotify),
                               "visibility_states": states, **evidence})
            final_info = rpc("info")
            assert (info["pid"], info["broker_pid"], info["window_id"]) == (
                final_info["pid"], final_info["broker_pid"], final_info["window_id"])
            print(json.dumps({"xvfb": server_info, "renderer": renderer, "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
                              "frame_sha256": hashlib.sha256(reference).hexdigest(), "stages": stages}), flush=True)
            remap, cover_stage, notification = stages
            assert remap["map_events"] >= 1 and remap["full_frame_restored"], remap
            assert notification["expose_events"] == 0 and notification["visibility_states"] == [
                X.VisibilityUnobscured], notification
            if args.expect == "baseline":
                assert notification["still_black"], notification
            else:
                assert notification["full_frame_restored"], notification
                if cover_stage["coverage"] == "confirmed":
                    assert cover_stage["full_frame_restored"], cover_stage
        finally:
            with contextlib.suppress(OSError):
                rpc("close")
            identities = {}
            add_owned_children(identities, [os.getpid()])
            identities.pop(server.process.pid, None)
            assert not cleanup(identities)
            connection.close()
            launcher.wait(timeout=4)


if __name__ == "__main__":
    main()
