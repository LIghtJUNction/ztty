#!/usr/bin/env python3
"""Kitty extensions through an owned raw PTY, actual X11 pixels and host I/O."""
import argparse
import base64
import contextlib
import ctypes
import hashlib
import json
import mmap
import os
from pathlib import Path
import tempfile
import time
import zlib

from gui_smoke import Gui, ROOT
from gui_renderer import record_renderer
from graphics_smoke import BG, command, expect_pixel, pixel, png

RED=(255,0,0,255)
GREEN=(0,255,0,255)
BLUE=(0,0,255,255)
PH="\U0010eeee"
def at(col,row):return f"\x1b[{row+1};{col+1}H"

def image(gui,ident,color=RED,**keys):
    fields={"a":"T","i":ident,"p":1,"f":32,"s":1,"v":1,"c":1,"r":1,"C":1,"q":2,**keys}
    gui.emit(command(",".join(f"{k}={v}" for k,v in fields.items()),bytes(color)))

def check_reply(gui,control,data,expected):
    gui.clear_input()
    gui.emit(command(control,data))
    gui.expect_input(expected)

def file_and_symlink(gui,root):
    gui.reset()
    path=root/"source.rgba";path.write_bytes(b"prefix"+bytes(RED)+b"suffix")
    alias=root/"image-link";alias.symlink_to(path)
    gui.emit(at(3,3))
    check_reply(gui,"a=T,t=f,f=32,s=1,v=1,c=1,r=1,i=701,C=1,O=6,S=4",os.fsencode(alias),b"\x1b_Gi=701;OK\x1b\\")
    expect_pixel(gui,3,3,RED,x=gui.cw//2,y=gui.ch//2)
    assert path.exists() and alias.is_symlink()
    return {"symlink_followed":True,"offset":6,"bytes":4,"pixels":RED}

def temporary_file(gui,root):
    gui.reset()
    path=root/"tty-graphics-protocol-owned.rgba";path.write_bytes(bytes(GREEN))
    gui.emit(at(3,3))
    check_reply(gui,"a=T,t=t,f=32,s=1,v=1,c=1,r=1,i=702,C=1",os.fsencode(path),b"\x1b_Gi=702;OK\x1b\\")
    expect_pixel(gui,3,3,GREEN,x=gui.cw//2,y=gui.ch//2)
    assert not path.exists()
    keep=root/"keep.rgba";keep.write_bytes(bytes(BLUE))
    check_reply(gui,"a=t,t=t,f=32,s=1,v=1,i=703",os.fsencode(keep),b"\x1b_Gi=703;OK\x1b\\")
    assert keep.exists()
    return {"eligible_source_unlinked":True,"nonprotocol_name_retained":True}

def shared_memory(gui,root):
    del root
    gui.reset()
    libc=ctypes.CDLL(None,use_errno=True)
    libc.shm_open.argtypes=[ctypes.c_char_p,ctypes.c_int,ctypes.c_uint]
    libc.shm_open.restype=ctypes.c_int
    libc.shm_unlink.argtypes=[ctypes.c_char_p]
    libc.shm_unlink.restype=ctypes.c_int
    for number,slash in enumerate((False,True)):
        raw=f"ztty-graphics-test-{os.getpid()}-{number}"
        name=("/" if slash else "")+raw
        fd=libc.shm_open(name.encode(),os.O_CREAT|os.O_EXCL|os.O_RDWR,0o600)
        assert fd>=0
        try:
            os.ftruncate(fd,12)
            with mmap.mmap(fd,12) as memory:memory[:]=b"padding!"+bytes(BLUE)
            gui.emit(at(4+number,4))
            ident=710+number
            check_reply(gui,f"a=T,t=s,f=32,s=1,v=1,c=1,r=1,i={ident},C=1,O=8,S=4",name.encode(),f"\x1b_Gi={ident};OK\x1b\\".encode())
            expect_pixel(gui,4+number,4,BLUE,x=gui.cw//2,y=gui.ch//2)
            assert not Path("/dev/shm",raw).exists()
        finally:
            os.close(fd);libc.shm_unlink(name.encode())
    return {"names_with_and_without_slash":True,"range_verified":True,"objects_unlinked":True}

def invalid_sources(gui,root):
    gui.reset()
    path=root/"short";path.write_bytes(b"x")
    fifo=root/"fifo";os.mkfifo(fifo)
    device=root/"device";device.symlink_to("/dev/null")
    expected=b"\x1b_Gi=720;EBADF:Failed to read image file\x1b\\"
    for source in (root/"absent",path,fifo,device,Path("/proc/self/mem")):
        check_reply(gui,"a=t,t=f,f=32,s=1,v=1,i=720,S=4",os.fsencode(source),expected)
    # Without S, a file shorter than the declared pixel dimensions must still
    # return the same opaque error, not a size-specific decoder response.
    check_reply(gui,"a=t,t=f,f=32,s=1,v=1,i=720",os.fsencode(path),expected)
    check_reply(gui,"a=t,t=f,f=100,i=720",os.fsencode(path),expected)
    assert path.exists() and fifo.exists() and device.is_symlink()
    return {"opaque_error_cases":7,"fifo_did_not_block":True,"sources_preserved":True}

def compressed_files(gui,root):
    gui.reset()
    raw=root/"raw.z";raw.write_bytes(zlib.compress(bytes(RED)))
    gui.emit(at(2,2))
    check_reply(gui,f"a=T,t=f,o=z,f=32,s=1,v=1,c=1,r=1,C=1,i=730,S={raw.stat().st_size}",os.fsencode(raw),b"\x1b_Gi=730;OK\x1b\\")
    expect_pixel(gui,2,2,RED,x=gui.cw//2,y=gui.ch//2)
    source=root/"image.png";source.write_bytes(png(1,1,bytes(GREEN)))
    gui.emit(at(4,2))
    check_reply(gui,"a=T,t=f,f=100,c=1,r=1,C=1,i=731",os.fsencode(source),b"\x1b_Gi=731;OK\x1b\\")
    expect_pixel(gui,4,2,GREEN,x=gui.cw//2,y=gui.ch//2)
    return {"zlib_rgba_and_png":True}

def relative_placements(gui,root):
    del root
    gui.reset();gui.emit(at(4,4));image(gui,740,RED)
    image(gui,741,GREEN,P=740,Q=1,H=2,V=1)
    expect_pixel(gui,6,5,GREEN,x=gui.cw//2,y=gui.ch//2)
    gui.emit(at(8,7)+command("a=p,i=740,p=1,c=1,r=1,C=1,q=2"))
    expect_pixel(gui,10,8,GREEN,x=gui.cw//2,y=gui.ch//2)
    expect_pixel(gui,6,5,BG,x=gui.cw//2,y=gui.ch//2)
    gui.emit(command("a=d,d=i,i=740,p=1,q=2"))
    expect_pixel(gui,10,8,BG,x=gui.cw//2,y=gui.ch//2)
    check_reply(gui,"a=p,i=741",b"",b"\x1b_Gi=741;ENOENT:image not found\x1b\\")
    return {"child_followed_moved_parent":True,"parent_removal_cascaded":True}

def placeholders(gui,root):
    del root
    gui.reset()
    gui.emit(command("a=T,U=1,i=42,p=7,s=2,v=1,c=2,r=1,C=1,q=2",bytes(RED+GREEN)))
    gui.emit(at(5,3)+"\x1b[38;5;42;58;5;7m"+PH+"\u0305\u030d"+"\x1b[0m")
    expect_pixel(gui,5,3,GREEN,x=gui.cw//2,y=gui.ch//2)
    expect_pixel(gui,4,3,BG,x=gui.cw//2,y=gui.ch//2)
    gui.emit(at(7,5)+"\x1b[38;5;42;58;5;7m"+PH+"\u0305\u0305"+PH+"\x1b[0m")
    expect_pixel(gui,7,5,RED,x=gui.cw//2,y=gui.ch//2)
    expect_pixel(gui,8,5,GREEN,x=gui.cw//2,y=gui.ch//2)
    # Omitted coordinates inherit from the current left placeholder. Pin the
    # right tile explicitly before deleting its predecessor; Kitty does not
    # require guessed coordinates to survive a missing left neighbour.
    gui.emit(at(8,5)+"\x1b[38;5;42;58;5;7m"+PH+"\u0305\u030d"+"\x1b[0m")
    gui.emit(at(7,5)+" "+at(0,0))
    expect_pixel(gui,7,5,BG,x=gui.cw//2,y=gui.ch//2)
    expect_pixel(gui,8,5,GREEN,x=gui.cw//2,y=gui.ch//2)
    image(gui,742,BLUE,P=42,Q=7,H=3,V=1)
    expect_pixel(gui,8,4,BLUE,x=gui.cw//2,y=gui.ch//2)
    return {"partial_tile_clipped":True,"inherited_column":True,"text_erase_only_removed_its_tile":True,"relative_virtual_parent":True}

def animation(gui,root):
    del root
    gui.reset();gui.emit(at(4,4));image(gui,750,RED)
    gui.emit(command("a=f,i=750,f=32,s=1,v=1,z=150,q=2",bytes(GREEN)))
    gui.emit(command("a=a,i=750,r=1,z=150,c=1,s=3,q=2"))
    expect_pixel(gui,4,4,GREEN,x=gui.cw//2,y=gui.ch//2)
    expect_pixel(gui,4,4,RED,x=gui.cw//2,y=gui.ch//2)
    gui.emit(command("a=a,i=750,s=1,c=2,q=2"))
    expect_pixel(gui,4,4,GREEN,x=gui.cw//2,y=gui.ch//2)
    time.sleep(.22);assert pixel(gui,4,4,x=gui.cw//2,y=gui.ch//2)==GREEN
    gui.emit(command("a=f,i=750,r=2,f=32,s=1,v=1,X=1,q=2",bytes(BLUE)))
    expect_pixel(gui,4,4,BLUE,x=gui.cw//2,y=gui.ch//2)
    gui.emit(command("a=c,i=750,r=2,c=1,w=1,h=1,C=1,q=2")+command("a=a,i=750,c=1,q=2"))
    expect_pixel(gui,4,4,BLUE,x=gui.cw//2,y=gui.ch//2)
    gui.emit(command("a=d,d=F,i=750,r=2,q=2")+command("a=d,d=F,i=750,r=1,q=2"))
    expect_pixel(gui,4,4,BG,x=gui.cw//2,y=gui.ch//2)
    return {"timer_advanced_real_pixels":True,"stop_stable":True,"edit_compose_delete":True}

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,required=True)
    parser.add_argument("--artifacts",type=Path,required=True)
    parser.add_argument("--renderer",choices=("gpu","software"))
    args=parser.parse_args();binary=args.binary.resolve(strict=True);args.artifacts.mkdir(parents=True,exist_ok=True)
    report={"schema":1,"ok":False,"binary":str(binary),"binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),"checks":[],"cleanup_errors":[]}
    os.environ.pop("WAYLAND_DISPLAY",None);os.environ.pop("WAYLAND_SOCKET",None)
    gui=None
    with tempfile.TemporaryDirectory(prefix="ztty-graphics-extensions-") as temporary:
        root=Path(temporary)
        try:
            gui=Gui(binary,"monospace:size=11",temporary,renderer=args.renderer)
            report["cell_pixels"]=[gui.cw,gui.ch];report["display_ownership"]=gui.xvfb.evidence
            report["renderer_before"]=record_renderer(gui.d,gui.window,args.renderer)
            for check in [file_and_symlink,temporary_file,shared_memory,invalid_sources,compressed_files,relative_placements,placeholders,animation]:
                try:
                    detail=check(gui,root);report["checks"].append({"name":check.__name__,"ok":True,**detail})
                    gui.stable_image().save(args.artifacts/(check.__name__+".png"))
                except Exception as error:
                    report["checks"].append({"name":check.__name__,"ok":False,"error":repr(error)})
            report["renderer_after"]=record_renderer(gui.d,gui.window,args.renderer)
            report["ok"]=all(c["ok"] for c in report["checks"])
        except Exception as error:report["error"]=repr(error)
        finally:
            if gui is not None:
                try:gui.close()
                except Exception as error:report["cleanup_errors"].append(repr(error));report["ok"]=False
            stderr=root/"ztty.stderr"
            if stderr.exists():report["stderr_tail"]=stderr.read_text(errors="replace")[-3000:]
    report["passed"]=sum(c["ok"] for c in report["checks"])
    (args.artifacts/"graphics-extensions.json").write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps(report));return 0 if report["ok"] else 1
if __name__=="__main__":raise SystemExit(main())
