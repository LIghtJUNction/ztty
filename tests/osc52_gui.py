#!/usr/bin/env python3
"""OSC 52 with real owned X11 selections, INCR transfers and a raw PTY."""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import socket
import tempfile
import time

from gui_smoke import Gui
from gui_renderer import record_renderer

LIMIT=1024*1024
def osc(data=None,target="c",bell=False):
    payload="?" if data is None else base64.b64encode(data).decode()
    return "\x1b]52;"+target+";"+payload+("\x07" if bell else "\x1b\\")
def reply(data=b"",target="c",bell=False):return osc(data,target,bell).encode()

class ClipboardGui(Gui):
    def __init__(self,*args,**kwargs):
        self.sources={};self.held=[];self.transfers={};self.xerrors=[]
        super().__init__(*args,**kwargs)
        self.d.set_error_handler(lambda error,_request:self.xerrors.append(type(error).__name__))
        sessions=self.rpc({"op":"list"})["sessions"]
        assert len(sessions)==1 and sessions[0]["window_id"]==self.window.id
        self.session=sessions[0]["uri"]
        self.broker_pid=self.rpc({"op":"list"})["broker_pid"]
    def rpc(self,request):
        # This is the exact private socket chosen by Gui, never the user pool.
        with socket.socket(socket.AF_UNIX,socket.SOCK_STREAM) as channel:
            channel.settimeout(3);channel.connect("\0ztty-gui-"+str(os.getpid()))
            channel.sendall(json.dumps(request).encode()+b"\n");data=bytearray()
            while b"\n" not in data:
                part=channel.recv(65536)
                if not part or len(data)>1048576:raise RuntimeError("invalid private broker reply")
                data.extend(part)
        result=json.loads(data.split(b"\n",1)[0]);assert result["ok"],result;return result["result"]
    def control_owner(self,actor):
        return self.rpc({"op":"control","session":self.session,"_actor":actor,"action":"acquire","takeover":True})
    def own(self,payload,target="c",mode="direct"):
        atom=self.atoms["CLIPBOARD"] if target=="c" else self.Xatom.PRIMARY
        self.sources[atom]={"payload":payload,"mode":mode}
        self.helper.set_selection_owner(atom,self.X.CurrentTime);self.d.sync()
        self.pump()
    def read_selection(self,target="c"):
        atom=self.atoms["CLIPBOARD"] if target=="c" else self.Xatom.PRIMARY
        self.selection_result=None
        self.helper.convert_selection(atom,self.atoms["UTF8_STRING"],self.atoms["ZTTY_GUI_REPLY"],self.X.CurrentTime)
        self.d.flush();self.wait(lambda:self.selection_result is not None,"selection contents")
        assert self.selection_result is not False
        return self.selection_result
    def notify(self,event,prop):
        event.requestor.send_event(self.protocol.event.SelectionNotify(time=event.time,
            requestor=event.requestor,selection=event.selection,target=event.target,property=prop))
        self.d.flush()
    def property_bytes(self,window,prop,kind,data):
        window.change_property(prop,kind,8,data[:32768],self.X.PropModeReplace)
        for at in range(32768,len(data),32768):
            window.change_property(prop,kind,8,data[at:at+32768],self.X.PropModeAppend)
    def pump(self):
        if self.d is None:return
        X=self.X
        while self.d.pending_events():
            event=self.d.next_event()
            if event.type==X.SelectionRequest and event.selection in self.sources:
                source=self.sources[event.selection];mode=source["mode"];prop=event.property or event.target
                targets=self.d.intern_atom("TARGETS")
                if event.target==targets:
                    kind=self.Xatom.STRING if mode=="latin" else self.atoms["UTF8_STRING"]
                    event.requestor.change_property(prop,self.Xatom.ATOM,32,[targets,kind])
                    self.notify(event,prop);continue
                if mode=="hold":self.held.append(event);continue
                if mode=="latin" and event.target==self.atoms["UTF8_STRING"]:
                    self.notify(event,0);continue
                kind=self.Xatom.STRING if mode=="latin" else self.atoms["UTF8_STRING"]
                if event.target not in (self.atoms["UTF8_STRING"],self.Xatom.STRING):
                    self.notify(event,0);continue
                data=source["payload"]
                if mode=="incr":
                    event.requestor.change_attributes(event_mask=X.PropertyChangeMask)
                    event.requestor.change_property(prop,self.d.intern_atom("INCR"),32,[len(data)])
                    self.transfers[(event.requestor.id,prop)]=[event.requestor,kind,data,0]
                else:self.property_bytes(event.requestor,prop,kind,data)
                self.notify(event,prop)
            elif event.type==X.PropertyNotify and event.state==X.PropertyDelete:
                key=(event.window.id,event.atom)
                if key in self.transfers:
                    window,kind,data,at=self.transfers[key]
                    piece=data[at:at+16381]
                    window.change_property(event.atom,kind,8,piece)
                    if piece:self.transfers[key][3]+=len(piece)
                    else:del self.transfers[key]
                    self.d.flush()
            elif event.type==X.SelectionClear:
                # python-Xlib names this field atom, unlike SelectionRequest.
                if getattr(self.d.get_selection_owner(event.atom),"id",0)!=self.helper.id:
                    self.sources.pop(event.atom,None)
            elif event.type==X.SelectionNotify and event.requestor.id==self.helper.id:
                if event.property==X.NONE:self.selection_result=False
                else:
                    prop=self.helper.get_full_property(event.property,X.AnyPropertyType)
                    self.selection_result=bytes(prop.value) if prop is not None else False
                    self.helper.delete_property(event.property)
            elif event.type in (X.KeyPress,X.KeyRelease):
                self.key_events.append((event.type,event.detail,event.time))
    def missing_window(self,window):
        from Xlib.error import BadWindow
        try:window.get_attributes();return False
        except BadWindow:return True

def writes(gui):
    gui.reset();gui.emit(osc("hello 中文😀\nnext".encode()))
    assert gui.read_selection()=="hello 中文😀\nnext".encode()
    gui.emit(osc("primary 文本".encode(),"p"))
    assert gui.read_selection("p")=="primary 文本".encode()
    return {"clipboard_and_primary_utf8":True}

def queries(gui):
    gui.reset();gui.own("read 中文😀".encode())
    gui.emit("\x1b[?2004h"+osc(None,bell=True));gui.expect_input(reply("read 中文😀".encode(),bell=True))
    gui.clear_input();gui.own(b"primary","p")
    gui.emit(osc(None,"p"));gui.expect_input(reply(b"primary","p"))
    return {"BEL_and_ST":True,"no_bracketed_paste_markers":True}

def latin_and_incr(gui):
    gui.reset();gui.own(b"caf\xe9","c","latin")
    gui.emit(osc());gui.expect_input(reply("café".encode()))
    gui.clear_input();data=("line 中文😀\n"*1800).encode();gui.own(data,"c","incr")
    gui.emit(osc());gui.expect_input(reply(data))
    return {"latin1_converted_once":True,"INCR_bytes":len(data),"split_inside_utf8":True}

def exact_budget(gui):
    gui.reset();data=b"x"*LIMIT
    gui.emit(osc(data));assert gui.read_selection()==data
    gui.clear_input();gui.own(data,"c","incr");gui.emit(osc())
    expected=reply(data)
    gui.wait(lambda:len(gui.input())>=len(expected),"1 MiB query response",12)
    actual=gui.input();assert actual==expected
    return {"decoded_bytes":LIMIT,"encoded_reply_bytes":len(actual),"sha256":hashlib.sha256(data).hexdigest()}

def over_budget(gui):
    gui.reset();gui.own(b"keep")
    gui.emit(osc(b"x"*(LIMIT+1)));assert gui.read_selection()==b"keep"
    gui.clear_input();gui.own(b"x"*(LIMIT+1),"c","incr");gui.emit(osc())
    gui.expect_input(reply())
    return {"oversize_write_preserved_clipboard":True,"oversize_read_empty_not_partial":True}

def invalid_text(gui):
    gui.reset();gui.own(b"keep")
    for data in [b"\0",b"\xff"]:
        gui.emit(osc(data));assert gui.read_selection()==b"keep"
    gui.emit("\x1b]52;c;!\x1b\\");assert gui.read_selection()==b""
    return {"invalid_utf8_and_nul_rejected":True,"invalid_base64_cleared_per_xterm":True}

def denied_owner(gui):
    gui.reset();gui.own(b"owner-secret")
    info=gui.control_owner("mcp:osc52-fixture");assert info["input_owner"]=="mcp:osc52-fixture"
    try:
        gui.clear_input()
        gui.command("write",text=osc(b"forbidden")+osc()+"\x1b[6n")
        gui.wait(lambda:b"R" in gui.input(),"denied query and parser barrier")
        data=gui.input();assert reply() in data and base64.b64encode(b"owner-secret") not in data
        assert gui.read_selection()==b"owner-secret"
    finally:gui.control_owner("local")
    return {"nonlocal_writes_ignored":True,"nonlocal_query_empty":True}

def cancelled_owner(gui):
    gui.reset();gui.own(b"stale","c","hold");gui.held.clear()
    started=time.monotonic();gui.emit(osc());gui.wait(lambda:gui.held,"held OSC request")
    event=gui.held[-1];assert event.requestor.id!=gui.window.id
    gui.control_owner("mcp:osc52-fixture");gui.control_owner("local")
    assert time.monotonic()-started<1.8,"owner cancellation must precede the timeout"
    gui.wait(lambda:gui.missing_window(event.requestor),"cancelled requestor destroyed")
    gui.clear_input();gui.own(b"fresh");gui.emit(osc());gui.expect_input(reply(b"fresh"))
    return {"requestor_distinct_from_window":True,"generation_change_destroyed_requestor":True,"new_query_fresh":True}

def cancelled_by_manual_paste(gui):
    gui.reset();gui.own(b"old","c","hold");gui.held.clear();gui.emit(osc());gui.wait(lambda:gui.held,"held OSC request")
    event=gui.held[-1];gui.own(b"manual")
    gui.chord("Control_L","Shift_L","v")
    gui.expect_input(b"manual")
    assert gui.missing_window(event.requestor)
    return {"manual_paste_preempted_query":True,"no_OSC_reply_in_user_text":True}

def query_during_manual_transfer(gui):
    gui.reset();gui.own(b"manual-later","c","hold");gui.held.clear()
    gui.chord("Control_L","Shift_L","v");gui.wait(lambda:gui.held,"held manual request")
    event=gui.held[-1];assert event.requestor.id==gui.window.id
    gui.emit(osc());gui.expect_input(reply())
    assert not gui.missing_window(event.requestor)
    gui.clear_input()
    gui.property_bytes(event.requestor,event.property,gui.atoms["UTF8_STRING"],b"manual-later")
    gui.notify(event,event.property);gui.expect_input(b"manual-later")
    return {"busy_query_empty":True,"manual_transfer_preserved":True}

def query_timeout(gui):
    gui.reset();gui.own(b"old","c","hold");gui.held.clear();gui.emit(osc());gui.wait(lambda:gui.held,"held OSC request")
    event=gui.held[-1];gui.expect_input(reply(),timeout=4)
    assert gui.missing_window(event.requestor)
    gui.clear_input();gui.own(b"after-timeout");gui.emit(osc());gui.expect_input(reply(b"after-timeout"))
    return {"timeout_empty_reply":True,"owned_requestor_destroyed":True,"next_request_recovered":True}

def denied_view(gui, headless):
    gui.reset();gui.own(b"view-secret")
    output=gui.directory/("headless-reply.bin" if headless else "observer-reply.bin")
    child = (
        "import os,sys,tty,select,time,pathlib\n"
        "tty.setraw(0)\n"
        "os.write(1,b'\\x1b]52;c;Zm9yYmlkZGVu\\x1b\\\\\\x1b]52;c;?\\x1b\\\\')\n"
        "data=bytearray();deadline=time.monotonic()+3\n"
        "while time.monotonic()<deadline and not data.endswith(b'\\x1b\\\\'):\n"
        " if select.select([0],[],[],.05)[0]: data.extend(os.read(0,256))\n"
        "pathlib.Path(sys.argv[1]).write_bytes(data)\n"
    )
    info=gui.rpc({"op":"create","name":"osc52-headless" if headless else "osc52-observer",
        "origin":"local","_actor":"local","headless":headless,"observe":not headless,
        "cwd":str(gui.directory),"command":["/usr/bin/python3","-c",child,str(output)]})
    assert info["input_owner"]=="local"
    if headless:assert not info["attached"]
    else:assert info["read_only"]
    gui.wait(output.exists,"denied view PTY reply")
    assert output.read_bytes()==reply()
    assert gui.read_selection()==b"view-secret"
    gui.wait(lambda:not Path(f"/proc/{info['pid']}").exists(),"fixture child reaped")
    gui.window.set_input_focus(gui.X.RevertToParent,gui.X.CurrentTime);gui.d.sync()
    return {"local_owner":True,"headless":headless,"observe":not headless,"empty_query_and_unchanged_clipboard":True}

def headless_denied(gui):return denied_view(gui,True)
def observer_denied(gui):return denied_view(gui,False)
def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary",type=Path,required=True);parser.add_argument("--artifacts",type=Path,required=True)
    parser.add_argument("--renderer",choices=("gpu","software"))
    args=parser.parse_args();binary=args.binary.resolve(strict=True);args.artifacts.mkdir(parents=True,exist_ok=True)
    report={"schema":1,"ok":False,"binary":str(binary),"binary_sha256":hashlib.sha256(binary.read_bytes()).hexdigest(),"checks":[],"cleanup_errors":[]}
    os.environ.pop("WAYLAND_DISPLAY",None);os.environ.pop("WAYLAND_SOCKET",None);gui=None
    with tempfile.TemporaryDirectory(prefix="ztty-osc52-") as directory:
        try:
            os.environ["XDG_CONFIG_HOME"]=str(Path(directory)/"config")
            gui=ClipboardGui(binary,"monospace:size=11",directory,renderer=args.renderer)
            report["display_ownership"]=gui.xvfb.evidence
            report["renderer_before"]=record_renderer(gui.d,gui.window,args.renderer)
            for check in [writes,queries,latin_and_incr,exact_budget,over_budget,invalid_text,denied_owner,headless_denied,observer_denied,cancelled_owner,cancelled_by_manual_paste,query_during_manual_transfer,query_timeout]:
                try:report["checks"].append({"name":check.__name__,"ok":True,**check(gui)})
                except Exception as error:report["checks"].append({"name":check.__name__,"ok":False,"error":repr(error)})
            report["renderer_after"]=record_renderer(gui.d,gui.window,args.renderer)
            report["ok"]=all(c["ok"] for c in report["checks"])
            report["expected_cancelled_x_errors"]=len(gui.xerrors)
        except Exception as error:report["error"]=repr(error)
        finally:
            if gui is not None:
                try:gui.control_owner("local")
                except Exception as error:report["cleanup_errors"].append(repr(error));report["ok"]=False
                try:gui.close()
                except Exception as error:report["cleanup_errors"].append(repr(error));report["ok"]=False
            path=Path(directory)/"ztty.stderr"
            if path.exists():report["stderr_tail"]=path.read_text(errors="replace")[-3000:]
    report["passed"]=sum(c["ok"] for c in report["checks"])
    (args.artifacts/"osc52.json").write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps(report));return 0 if report["ok"] else 1
if __name__=="__main__":raise SystemExit(main())
