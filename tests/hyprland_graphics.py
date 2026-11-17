#!/usr/bin/env python3
"""Display real kitten images in one owned native Hyprland/XWayland window.

This opt-in desktop acceptance probe changes only its own window, captures that
window (not the desktop), and closes it after verification. Requires a logged-in
Hyprland session, python-Xlib/Pillow, and kitten on PATH.
"""
import argparse
import contextlib
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import sys
import tempfile
import termios
import time
import tty

ROOT=Path(__file__).resolve().parents[1]


def wait(predicate, description, timeout=15):
    end=time.monotonic()+timeout
    while time.monotonic()<end:
        value=predicate()
        if value:return value
        time.sleep(0.025)
    raise AssertionError('timed out: '+description)


def child(directory):
    directory=Path(directory)
    saved=termios.tcgetattr(0)
    tty.setraw(0)
    report={'tools':[],'ok':False}
    try:
        (directory/'ready').write_text('ready')
        wait(lambda:(directory/'go').exists(),'parent window placement')
        os.write(1,b'\x1bc\x1b[?25l\x1b]2;ztty - native Kitty graphics acceptance\x07')
        def at(row,col,text):os.write(1,f'\x1b[{row};{col}H{text}'.encode())
        at(2,5,'\x1b[1;38;2;232;239;248mztty\x1b[0m')
        at(2,17,'\x1b[38;2;137;163;185mNATIVE XWAYLAND / KITTY GRAPHICS\x1b[0m')
        at(5,5,'\x1b[38;2;137;163;185mPHOTO · JPEG\x1b[0m')
        at(5,53,'\x1b[38;2;137;163;185mPNG · TRANSPARENT EDGES\x1b[0m')
        detected=subprocess.run(['kitten','icat','--detect-support','--detection-timeout=3'],stdin=0,stdout=1,stderr=subprocess.PIPE,timeout=6)
        report['detected_transport']=detected.stderr.decode(errors='replace').strip()
        assert detected.returncode==0,report['detected_transport']
        assert 'stream' in report['detected_transport'],report['detected_transport']
        for path,place,options in [(ROOT/'tests/fixtures/graphics/hopper.jpg','32x16@4x6',['--transfer-mode=stream']),
                                   (directory/'transparent.png','24x14@52x6',[])]:
            argv=['kitten','icat',*options,'--scale-up','--align=left','--place='+place,str(path)]
            done=subprocess.run(argv,stdin=0,stdout=1,stderr=subprocess.PIPE,timeout=10)
            result={'file':path.name,'exit_code':done.returncode,'stderr':done.stderr.decode(errors='replace'),
                    'transport':'explicit stream' if options else 'automatic detection'}
            report['tools'].append(result)
            assert done.returncode==0,result
        at(24,5,'\x1b[38;2;137;163;185mPTY inline transfer   /   RGBA alpha   /   native retained rendering\x1b[0m')
        at(26,5,'\x1b[38;2;110;134;154mRust · no_std · zterm · zgraphics · zvte\x1b[0m')
        report['pty_size']=list(os.get_terminal_size())
        report['ok']=True
        (directory/'child.json').write_text(json.dumps(report))
        wait(lambda:(directory/'stop').exists(),'screenshot completion')
        return 0
    except BaseException as error:
        report['error']=repr(error)
        (directory/'child.json').write_text(json.dumps(report))
        return 1
    finally:termios.tcsetattr(0,termios.TCSANOW,saved)


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=Path,default=ROOT/'target/release/ztty')
    parser.add_argument('--artifacts',type=Path,default=ROOT/'artifacts')
    parser.add_argument('--child')
    args=parser.parse_args()
    if args.child:return child(args.child)
    if not os.environ.get('HYPRLAND_INSTANCE_SIGNATURE') or not os.environ.get('DISPLAY'):
        parser.error('a logged-in Hyprland/XWayland desktop is required')
    from PIL import Image,ImageDraw
    from Xlib import X,display
    args.artifacts.mkdir(parents=True,exist_ok=True)
    report={'schema':1,'ok':False,'desktop':'Hyprland/XWayland','display':os.environ['DISPLAY'],'checks':[]}
    process=None;connection=None;address=None
    with tempfile.TemporaryDirectory(prefix='ztty-hyprland-graphics-') as temporary:
        directory=Path(temporary)
        try:
            transparent=Image.new('RGBA',(128,128),(0,0,0,0));draw=ImageDraw.Draw(transparent)
            for radius in range(58,0,-1):
                draw.ellipse((64-radius,64-radius,64+radius,64+radius),fill=(70,175,240,min(255,(59-radius)*12)))
            transparent.save(directory/'transparent.png')
            config=directory/'ztty.conf';config.write_text('window-padding = 16\nbackground-opacity = 100\nbackground-blur = false\ncursor-blink = false\n')
            process=subprocess.Popen([str(args.binary.resolve()),'--config',str(config),'--geometry','112x28','-e',sys.executable,str(Path(__file__).resolve()),'--child',str(directory)],stdout=subprocess.DEVNULL,stderr=subprocess.PIPE,start_new_session=True)
            def own_window():
                clients=json.loads(subprocess.check_output(['hyprctl','clients','-j']))
                return next((w for w in clients if w.get('pid')==process.pid),None)
            window_info=wait(own_window,'owned Hyprland window')
            address=window_info['address'];assert window_info.get('xwayland'),window_info
            assert re.fullmatch(r'0x[0-9a-fA-F]+',address),address
            version=json.loads(subprocess.check_output(['hyprctl','version','-j']))
            report['hyprland_version']=version['version']
            modern=tuple(int(n) for n in version['version'].split('.')[:2]) >= (0,55)
            def dispatch(legacy,lua):
                command=['hyprctl','dispatch',lua] if modern else ['hyprctl','dispatch',*legacy]
                done=subprocess.run(command,capture_output=True,text=True)
                assert done.returncode==0,{'command':command,'stdout':done.stdout,'stderr':done.stderr}
            dispatch(['setfloating','address:'+address],
                     f'hl.dsp.window.float({{window="address:{address}",action="enable"}})')
            monitors=json.loads(subprocess.check_output(['hyprctl','monitors','-j']))
            monitor=next(m for m in monitors if m['id']==window_info['monitor'])
            mw,mh=monitor['width'],monitor['height']
            if monitor.get('transform',0)%2:mw,mh=mh,mw
            mw=int(mw/monitor['scale']);mh=int(mh/monitor['scale'])
            width,height=min(1060,mw-64),min(720,mh-64)
            left,top=monitor['x']+(mw-width)//2,monitor['y']+(mh-height)//2
            dispatch(['resizewindowpixel',f'exact {width} {height},address:'+address],
                     f'hl.dsp.window.resize({{window="address:{address}",x={width},y={height},relative=false}})')
            dispatch(['movewindowpixel',f'exact {left} {top},address:'+address],
                     f'hl.dsp.window.move({{window="address:{address}",x={left},y={top},relative=false}})')
            connection=display.Display(os.environ['DISPLAY'])
            pid_atom=connection.intern_atom('_NET_WM_PID')
            def find_xwindow(parent):
                for w in parent.query_tree().children:
                    p=w.get_full_property(pid_atom,X.AnyPropertyType)
                    if p is not None and len(p.value) and int(p.value[0])==process.pid:return w
                    found=find_xwindow(w)
                    if found:return found
                return None
            xwindow=wait(lambda:find_xwindow(connection.screen().root),'owned X11 window')
            wait(lambda:(directory/'ready').exists(),'child readiness')
            time.sleep(0.15)
            (directory/'go').write_text('go')
            wait(lambda:(directory/'child.json').exists(),'real kitten output',20)
            child_report=json.loads((directory/'child.json').read_text());report.update(child_report)
            assert child_report['ok'],child_report
            last=None;stable=0
            for _ in range(50):
                geo=xwindow.get_geometry()
                raw=xwindow.get_image(0,0,geo.width,geo.height,X.ZPixmap,0xffffffff).data
                if raw==last:stable+=1
                else:last=raw;stable=0
                if stable>=3:break
                time.sleep(0.025)
            image=Image.frombytes('RGB',(geo.width,geo.height),raw,'raw','BGRX')
            cols,rows=child_report['pty_size'];cw=(geo.width-32)//cols;ch=(geo.height-32)//rows
            for name,col,width in [('photo',4,32),('transparent_png',52,24)]:
                crop=image.crop((16+col*cw,16+6*ch,16+(col+width)*cw,16+22*ch))
                count=len(crop.getcolors(crop.width*crop.height) or [])
                assert count>64,(name,count)
                report['checks'].append({'name':name,'distinct_colors':count,'passed':True})
            screenshot=args.artifacts/'kitty-graphics-hyprland.png';image.save(screenshot)
            report['window']={'xwayland':True,'client_pixels':[geo.width,geo.height],'cell_pixels':[cw,ch]}
            report['screenshot']=str(screenshot)
            report['ok']=True
            (directory/'stop').write_text('stop')
            process.wait(timeout=5)
            assert process.returncode==0,process.returncode
        except BaseException as error:
            report['ok']=False;report['error']=repr(error)
        finally:
            if process and process.poll() is None:
                with contextlib.suppress(Exception):(directory/'stop').write_text('stop')
                try:process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    os.killpg(process.pid,signal.SIGTERM);process.wait(timeout=3)
            if process:
                report['stderr']=process.stderr.read().decode(errors='replace')[-2000:]
            if connection:
                with contextlib.suppress(Exception):connection.close()
    (args.artifacts/'hyprland-graphics.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))
    return 0 if report['ok'] else 1


if __name__=='__main__':sys.exit(main())
