#!/usr/bin/env python3
"""Verify Sixel through a real persistent PTY, MCP, and X11 window pixels.

The default uses an owned Xvfb. --display uses an existing X11/XWayland display
without focusing or moving a window. Only the newly created ztty session/window
is resized. All input is sent by its MCP operator; the GUI attaches as observer.

    python3 tests/sixel_gui.py --binary target/release/ztty
    python3 tests/sixel_gui.py --binary target/release/ztty --display "$DISPLAY"

Python Xlib and Pillow are test dependencies, not production dependencies.
"""
import argparse
import base64
import contextlib
import ctypes
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import select
import shutil
import signal
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/"tools"))
from isolated_xvfb import display_endpoints, start_xvfb


ESC = b"\x1b"
PAD = 24
CHILD = r'''import base64,json,os,sys,tty
from pathlib import Path
root=Path(sys.argv[1]); tty.setraw(0)
replies=open(root/'replies.bin','wb',buffering=0)
def write(data):
 while data:
  data=data[os.write(1,data):]
write(b'\x1b]2;ztty Sixel acceptance\x07\x1b[?25l\x1b[0m\x1b[2J\x1b[H')
(root/'ready').write_text('ready')
command=None
while True:
 data=os.read(0,65536)
 if not data: break
 for byte in data:
  if byte==30: command=bytearray()
  elif command is None: replies.write(bytes([byte]))
  elif byte==10:
   request=json.loads(command); command=None
   payload=base64.b64decode(request['data'],validate=True)
   write(payload)
   temp=root/'state.tmp';temp.write_text(json.dumps({'sequence':request['sequence'],'bytes':len(payload)}));temp.replace(root/'state.json')
  else:
   command.append(byte)
   if len(command)>262144: raise ValueError('test command too large')
'''


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def wait_for(function, label, timeout=8):
    deadline = time.monotonic() + timeout
    last_error = None
    while time.monotonic() < deadline:
        try:
            value = function()
            if value:
                return value
        except (FileNotFoundError, ConnectionRefusedError, json.JSONDecodeError) as error:
            last_error = error
        time.sleep(0.025)
    raise TimeoutError(f"{label}: {last_error!r}")


class Mcp:
    def __init__(self, binary, environment, directory, log):
        self.process = subprocess.Popen([str(binary), "mcp", "--transport", "stdio"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=log,
            cwd=directory, env=environment, start_new_session=True)
        self.buffer = bytearray()
        self.identifier = 0
        try:
            initialized = self.request("initialize", {"protocolVersion":"2025-11-25",
                "capabilities":{}, "clientInfo":{"name":"ztty-sixel-gui", "version":"1"}})
            assert initialized["protocolVersion"] == "2025-11-25", initialized
            self.send({"jsonrpc":"2.0", "method":"notifications/initialized"})
        except BaseException:
            stop_process(self.process)
            self.process.stdin.close()
            self.process.stdout.close()
            raise

    def send(self, value):
        self.process.stdin.write(json.dumps(value, separators=(",", ":")).encode()+b"\n")
        self.process.stdin.flush()

    def request(self, method, params):
        self.identifier += 1
        self.send({"jsonrpc":"2.0", "id":self.identifier, "method":method, "params":params})
        deadline = time.monotonic() + 8
        while time.monotonic() < deadline:
            if b"\n" in self.buffer:
                line, _, rest = self.buffer.partition(b"\n")
                self.buffer = bytearray(rest)
                response = json.loads(line)
                assert response.get("id") == self.identifier and "result" in response, response
                return response["result"]
            if not select.select([self.process.stdout], [], [], max(0, deadline-time.monotonic()))[0]:
                break
            data = os.read(self.process.stdout.fileno(), 65536)
            if not data:
                raise RuntimeError(f"MCP exited: {self.process.poll()}")
            self.buffer.extend(data)
            if len(self.buffer) > 2*1024*1024:
                raise RuntimeError("oversized MCP response")
        raise TimeoutError(f"MCP {method}")

    def tool(self, name, arguments):
        result = self.request("tools/call", {"name":name, "arguments":arguments})
        assert not result.get("isError"), result
        return result["structuredContent"]

    def close(self):
        if self.process.poll() is None:
            self.process.stdin.close()
            self.process.wait(timeout=4)
        self.process.stdout.close()


def stop_process(process):
    if process is None or process.poll() is not None:
        return
    process.terminate()
    try:
        process.wait(timeout=4)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=4)


def process_identity(pid):
    # A saved start time prevents killing an unrelated process after PID reuse.
    try:
        stat = Path(f"/proc/{pid}/stat").read_text()
        return stat[stat.rfind(")")+2:].split()[19]
    except FileNotFoundError:
        return None


def stop_broker(pid, started, socket_name):
    if not pid or started is None or process_identity(pid) != started:
        return
    environment = Path(f"/proc/{pid}/environ").read_bytes().split(b"\0")
    if ("ZTTY_SOCKET="+socket_name).encode() not in environment:
        raise RuntimeError("broker identity no longer matches owned socket")
    os.kill(pid, signal.SIGTERM)
    deadline = time.monotonic() + 3
    while time.monotonic() < deadline and process_identity(pid) == started:
        with contextlib.suppress(ChildProcessError):
            if os.waitpid(pid, os.WNOHANG)[0]:
                return
        time.sleep(0.025)
    if process_identity(pid) == started:
        os.kill(pid, signal.SIGKILL)
    with contextlib.suppress(ChildProcessError):
        os.waitpid(pid, 0)


def rle(row):
    output = bytearray()
    index = 0
    while index < len(row):
        end = index + 1
        while end < len(row) and row[end] == row[index]:
            end += 1
        count = end-index
        if count >= 3:
            output.extend(b"!"+str(count).encode()+bytes([row[index]]))
        else:
            output.extend(row[index:end])
        index = end
    return output


def sixel(width, height, indices, palette, transparent=True):
    body = bytearray(f'"1;1;{width};{height}'.encode())
    for index, rgb in palette.items():
        body.extend((f"#{index};2;"+";".join(map(str, rgb))).encode())
    for y in range(0, height, 6):
        for index in palette:
            body.extend(f"#{index}".encode())
            row = bytearray(63+sum(1 << bit for bit in range(min(6, height-y))
                if indices[(y+bit)*width+x] == index) for x in range(width))
            body.extend(rle(row))
            body.extend(b"$")
        if y+6 < height:
            body.extend(b"-")
    return ESC+b"P7;"+(b"1" if transparent else b"0")+b"q"+body+ESC+b"\\"


def at(column, row):
    return ESC+f"[{row+1};{column+1}H".encode()


def truecolor_image(window, connection):
    from Xlib import X
    from PIL import Image
    geometry = window.get_geometry()
    reply = window.get_image(0, 0, geometry.width, geometry.height, X.ZPixmap, 0xffffffff)
    setup = connection.display.info
    fmt = next(item for item in setup.pixmap_formats if item.depth == reply.depth)
    visual = next(visual for depth in connection.screen().allowed_depths
                  for visual in depth.visuals if visual.visual_id == reply.visual)
    masks = (visual.red_mask, visual.green_mask, visual.blue_mask)
    if masks != (0xff0000, 0xff00, 0xff) or fmt.bits_per_pixel not in (24, 32):
        raise RuntimeError(f"unsupported exact-pixel test visual: {masks}, {fmt.bits_per_pixel} bpp")
    stride = ((geometry.width*fmt.bits_per_pixel+fmt.scanline_pad-1)//fmt.scanline_pad)*fmt.scanline_pad//8
    if fmt.bits_per_pixel == 32:
        mode = "BGRX" if setup.image_byte_order == X.LSBFirst else "XRGB"
    else:
        mode = "BGR" if setup.image_byte_order == X.LSBFirst else "RGB"
    return Image.frombytes("RGB", (geometry.width, geometry.height), reply.data,
                           "raw", mode, stride, 1)


def existing_desktop(display_name):
    result = {"kind":"existing X11 display", "display":display_name}
    if (display_name == os.environ.get("DISPLAY") and
            os.environ.get("HYPRLAND_INSTANCE_SIGNATURE") and shutil.which("hyprctl")):
        probe = subprocess.run(["hyprctl", "version", "-j"], capture_output=True,
                               text=True, timeout=3, check=True)
        result.update(kind="Hyprland/XWayland", hyprland_version=json.loads(probe.stdout).get("version"))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--display", help="Existing X11/XWayland display; omitted means an owned Xvfb")
    parser.add_argument("--output", type=Path, default=Path("artifacts/sixel-gui.json"))
    parser.add_argument("--screenshot", type=Path, default=Path("artifacts/sixel-gui.png"))
    parser.add_argument("--source-root", type=Path, help="Source checkout for the tested binary")
    args = parser.parse_args()
    if platform.system() != "Linux":
        parser.error("this acceptance harness requires Linux X11/XWayland")
    def interrupted(signum, _frame):
        raise RuntimeError(f"acceptance interrupted by signal {signum}")
    signal.signal(signal.SIGTERM, interrupted)
    from Xlib import X, display
    binary = args.binary.resolve(strict=True)
    source_root = args.source_root.resolve() if args.source_root else binary.parents[2]
    report = {"schema":1, "ok":False, "timestamp_utc":datetime.now(timezone.utc).isoformat(),
        "platform":platform.platform(), "binary":str(binary), "binary_sha256":sha256(binary.read_bytes()),
        "test_sha256":sha256(Path(__file__).read_bytes()), "checks":[],
        "display_helper_sha256":sha256((Path(__file__).resolve().parents[1]/"tools/isolated_xvfb.py").read_bytes()),
        "source_snapshot_sha256":{},
        "source_snapshot_note":"Checkout hashes at test start; match binary identity to the build log independently.",
        "input_transport":"MCP stdio operator to real PTY",
        "attachment":"observe", "screenshot_method":"XGetImage of owned native window",
        "limitations":["Xvfb verifies X11 rendering, not native Wayland or GPU scanout.",
            "Existing-display screenshots capture X11 client pixels, not compositor scanout timing.",
            "No macOS or Windows execution is claimed."]}
    try:
        report["source_git"] = {
            "commit":subprocess.check_output(["git","rev-parse","HEAD"],cwd=source_root,text=True,timeout=3).strip(),
            "tracked_changes":subprocess.check_output(["git","status","--porcelain","--untracked-files=no"],
                cwd=source_root,text=True,timeout=3).splitlines()}
    except (OSError, subprocess.SubprocessError) as error:
        report["source_git_error"] = repr(error)
    for name in ["crates/zgraphics/src/lib.rs", "crates/zgraphics/src/sixel.rs",
                 "crates/zterm/src/terminal.rs", "src/session.rs", "src/x11.rs"]:
        path = source_root/name
        if path.is_file():
            report["source_snapshot_sha256"][name] = sha256(path.read_bytes())
    connection = xvfb = client = attach = None
    broker_pid = broker_started = uri = None
    socket_name = ""
    cleanup_errors = []
    # The broker deliberately survives its MCP parent. Reap our own descendants.
    ctypes.CDLL(None).prctl(36, 1, 0, 0, 0)
    with tempfile.TemporaryDirectory(prefix="ztty-sixel-gui-") as temporary:
        directory = Path(temporary)
        with (directory/"stderr.log").open("wb") as log:
            try:
                if args.display:
                    display_name = args.display
                    report["desktop"] = existing_desktop(display_name)
                    number = re.search(r":([0-9]+)(?:\.[0-9]+)?$",display_name)
                    if number:
                        report["desktop"]["endpoints"] = display_endpoints(int(number[1]))
                else:
                    xvfb=start_xvfb(screen="1600x1000x24",stderr=log)
                    display_name=xvfb.display
                    report["desktop"] = {"kind":"private Xvfb/X11", **xvfb.evidence}
                connection = display.Display(display_name)
                socket_name = "@"+directory.name
                config_root = directory/"config"
                (config_root/"ztty").mkdir(parents=True)
                (config_root/"ztty/ztty.conf").write_text("font = monospace:size=12\n"
                    "background = #000000\nforeground = #e6e6e6\nbackground-opacity = 100\n"
                    "background-blur = false\ncursor-blink = false\nwindow-padding = 24\n")
                environment = dict(os.environ, DISPLAY=display_name, XDG_CONFIG_HOME=str(config_root),
                    XDG_RUNTIME_DIR=temporary, ZTTY_SOCKET=socket_name, XMODIFIERS="@im=none", LC_ALL="C.UTF-8")
                environment.pop("WAYLAND_DISPLAY", None)
                client = Mcp(binary, environment, directory, log)
                created = client.tool("session_create", {"name":"sixel-native-pixels", "cols":80,
                    "rows":30, "history":0, "command":[sys.executable,"-c",CHILD,temporary]})
                uri = created["uri"]
                broker_pid = created["broker_pid"]
                broker_started = process_identity(broker_pid)
                owner = created["input_owner"]
                report["session"] = {"uri":uri, "broker_pid":broker_pid, "pty_pid":created["pid"]}
                wait_for(lambda:(directory/"ready").exists(), "PTY child readiness")
                attach = subprocess.Popen([str(binary),"attach","--observe",uri], env=environment,
                    cwd=directory, stdout=subprocess.DEVNULL, stderr=log, start_new_session=True)

                def info():
                    return client.tool("session_read", {"session":uri, "max_bytes":1})

                view = wait_for(lambda:(value if value.get("window_id") and value.get("attached") else None)
                    if (value:=info()) else None, "observer window")
                window = connection.create_resource_object("window",view["window_id"])
                wait_for(lambda:window.get_attributes().map_state == X.IsViewable, "mapped observer")
                assert view["input_owner"] == owner, "observer took operator ownership"
                focus = connection.get_input_focus().focus
                assert getattr(focus,"id",focus) != window.id, "observer window stole X11 focus"
                user_time = window.get_full_property(connection.intern_atom("_NET_WM_USER_TIME"), X.AnyPropertyType)
                assert user_time is not None and list(user_time.value)==[0], "missing nonfocus activation hint"
                report["checks"].append({"name":"persistent_pty_observer_without_focus", "passed":True})
                if report["desktop"]["kind"] == "Hyprland/XWayland":
                    clients = json.loads(subprocess.check_output(["hyprctl","clients","-j"],timeout=3))
                    owned = next((item for item in clients if item.get("pid")==broker_pid),None)
                    assert owned and owned.get("xwayland"), "owned XWayland window not registered by compositor"
                    active = json.loads(subprocess.check_output(["hyprctl","activewindow","-j"],timeout=3))
                    assert active.get("pid") != broker_pid, "observer stole compositor focus"
                    report["desktop"]["owned_window_xwayland"] = True

                sequence = 0
                def emit(data):
                    nonlocal sequence
                    sequence += 1
                    command = "\x1e"+json.dumps({"sequence":sequence,"data":base64.b64encode(data).decode()},separators=(",",":"))+"\n"
                    client.tool("session_write", {"session":uri,"data":command})
                    wait_for(lambda:json.loads((directory/"state.json").read_text())["sequence"]==sequence,
                             "PTY emitted test data")

                emit(ESC+b"[16t"+ESC+b"[c")
                def geometry_reply():
                    data = (directory/"replies.bin").read_bytes()
                    match = re.search(rb"\x1b\[6;([0-9]+);([0-9]+)t",data)
                    return match if match and b"\x1b[?62;4c" in data else None
                measured = wait_for(geometry_reply,"actual cell geometry and Sixel DA1")
                ch,cw = map(int,measured.groups())
                assert cw>0 and ch>0
                origin = (PAD+3*cw,PAD+4*ch)
                report["cell_pixels"] = [cw,ch]
                report["primary_da"] = [62,4]

                def exact_pixels(name, width, height, expected):
                    last = None
                    def matches():
                        nonlocal last
                        image = truecolor_image(window,connection)
                        x,y = origin
                        if image.width < x+width or image.height < y+height:
                            raise AssertionError(f"owned viewport {image.size} clips required test region {(x,y,width,height)}")
                        last = image.crop((x,y,x+width,y+height)).tobytes()
                        return image if last == expected else None
                    try:
                        result=wait_for(matches,name)
                    except TimeoutError as error:
                        raise AssertionError(f"{name}: expected {sha256(expected)}, observed {sha256(last or b'')}") from error
                    report["checks"].append({"name":name,"passed":True,"pixels":width*height,"rgb_sha256":sha256(expected)})
                    return result

                width,height = 320,180
                palette = {0:(0,0,0),1:(20,40,80),2:(0,100,0),3:(100,40,0)}
                colors = {key:bytes((value*255+50)//100 for value in rgb) for key,rgb in palette.items()}
                indices=[0 if x%80<8 or y%60<8 else 1+(x//80+y//60)%3 for y in range(height) for x in range(width)]
                demo=sixel(width,height,indices,palette)
                expected=b"".join(colors[index] for index in indices)
                clear=ESC+b"[0m"+ESC+b"[2J"+ESC+b"[H"+ESC+b"[?25l"
                demo_frame=clear+at(3,1)+b"Sixel / native pixels"+at(3,4)+demo
                emit(demo_frame)
                exact_pixels("rgb_blocks_320x180",width,height,expected)
                before=window.get_geometry()
                client.tool("session_resize",{"session":uri,"cols":90,"rows":32})
                wait_for(lambda:info()["cols"]==90 and info()["rows"]==32,"operator PTY resize")
                exact_pixels("image_preserved_after_operator_resize",width,height,expected)
                after=window.get_geometry()
                if not args.display:
                    assert (before.width,before.height)!=(after.width,after.height), "owned Xvfb window did not resize"
                report["resize"]={"before":[before.width,before.height],"after":[after.width,after.height],"pty":[90,32]}

                w,h=64,36
                green=sixel(w,h,[2]*(w*h),{2:(0,100,0)})
                holes=[1 if (x//16+y//12)%2 else -1 for y in range(h) for x in range(w)]
                background=bytes((23,51,77))
                emit(clear+ESC+b"[48;2;23;51;77m"+ESC+b"[2J"+at(3,4)+green+
                     at(3,4)+sixel(w,h,holes,{1:(100,0,0)},True))
                red_or_green=b"".join(bytes((255,0,0)) if index==1 else bytes((0,255,0)) for index in holes)
                exact_pixels("transparent_sixel_preserves_underlying_image",w,h,red_or_green)
                emit(at(3,4)+sixel(w,h,holes,{1:(100,0,0)},False))
                red_or_bg=b"".join(bytes((255,0,0)) if index==1 else background for index in holes)
                exact_pixels("opaque_sixel_uses_current_background",w,h,red_or_bg)
                emit(clear)
                exact_pixels("erase_display_releases_visible_sixels",width,height,bytes(width*height*3))

                emit(at(3,4)+ESC+b'P7q"1;1;8193;1#1;2;100;0;0~'+ESC+b"\\"+at(3,1)+b"Recovered after rejected Sixel")
                wait_for(lambda:"Recovered after rejected Sixel" in info()["screen"],"text after oversized Sixel")
                exact_pixels("oversized_sixel_rejected_and_text_recovers",width,height,bytes(width*height*3))
                emit(clear+at(3,4)+ESC+b"P7q#1;2;100;0;0!320~")
                exact_pixels("unterminated_sixel_is_not_presented",width,height,bytes(width*height*3))
                emit(b"\x18"+at(3,1)+b"Recovered after cancelled Sixel")
                wait_for(lambda:"Recovered after cancelled Sixel" in info()["screen"],"text after Sixel cancellation")
                exact_pixels("cancelled_sixel_is_not_presented",width,height,bytes(width*height*3))

                footer = 4+(height+ch-1)//ch+2
                assert footer+2 < info()["rows"], "owned viewport has insufficient footer space"
                emit(demo_frame+at(3,footer)+b"MCP operator / observer attachment"+
                     at(3,footer+2)+b"Rust / no_std / zterm / zgraphics")
                screenshot=exact_pixels("valid_image_after_rejection_and_cancellation",width,height,expected)
                assert info()["input_owner"] == owner, "GUI changed input ownership during rendering"
                focus=connection.get_input_focus().focus
                assert getattr(focus,"id",focus)!=window.id, "observer became focused"
                args.screenshot.parent.mkdir(parents=True,exist_ok=True)
                screenshot.save(args.screenshot)
                report["screenshot"]={"path":str(args.screenshot.resolve()),"width":screenshot.width,
                    "height":screenshot.height,"sha256":sha256(args.screenshot.read_bytes())}
                report["ok"]=True
            except BaseException as error:
                report["error"]=repr(error)
                if connection is not None and "window" in locals():
                    with contextlib.suppress(Exception):
                        args.screenshot.parent.mkdir(parents=True,exist_ok=True)
                        truecolor_image(window,connection).save(args.screenshot)
                        report["failure_screenshot"]=str(args.screenshot.resolve())
            finally:
                if client is not None and uri is not None:
                    try:
                        client.tool("session_close",{"session":uri})
                    except Exception as error:
                        cleanup_errors.append("session_close: "+repr(error))
                if attach is not None:
                    with contextlib.suppress(subprocess.TimeoutExpired):
                        attach.wait(timeout=4)
                    stop_process(attach)
                if client is not None:
                    try:
                        client.close()
                    except Exception as error:
                        cleanup_errors.append("MCP close: "+repr(error))
                    stop_process(client.process)
                if connection is not None:
                    connection.close()
                try:
                    stop_broker(broker_pid,broker_started,socket_name)
                except Exception as error:
                    cleanup_errors.append("owned broker: "+repr(error))
                if xvfb is not None:
                    try:
                        xvfb.close()
                    except Exception as error:
                        cleanup_errors.append("owned Xvfb: "+repr(error))
        report["stderr_tail"]=(directory/"stderr.log").read_text(errors="replace")[-4000:]
    report["cleanup_errors"]=cleanup_errors
    if cleanup_errors:
        report["ok"]=False
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,indent=2,ensure_ascii=False)+"\n")
    print(json.dumps({"ok":report["ok"],"checks":len(report["checks"]),"output":str(args.output),
        "screenshot":str(args.screenshot),"error":report.get("error"),"cleanup_errors":cleanup_errors},ensure_ascii=False))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
