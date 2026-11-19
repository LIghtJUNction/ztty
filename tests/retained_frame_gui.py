#!/usr/bin/env python3
"""Reproduce undefined retained-canvas publication through a real private X server.

The preload helper fills undefined newly allocated pixmaps with a visible marker.
It records actual source pixels at XCopyArea; it does not substitute presents.
This proves canvas initialization ordering, not physical compositor latency.
"""
import argparse, contextlib, ctypes, hashlib, json, os, socket, subprocess, sys, tempfile, time
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT/"tools"))
from isolated_xvfb import start_xvfb
from measure_runtime import process_stat
from measure_multiwindow import cleanup
from input_gui import CHILD
from gui_renderer import record_renderer
from repaint_gui import add_owned_children

def digest(p): return hashlib.sha256(Path(p).read_bytes()).hexdigest()
def wait(f,label,seconds=10):
    until=time.monotonic()+seconds
    while time.monotonic()<until:
        try:
            value=f()
            if value:return value
        except (FileNotFoundError,ConnectionRefusedError):pass
        time.sleep(.01)
    raise TimeoutError(label)
def main():
    from Xlib import X, display
    from PIL import Image
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument("--binary",type=Path,required=True)
    p.add_argument("--output",type=Path,required=True)
    p.add_argument("--expect",choices=("clean","reproduce"),default="clean")
    p.add_argument("--renderer",choices=("software",))
    p.add_argument("--unpoisoned",action="store_true",help="observe original pixels without initializing the pixmap")
    args=p.parse_args();binary=args.binary.resolve(strict=True)
    args.output.parent.mkdir(parents=True,exist_ok=True)
    report={"ok":False,"binary":str(binary),"binary_sha256":digest(binary),
            "scope":__doc__,"poisoned":not args.unpoisoned,"stages":[],"checks":[],
            "source_sha256":{name:digest(ROOT/name) for name in
                ("src/x11.rs","src/x11/gpu.rs","src/session.rs","src/appearance.rs",
                 "tests/retained_frame_gui.py","tests/retained_frame_trace.c","tools/gui_renderer.py")}}
    assert ctypes.CDLL(None).prctl(36,1,0,0,0)==0
    server=connection=launcher=None;identities={}
    with tempfile.TemporaryDirectory(prefix="ztty-canvas-") as temporary:
        directory=Path(temporary);logpath=directory/"stderr.log";trace=directory/"trace.jsonl"
        helper=directory/"canvas_trace.so"
        subprocess.run(["cc","-shared","-fPIC","-O2","-Wall","-Wextra","-Werror",
                        str(ROOT/"tests/retained_frame_trace.c"),"-o",str(helper),"-lX11","-ldl"],check=True)
        report["helper_sha256"]=digest(helper)
        with logpath.open("wb") as log:
            try:
                server=start_xvfb(screen="1600x1000x24",dpi=96,stderr=log)
                report["display_ownership"]=server.evidence
                connection=display.Display(server.display)
                env=dict(os.environ,DISPLAY=server.display,XMODIFIERS="@im=none",LC_ALL="C.UTF-8",
                    XDG_RUNTIME_DIR=temporary,XDG_CONFIG_HOME=str(directory/"config-home"),
                    XDG_CACHE_HOME=str(directory/"cache"),XDG_STATE_HOME=str(directory/"state"),
                    ZTTY_SOCKET="@ztty-canvas-"+directory.name,LD_PRELOAD=str(helper),ZTTY_CANVAS_TRACE=str(trace),
                    ZTTY_CANVAS_POISON="0" if args.unpoisoned else "1")
                env.pop("WAYLAND_DISPLAY",None)
                config=directory/"ztty.conf"
                config.write_text("font = DejaVu Sans Mono:size=14\nbackground = #000000\n"
                    "foreground = #ffffff\nbackground-opacity = 70\nbackground-blur = false\n"
                    "cursor-blink = false\nwindow-padding = 0\n"+
                    ("renderer = software\n" if args.renderer else ""))
                control_path,input_path=directory/"control.sock",directory/"input.bin"
                def exchange(path,request):
                    with socket.socket(socket.AF_UNIX) as channel:
                        channel.settimeout(8);channel.connect(path)
                        channel.sendall(json.dumps(request).encode()+b"\n");data=bytearray()
                        while b"\n" not in data:
                            part=channel.recv(65536)
                            if not part:raise EOFError("owned channel closed")
                            data.extend(part)
                            if len(data)>4*1024*1024:raise RuntimeError("oversized response")
                    reply=json.loads(data.split(b"\n",1)[0]);assert reply["ok"],reply;return reply
                def rpc(request):return exchange("\0"+env["ZTTY_SOCKET"][1:],request)["result"]
                def control(text=""):return exchange(str(control_path),{"output":text,"clear":False})
                def info():return rpc({"op":"info","session":"canvas"})
                launcher=subprocess.Popen([str(binary),"--name","canvas","--config",str(config),
                    "--geometry","60x12","-e","/usr/bin/python3","-c",CHILD,str(input_path),str(control_path)],
                    env=env,stdout=subprocess.DEVNULL,stderr=log,start_new_session=True)
                identities[launcher.pid]=process_stat(launcher.pid)["start_ticks"]
                wait(lambda:rpc({"op":"list"})["sessions"],"owned session")
                session=info()
                for pid in (session["pid"],session["broker_pid"]):identities[pid]=process_stat(pid)["start_ticks"]
                wait(lambda:control_path.exists() and "READY" in rpc({"op":"read","session":"canvas"})["screen"],"raw PTY")
                view=connection.create_resource_object("window",session["window_id"])
                view.set_input_focus(X.RevertToParent,X.CurrentTime);connection.sync()
                if args.renderer:
                    report["renderer_before"]=record_renderer(connection,view,args.renderer)
                def capture():
                    g=view.get_geometry();raw=view.get_image(0,0,g.width,g.height,X.ZPixmap,0xffffffff)
                    return Image.frombytes("RGBA",(g.width,g.height),raw.data,"raw","BGRA")
                def valid():
                    g=view.get_geometry()
                    return 177<=capture().getpixel((g.width-2,g.height-2))[3]<=179
                def copies():
                    return [json.loads(line) for line in trace.read_text().splitlines()
                            if line.strip() and json.loads(line)["kind"]=="copy" and json.loads(line)["dst"]==view.id]
                control("\x1b[?25l\x1b[0m\x1b[2J\x1b[HCanvas lifetime check")
                wait(valid,"initial complete translucent frame")
                report["initial_copies"]=len(copies())
                def stage(name,action,count):
                    start=len(copies())
                    for i in range(count):
                        action(i);connection.sync();control();wait(valid,name+" completed frame")
                    rows=copies()[start:]
                    report["stages"].append({"name":name,"iterations":count,"copies":len(rows),
                         "invalid_publications":[r for r in rows if r["poison_pixels"] or not 177 <= (r["last"] >> 24) <= 179],
                         "all_publications":rows})
                def resize(i):
                    view.configure(width=660+(i%3+1)*11,height=312+(i%2+1)*26)
                    connection.sync()
                    wait(lambda:info()["cols"]==61+i%3,"ConfigureNotify applied")
                stage("resize",resize,12)
                def offscreen(i):
                    view.configure(x=1700);connection.sync();control()
                    view.configure(x=0);connection.sync()
                stage("offscreen_return",offscreen,6)
                def remap(i):
                    view.unmap();connection.sync();control()
                    view.map();connection.sync()
                stage("external_unmap_map",remap,6)
                image_path=args.output.with_suffix(".png");capture().save(image_path)
                report["final_screenshot"]={"path":str(image_path),"sha256":digest(image_path)}
                invalid=sum(len(s["invalid_publications"]) for s in report["stages"])
                report["invalid_publications"]=invalid
                assert invalid>0 if args.expect=="reproduce" else invalid==0,(args.expect,invalid)
                assert info()["pid"]==session["pid"] and info()["broker_pid"]==session["broker_pid"]
                report["checks"].append("expected canvas publication result observed from real X server source pixels")
                report["checks"].append("all resizes and visibility transitions preserve the owned PTY and broker")
                report["ok"]=True
            except Exception as e:
                import traceback
                report["error"],report["traceback"]=repr(e),traceback.format_exc()
            finally:
                with contextlib.suppress(Exception):
                    if "rpc" in locals():rpc({"op":"close","session":"canvas"})
                add_owned_children(identities,[os.getpid(),*identities])
                if server is not None:identities.pop(server.process.pid,None)
                report["remaining_owned_pids"]=cleanup(identities)
                if launcher is not None:
                    with contextlib.suppress(subprocess.TimeoutExpired):launcher.wait(timeout=1)
                if connection is not None:connection.close()
                if server is not None:server.close()
                if report["remaining_owned_pids"]:report["ok"]=False
                log.flush();report["stderr_tail"]=logpath.read_text(errors="replace")[-6000:]
                if trace.exists():
                    raw=args.output.with_suffix(".trace.jsonl");raw.write_bytes(trace.read_bytes())
                    report["trace"]={"path":str(raw),"sha256":digest(raw)}
    args.output.write_text(json.dumps(report,ensure_ascii=False,indent=2)+"\n")
    print(json.dumps({"ok":report["ok"],"invalid":report.get("invalid_publications"),
                      "stages":[{k:v for k,v in s.items() if k!="all_publications"} for s in report["stages"]],
                      "error":report.get("error"),"report":str(args.output)}),flush=True)
    return 0 if report["ok"] else 1
if __name__=="__main__":raise SystemExit(main())
