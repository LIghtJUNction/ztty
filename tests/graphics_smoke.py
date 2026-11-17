#!/usr/bin/env python3
"""Kitty graphics acceptance through the real PTY and X11 pixels.

Owns one Xvfb and one ztty, reuses the keyboard/clipboard GUI driver, and runs
installed kitten itself inside the slave PTY. No graphics parser is mocked.
"""
import argparse
import base64
import contextlib
import io
import json
from pathlib import Path
import struct
import sys
import tempfile
import time
import zlib

from gui_smoke import Gui, ROOT

ESC = "\x1b"
BG = (24, 36, 48, 191)  # Configured 75% ARGB background in the shared harness.


def command(control, data=b""):
    return ESC + "_G" + control + (";" + base64.b64encode(data).decode() if data else "") + ESC + "\\"


def transmit(data, control, chunk=4096):
    encoded = base64.b64encode(data).decode()
    chunks = [encoded[i:i + chunk] for i in range(0, len(encoded), chunk)] or [""]
    return "".join(ESC + "_G" + (control + "," if i == 0 else "") + f"m={int(i < len(chunks)-1)};" + piece + ESC + "\\"
                   for i, piece in enumerate(chunks))


def png(width, height, pixels):
    def part(kind, data):
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    rows = b"".join(b"\0" + pixels[y * width * 4:(y + 1) * width * 4] for y in range(height))
    return (b"\x89PNG\r\n\x1a\n" + part(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
            + part(b"IDAT", zlib.compress(rows)) + part(b"IEND", b""))


def pixel(gui, col, row, x=0, y=0):
    raw = gui.window.get_image(12 + col * gui.cw + x, 12 + row * gui.ch + y,
                               1, 1, gui.X.ZPixmap, 0xffffffff).data
    return tuple(raw[2::-1]) + (raw[3],)


def expect_pixel(gui, col, row, rgba, x=0, y=0, tolerance=0):
    def matches():
        got = pixel(gui, col, row, x, y)
        return all(abs(a-b) <= tolerance for a, b in zip(got, rgba))
    try:
        gui.wait(matches, f"pixel ({col},{row})+({x},{y})={rgba}")
    except AssertionError as error:
        raise AssertionError(f"{error}; observed {pixel(gui,col,row,x,y)}") from error


def rss(pid):
    status = Path(f"/proc/{pid}/status").read_text()
    return int(next(line.split()[1] for line in status.splitlines() if line.startswith("VmRSS:"))) * 1024


def capability_and_size(gui):
    gui.reset()
    gui.emit(command("a=q,t=d,f=24,s=1,v=1,i=71", b"\0\0\0") + ESC + "[c" + ESC + "[16t" + ESC + "[14t")
    gui.wait(lambda: b"\x1b[4;" in gui.input(), "graphics and geometry replies")
    data = gui.input()
    assert data.index(b"\x1b_Gi=71;OK\x1b\\") < data.index(b"\x1b[?"), data
    assert f"\x1b[6;{gui.ch};{gui.cw}t".encode() in data, data
    assert f"\x1b[4;{gui.ch*gui.snapshot()['rows']};{gui.cw*gui.snapshot()['cols']}t".encode() in data, data
    assert pixel(gui, 0, 0) == BG
    return {"cell_pixels": [gui.cw, gui.ch], "query_did_not_place": True}


def rgb_rgba_png(gui):
    gui.reset()
    gui.emit(ESC + "[4;5H" + command("a=T,t=d,f=24,s=3,v=1,i=1,C=1,q=2", bytes([255,0,0,0,255,0,0,0,255])))
    for i, color in enumerate([(255,0,0,255),(0,255,0,255),(0,0,255,255)]):
        expect_pixel(gui,4,3,color,x=i)
    raw = bytes([255,0,0,255, 0,255,0,128, 0,0,255,0, 255,255,255,64]) * 2
    gui.emit(ESC + "[6;5H" + command("a=T,f=32,s=4,v=2,i=2,C=1,q=2", raw))
    expected = [(255,0,0,255),(12,146,24,223),BG,(82,91,100,207)]
    for i, color in enumerate(expected):
        expect_pixel(gui,4,5,color,x=i,tolerance=1)
    gui.emit(ESC + "[8;5H" + transmit(png(4,2,raw), "a=T,f=100,i=3,C=1,q=2", chunk=16))
    for i, color in enumerate(expected):
        expect_pixel(gui,4,7,color,x=i,tolerance=1)
    return {"formats": [24,32,100], "alpha": [0,64,128,255], "png_base64_chunk": 16}


def chunk_place_delete_replace(gui):
    gui.reset()
    raw = bytes([33,177,241,255]) * (80*40)
    stream = transmit(raw,"a=t,f=32,s=80,v=40,i=10,q=2")
    first_end = stream.index(ESC + "\\") + 2
    gui.emit(stream[:first_end])
    assert pixel(gui,0,0) == BG
    gui.emit(stream[first_end:])
    assert pixel(gui,0,0) == BG
    gui.emit(ESC + "[3;3H" + command("a=p,i=10,p=7,c=10,C=1,q=2"))
    expect_pixel(gui,2,2,(33,177,241,255),x=gui.cw,y=gui.ch)
    gui.emit(command("a=d,d=i,i=10,p=7,q=2"))
    expect_pixel(gui,2,2,BG,x=gui.cw,y=gui.ch)
    gui.emit(command("a=p,i=10,p=8,c=10,C=1,q=2"))
    expect_pixel(gui,2,2,(33,177,241,255),x=gui.cw,y=gui.ch)
    gui.emit(command("a=t,f=32,s=1,v=1,i=10,q=2",bytes([230,30,60,255])) + command("a=p,i=10,p=9,c=2,C=1,q=2"))
    expect_pixel(gui,2,2,(230,30,60,255),x=gui.cw//2,y=gui.ch//2)
    gui.emit(command("a=d,d=I,i=10,q=2"))
    expect_pixel(gui,2,2,BG,x=gui.cw//2,y=gui.ch//2)
    gui.clear_input()
    gui.emit(command("a=p,i=10"))
    gui.wait(lambda: b"ENOENT" in gui.input(), "hard deletion releases image data")
    return {"raw_bytes":len(raw),"chunked":True,"soft_delete_replaced":True,"hard_delete":True}


def crop_clip_and_layers(gui):
    gui.reset()
    raw = b"".join(bytes(color)*2 for color in [(255,0,0,255),(0,255,0,255),(0,0,255,255),(255,255,0,255)])
    gui.emit(command("a=t,f=32,s=4,v=2,i=21,q=2",raw))
    gui.emit(ESC + "[3;3H" + command("a=p,i=21,x=2,y=0,w=2,h=1,C=1,q=2"))
    expect_pixel(gui,2,2,(0,255,0,255),x=0)
    expect_pixel(gui,2,2,(0,255,0,255),x=1)
    expect_pixel(gui,2,2,BG,x=2)
    cols,rows=gui.snapshot()["cols"],gui.snapshot()["rows"]
    gui.emit(f"\x1b[{rows};{cols}H" + command("a=p,i=21,p=2,c=6,r=2,C=1,q=2"))
    # Window padding is outside the terminal viewport even when images exceed it.
    geo=gui.window.get_geometry()
    rawpad=gui.window.get_image(geo.width-1,geo.height-1,1,1,gui.X.ZPixmap,0xffffffff).data
    assert tuple(rawpad[2::-1])+(rawpad[3],)==BG
    gui.emit(ESC+"[10;3H\x1b[44m  \x1b[0m\x1b[10;3H"+command("a=T,f=32,s=1,v=1,i=22,c=2,r=1,z=-2000000000,C=1,q=2",bytes([255,0,0,255])))
    # The explicitly colored cells occlude a bottom-layer image.
    expect_pixel(gui,2,9,(145,181,241,255),x=gui.cw//2,y=gui.ch//2)
    return {"source_crop":True,"viewport_clip":True,"negative_z_background":True}


def scrolling_resize_alternate(gui):
    gui.reset()
    gui.emit(ESC+"[5;5H"+command("a=T,f=32,s=1,v=1,i=30,c=2,C=1,q=2",bytes([10,210,70,255])))
    expect_pixel(gui,4,4,(10,210,70,255),x=gui.cw//2,y=gui.ch//2)
    rows=gui.snapshot()["rows"]
    gui.emit(f"\x1b[{rows};1H\n\n\n")
    expect_pixel(gui,4,1,(10,210,70,255),x=gui.cw//2,y=gui.ch//2)
    expect_pixel(gui,4,4,BG,x=gui.cw//2,y=gui.ch//2)
    gui.emit(ESC+"[?1049h")
    expect_pixel(gui,4,1,BG,x=gui.cw//2,y=gui.ch//2)
    gui.emit(ESC+"[3;3H"+command("a=T,f=32,s=1,v=1,i=31,c=2,C=1,q=2",bytes([250,180,20,255])))
    expect_pixel(gui,2,2,(250,180,20,255),x=gui.cw//2,y=gui.ch//2)
    gui.emit(ESC+"[?1049l")
    expect_pixel(gui,4,1,(10,210,70,255),x=gui.cw//2,y=gui.ch//2)
    expect_pixel(gui,2,2,BG,x=gui.cw//2,y=gui.ch//2)
    gui.emit(ESC+"[H")
    gui.window.configure(width=80*gui.cw+24,height=24*gui.ch+24)
    gui.d.sync()
    gui.wait(lambda:gui.snapshot()["cols"]==80,"resize to 80 columns")
    expect_pixel(gui,4,1,(10,210,70,255),x=gui.cw//2,y=gui.ch//2)
    gui.emit(command("a=d,d=A,q=2"))
    expect_pixel(gui,4,1,BG,x=gui.cw//2,y=gui.ch//2)
    return {"scroll_rows":3,"alternate_isolation":True,"resize_cols":80,"delete_no_ghost":True}


def malformed_and_bounded(gui):
    gui.reset()
    baseline=rss(gui.terminal.pid)
    cases=[command("a=T,f=100,i=101",b"not a PNG"),
           command("a=T,f=32,s=4294967295,v=4294967295,i=102",b"1234"),
           ESC+"_Ga=T,f=32,s=1,v=1,i=103;!!!!"+ESC+"\\",
           command("a=T,f=100,i=104",png(1,1,b"\xff\0\0\xff")[:-7])]
    for case in cases:
        gui.clear_input()
        gui.emit(case)
        gui.wait(lambda:b"E" in gui.input(),"malformed image error")
        assert b";OK" not in gui.input()
    # Incomplete uploads are bounded and independent from subsequent text/key IO.
    for index in range(80):
        gui.emit(ESC+"_Ga=T,f=32,s=2,v=2,i=105,m=1,q=2;AAAA"+ESC+"\\"+command("a=d,d=A,q=2"))
    gui.emit(ESC+"_Ga=T,f=32,s=1,v=1,i=106;"+"A"*8192+ESC+"\\RECOVERED")
    gui.emit(ESC+"_Ga=T,f=32,s=1,v=1,i=107;AAAA"+"\x18"+"\x1b[HRECOVERED")
    gui.clear_input()
    gui.tap("a")
    gui.expect_input(b"a")
    peak=rss(gui.terminal.pid)
    assert peak-baseline<24*1024*1024,(baseline,peak)
    gui.emit(command("a=T,f=32,s=1,v=1,i=108,C=1,q=2",bytes([13,29,233,255])))
    # A valid upload after errors must still render.
    cursor_col=len("RECOVERED")
    expect_pixel(gui,cursor_col,0,(13,29,233,255))
    return {"malformed_cases":len(cases),"aborted_transfers":80,"rss_growth_bytes":peak-baseline,"keyboard_recovered":True}


def kitten_icat(gui, artifacts):
    from PIL import Image, ImageDraw
    gui.reset()
    transparent=Image.new("RGBA",(96,96),(0,0,0,0))
    draw=ImageDraw.Draw(transparent)
    for radius in range(44,0,-1):
        draw.ellipse((48-radius,48-radius,48+radius,48+radius),fill=(40,180,240,min(255,(45-radius)*16)))
    path=artifacts/"transparent-edge.png"
    transparent.save(path)
    old_timeout=gui.control.gettimeout();gui.control.settimeout(25)
    results=[]
    try:
        detected=gui.command("run",argv=["kitten","icat","--detect-support","--detection-timeout=3"])
        assert detected["code"]==0 and detected["stderr"].strip()=="stream",detected
        for image,place,transport in [(ROOT/"tests/fixtures/graphics/hopper.jpg","30x12@2x2",["--transfer-mode=stream"]),(path,"16x8@40x2",[])]:
            result=gui.command("run",argv=["kitten","icat",*transport,"--scale-up","--place="+place,"--align=left",str(image)])
            assert result["code"]==0,result
            results.append({"file":image.name,"exit_code":result["code"],"stderr":result["stderr"],"transport":"explicit stream" if transport else "automatic detection"})
    finally:
        gui.control.settimeout(old_timeout)
    gui.emit(ESC+"[?25l")
    frame=gui.stable_image()
    # Each known placement contains many photographic / alpha-edge colors.
    for left,width in [(2,30),(40,16)]:
        crop=frame.crop((12+left*gui.cw,12+2*gui.ch,12+(left+width)*gui.cw,12+12*gui.ch))
        colors=crop.getcolors(crop.width*crop.height) or []
        assert len(colors)>32,(left,len(colors))
    frame.save(artifacts/"kitty-graphics-xvfb.png")
    return {"detected_transport":detected["stderr"].strip(),"tools":results,"preview":"kitty-graphics-xvfb.png"}


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,default=ROOT/"target/release/ztty")
    parser.add_argument("--artifacts",type=Path,default=ROOT/"artifacts")
    parser.add_argument("--font",default="monospace:size=11")
    args=parser.parse_args();args.artifacts.mkdir(parents=True,exist_ok=True)
    result={"schema":1,"ok":False,"binary":str(args.binary.resolve()),"display":"owned Xvfb","checks":[]}
    gui=None
    with tempfile.TemporaryDirectory(prefix="ztty-graphics-") as directory:
        try:
            gui=Gui.__new__(Gui);gui.__init__(args.binary.resolve(),args.font,directory)
            for check in [capability_and_size,rgb_rgba_png,chunk_place_delete_replace,crop_clip_and_layers,scrolling_resize_alternate,malformed_and_bounded]:
                try:result["checks"].append({"name":check.__name__,"ok":True,**check(gui)})
                except Exception as error:result["checks"].append({"name":check.__name__,"ok":False,"error":f"{type(error).__name__}: {error}"})
            try:result["checks"].append({"name":"kitten_icat","ok":True,**kitten_icat(gui,args.artifacts)})
            except Exception as error:result["checks"].append({"name":"kitten_icat","ok":False,"error":f"{type(error).__name__}: {error}"})
            result["ok"]=all(check["ok"] for check in result["checks"])
        except Exception as error:result["error"]=f"{type(error).__name__}: {error}"
        finally:
            if gui is not None:
                with contextlib.suppress(Exception):gui.close()
            stderr=Path(directory)/"ztty.stderr"
            if stderr.exists():result["stderr_tail"]=stderr.read_text(errors="replace")[-2000:]
    result["passed"]=sum(check["ok"] for check in result["checks"])
    result["failed"]=sum(not check["ok"] for check in result["checks"])
    (args.artifacts/"graphics-smoke.json").write_text(json.dumps(result,indent=2)+"\n")
    print(json.dumps(result))
    return 0 if result["ok"] else 1


if __name__=="__main__":sys.exit(main())
