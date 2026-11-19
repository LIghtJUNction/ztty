#!/usr/bin/env python3
"""Check complete text, copy, blink, opacity and font reload on owned X11 pixels."""
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

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "tools"))
from isolated_xvfb import start_xvfb
from gui_renderer import record_renderer
from measure_runtime import process_stat
from measure_multiwindow import cleanup
from input_gui import CHILD
from repaint_gui import add_owned_children, switches

def digest(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()

def wait(predicate, label, seconds=10):
    until = time.monotonic() + seconds
    while time.monotonic() < until:
        try:
            result = predicate()
            if result:
                return result
        except (FileNotFoundError, ConnectionRefusedError):
            pass
        time.sleep(0.01)
    raise TimeoutError(label)

def main():
    from Xlib import X, XK, display
    from Xlib.ext import xtest
    from PIL import Image
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--renderer", choices=("gpu", "software"), default="gpu")
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--screenshots", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    args.screenshots.mkdir(parents=True, exist_ok=True)
    report = {"ok":False, "binary":str(binary), "binary_sha256":digest(binary),
              "scope":"Owned PTY, XTEST, private clipboard and X11 pixel readback; not physical display latency",
              "checks":[], "screenshots":{}, "source_sha256":{}}
    for name in ("src/x11.rs","src/x11/gpu.rs","src/session.rs","src/appearance.rs",
                 "src/gpu/mod.rs","src/gpu/font.rs","src/gpu/grid.rs","src/text_blink.rs",
                 "native/font_bridge.c","tests/text_gpu_gui.py","tools/gui_renderer.py"):
        report["source_sha256"][name] = digest(ROOT/name)
    assert ctypes.CDLL(None).prctl(36,1,0,0,0)==0
    server = connection = launcher = None
    identities = {}
    with tempfile.TemporaryDirectory(prefix="ztty-text-gpu-") as temporary:
        directory = Path(temporary)
        logpath = directory/"stderr.log"
        with logpath.open("wb") as log:
            try:
                server = start_xvfb(screen="1600x1000x24",dpi=96,stderr=log)
                report["display_ownership"] = server.evidence
                connection = display.Display(server.display)
                env = dict(os.environ,DISPLAY=server.display,XMODIFIERS="@im=none",LC_ALL="C.UTF-8",
                           XDG_RUNTIME_DIR=temporary,XDG_CONFIG_HOME=str(directory/"config-home"),
                           XDG_CACHE_HOME=str(directory/"cache"),XDG_STATE_HOME=str(directory/"state"),
                           ZTTY_SOCKET="@ztty-text-gpu-"+directory.name)
                env.pop("WAYLAND_DISPLAY",None)
                env.pop("LD_PRELOAD",None)
                config = directory/"ztty.conf"
                base = ("font = DejaVu Sans Mono:size=14\nbackground = #000000\n"
                        "foreground = #ffffff\nbackground-opacity = 70\nbackground-blur = false\n"
                        "cursor-blink = false\nwindow-padding = 0\nrenderer = "+args.renderer+"\n")
                config.write_text(base)
                input_path, control_path = directory/"input.bin",directory/"control.sock"

                def exchange(path, request):
                    with socket.socket(socket.AF_UNIX) as channel:
                        channel.settimeout(8)
                        channel.connect(path)
                        channel.sendall(json.dumps(request).encode()+b"\n")
                        data=bytearray()
                        while b"\n" not in data:
                            chunk=channel.recv(65536)
                            if not chunk: raise EOFError("test channel closed")
                            data.extend(chunk)
                            if len(data)>4*1024*1024: raise RuntimeError("oversized test response")
                    reply=json.loads(data.split(b"\n",1)[0])
                    assert reply["ok"],reply
                    return reply

                def rpc(request):
                    return exchange("\0"+env["ZTTY_SOCKET"][1:],request)["result"]

                def control(text=""):
                    return exchange(str(control_path),{"output":text,"clear":False})

                def info():
                    return rpc({"op":"info","session":"text-gpu"})

                launcher=subprocess.Popen([str(binary),"--name","text-gpu","--config",str(config),
                    "--geometry","60x12","-e","/usr/bin/python3","-c",CHILD,str(input_path),str(control_path)],
                    env=env,stdout=subprocess.DEVNULL,stderr=log,start_new_session=True)
                identities[launcher.pid]=process_stat(launcher.pid)["start_ticks"]
                wait(lambda: rpc({"op":"list"})["sessions"],"owned text session")
                session=info()
                for pid in (session["pid"],session["broker_pid"]):
                    identities[pid]=process_stat(pid)["start_ticks"]
                wait(lambda: control_path.exists() and "READY" in rpc({"op":"read","session":"text-gpu"})["screen"],
                     "raw PTY ready")
                view=connection.create_resource_object("window",session["window_id"])
                view.set_input_focus(X.RevertToParent,X.CurrentTime)
                connection.sync()
                report["renderer_before"]=record_renderer(connection,view,args.renderer)
                geometry=view.get_geometry()
                cw,ch=geometry.width//session["cols"],geometry.height//session["rows"]
                report["grid"]={"cell_width":cw,"cell_height":ch,"depth":geometry.depth}

                def capture(label=None):
                    geo=view.get_geometry()
                    raw=view.get_image(0,0,geo.width,geo.height,X.ZPixmap,0xffffffff)
                    picture=Image.frombytes("RGBA",(geo.width,geo.height),raw.data,"raw","BGRA")
                    if label:
                        path=args.screenshots/(label+".png")
                        picture.save(path)
                        report["screenshots"][label]={"path":str(path),"sha256":digest(path)}
                    return picture

                def key(name,down):
                    code=connection.keysym_to_keycode(XK.string_to_keysym(name))
                    assert code,name
                    xtest.fake_input(connection,X.KeyPress if down else X.KeyRelease,code)

                def copy_selection():
                    requestor=connection.screen().root.create_window(0,0,1,1,0,X.CopyFromParent,
                        X.InputOutput,X.CopyFromParent,event_mask=X.PropertyChangeMask)
                    clipboard=connection.intern_atom("CLIPBOARD")
                    utf8=connection.intern_atom("UTF8_STRING")
                    prop=connection.intern_atom("ZTTY_TEXT_TEST_SELECTION")
                    try:
                        wait(lambda: getattr(connection.get_selection_owner(clipboard),"id",0)==view.id,
                             "copy action acquired the private clipboard")
                        requestor.convert_selection(clipboard,utf8,prop,X.CurrentTime)
                        connection.flush()
                        def receive():
                            while connection.pending_events():
                                event=connection.next_event()
                                if event.type==X.SelectionNotify and event.requestor.id==requestor.id:
                                    assert event.property==prop
                                    return bytes(requestor.get_full_property(prop,utf8).value)
                            return None
                        return wait(receive,"complete private clipboard selection")
                    finally:
                        requestor.destroy()
                        connection.flush()

                text="A 👩🏽‍💻 🇨🇳 e\u0301 Z"
                control("\x1b[?25l\x1b[0m\x1b[2J\x1b[H"+text)
                wait(lambda: text in rpc({"op":"read","session":"text-gpu"})["screen"],"complete grapheme model")
                wait(lambda: any(r!=g or g!=b for r,g,b,a in capture().crop((2*cw,0,4*cw,ch)).getdata()),
                     "colored complete ZWJ glyph")
                picture=capture("complete-graphemes")
                assert any(r!=g or g!=b for r,g,b,a in picture.crop((5*cw,0,7*cw,ch)).getdata())
                assert geometry.depth==32,geometry.depth
                alpha=picture.getpixel((picture.width-2,picture.height-2))[3]
                assert 177<=alpha<=179,alpha
                report["background_alpha"]=alpha
                report["checks"].append("complete colored ZWJ and flag clusters reach real pixels in two-cell allocations")
                report["checks"].append("the ARGB window preserves configured 70 percent background alpha")

                xtest.fake_input(connection,X.MotionNotify,x=cw//2,y=ch//2)
                xtest.fake_input(connection,X.ButtonPress,1)
                xtest.fake_input(connection,X.MotionNotify,x=10*cw+cw//2,y=ch//2)
                xtest.fake_input(connection,X.ButtonRelease,1)
                connection.sync()
                key("Control_L",True);key("Shift_L",True);key("c",True);key("c",False)
                key("Shift_L",False);key("Control_L",False);connection.sync()
                copied=copy_selection().decode("utf-8")
                assert copied==text,(copied,text)
                report["copied_utf8_sha256"]=hashlib.sha256(copied.encode()).hexdigest()
                report["checks"].append("real drag and Ctrl+Shift+C copy the exact original ZWJ, flag and combining sequence")

                # A new click removes the selection without writing application input.
                xtest.fake_input(connection,X.MotionNotify,x=20*cw,y=2*ch)
                xtest.fake_input(connection,X.ButtonPress,1)
                xtest.fake_input(connection,X.ButtonRelease,1)
                connection.sync()
                control("\x1b[0m\x1b[2J\x1b[H\x1b[5mSLOW\x1b[0m \x1b[6mFAST\x1b[0m STILL")
                regions=[(0,0,4*cw,ch),(5*cw,0,9*cw,ch),(10*cw,0,15*cw,ch)]
                def lit(picture,region):
                    return any(r>20 or g>20 or b>20 for r,g,b,a in picture.crop(region).getdata())
                wait(lambda: all(lit(capture(),r) for r in regions),"initial blink and static text")
                start=time.monotonic();samples=[]
                while time.monotonic()-start<1.35:
                    picture=capture()
                    samples.append({"ms":round((time.monotonic()-start)*1000,3),
                                    "visible":[lit(picture,region) for region in regions]})
                    time.sleep(0.02)
                assert {tuple(x["visible"][:2]) for x in samples}=={(True,True),(True,False),(False,True),(False,False)}
                assert all(x["visible"][2] for x in samples),"ordinary text blinked"
                report["blink_samples"]=samples
                report["checks"].append("SGR 5 and SGR 6 change actual pixels independently while ordinary text remains visible")

                sink=connection.screen().root.create_window(0,900,2,2,0,X.CopyFromParent,
                    X.InputOutput,X.CopyFromParent)
                sink.map();sink.set_input_focus(X.RevertToParent,X.CurrentTime);connection.sync()
                wait(lambda: all(lit(capture(),r) for r in regions),"unfocused text restored")
                time.sleep(0.05)
                before=switches(session["broker_pid"])
                time.sleep(0.55)
                after=switches(session["broker_pid"])
                delta={k:after[k]-before[k] for k in before}
                assert delta["voluntary_ctxt_switches"]<=4,delta
                assert all(lit(capture(),r) for r in regions)
                report["unfocused_idle"]={"seconds":0.55,"context_switch_delta":delta}
                report["checks"].append("unfocused blinking text stays visible without recurring repaint wakeups")
                view.set_input_focus(X.RevertToParent,X.CurrentTime);sink.destroy();connection.sync()

                # Reload the face on the existing EGL surface, then switch the
                # renderer twice. The broker and PTY identities must survive.
                config.write_text(base.replace("size=14","size=20"))
                rpc({"op":"action","session":"text-gpu","actor":"local","action":"reload"})
                wait(lambda: info()["cols"]<session["cols"],"larger live font")
                control("\x1b[0m\x1b[2J\x1b[H\x1b[3mItalic office\x1b[0m office")
                wait(lambda: any(r>20 for r,g,b,a in capture().getdata()),"font reload pixels")
                record_renderer(connection,view,args.renderer)
                capture("font-reload")
                assert info()["pid"]==session["pid"] and info()["broker_pid"]==session["broker_pid"]
                report["checks"].append("font reload changes geometry while retaining the same window, broker and PTY")
                font_boxes={}
                for renderer in (("software","gpu") if args.renderer=="gpu" else ("gpu","software")):
                    config.write_text(base.replace("renderer = "+args.renderer,"renderer = "+renderer))
                    rpc({"op":"action","session":"text-gpu","actor":"local","action":"reload"})
                    wait(lambda: record_renderer(connection,view)["actual"]==renderer,"renderer mode "+renderer)
                    wait(lambda: info()["cols"]==session["cols"],"original grid restored")
                    patch=(160,30,45) if renderer=="software" else (30,160,45)
                    control("\x1b[0m\x1b[2J\x1b[HHMWH "+renderer+" 👩🏽‍💻"
                            +"\x1b[2;50H\x1b[48;2;"+";".join(map(str,patch))+"m \x1b[0m")
                    wait(lambda: capture().getpixel((49*cw+1,ch+1))[:3]==patch,"new backend frame pixels")
                    box=capture().convert("RGB").crop((0,0,4*cw,ch)).getbbox()
                    assert box and box[3]-box[1]>8,(renderer,box)
                    font_boxes[renderer]=box
                gpu_box,software_box=font_boxes["gpu"],font_boxes["software"]
                assert abs((gpu_box[3]-gpu_box[1])-(software_box[3]-software_box[1]))<=2,font_boxes
                assert abs(gpu_box[1]-software_box[1])<=2,font_boxes
                report["backend_font_boxes"]=font_boxes
                report["checks"].append("GPU and Xft use the same display-resolved font size and baseline")
                assert info()["pid"]==session["pid"] and info()["broker_pid"]==session["broker_pid"]
                report["renderer_after"]=record_renderer(connection,view,args.renderer)
                report["checks"].append("live GPU/software switching preserves the PTY and returns to the requested backend")
                capture("completed")
                assert digest(binary)==report["binary_sha256"]
                report["ok"]=True
            except Exception as error:
                import traceback
                report["error"],report["traceback"]=repr(error),traceback.format_exc()
                if "capture" in locals():
                    with contextlib.suppress(Exception):capture("failure")
            finally:
                with contextlib.suppress(Exception):
                    if "rpc" in locals():rpc({"op":"close","session":"text-gpu"})
                add_owned_children(identities,[os.getpid(),*identities])
                if server is not None:identities.pop(server.process.pid,None)
                report["remaining_owned_pids"]=cleanup(identities)
                if launcher is not None:
                    with contextlib.suppress(subprocess.TimeoutExpired):launcher.wait(timeout=1)
                if connection is not None:connection.close()
                if server is not None:
                    number=server.number;server.close()
                    report["display_cleanup"]={"exit_code":server.process.returncode,
                        "pid_lock_exists":Path(f"/tmp/.X{number}-lock").exists(),
                        "socket_exists":Path(f"/tmp/.X11-unix/X{number}").exists(),
                        "lease_exists":Path(f"/tmp/ztty-xvfb-{number}.lease").exists()}
                if report["remaining_owned_pids"]:report["ok"]=False
                log.flush()
                report["stderr_tail"]=logpath.read_text(errors="replace")[-12000:]
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,ensure_ascii=False,indent=2)+"\n")
    print(json.dumps({"ok":report["ok"],"checks":len(report["checks"]),"error":report.get("error"),
                      "remaining":report.get("remaining_owned_pids"),"report":str(args.output)}),flush=True)
    return 0 if report["ok"] else 1

if __name__=="__main__":
    raise SystemExit(main())
