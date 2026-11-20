import pathlib,sys,os,json,hashlib,subprocess,datetime,signal
R=pathlib.Path("WORKTREE")
D=pathlib.Path(__file__).resolve().parent
S="9855da563f3038adee2fa88e1e24e20eafb77010"
B=R/"artifacts/final-elf-9855da5-20261120/ztty-9855da5-wayland"
H="9c0297477b2a24355f865d12413135a327a69a7a8f7dd50e40927ae9e21f1e66"
def digest(p):return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def utc():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def identity():
 head=subprocess.check_output(["git","rev-parse","HEAD"],cwd=R,text=True).strip()
 status=subprocess.check_output(["git","status","--porcelain"],cwd=R,text=True)
 assert head==S and status=="",(head,status)
 assert digest(B)==H and B.stat().st_size==1237824
 return {"source":head,"clean":True,"binary":str(B),"binary_sha256":H,"binary_bytes":B.stat().st_size}
cases=[
 ("native-ui-auto",[sys.executable,str(R/"tests/native_ui_gui.py"),"--binary",str(B),"--source-root",str(R),"--renderer","auto","--output",str(D/"native-ui-auto.json"),"--screenshot",str(D/"native-ui-auto.png")],24),
 ("text-gpu",[sys.executable,str(R/"tests/text_gpu_gui.py"),"--binary",str(B),"--renderer","gpu","--output",str(D/"text-gpu.json"),"--screenshots",str(D/"text-gpu")],8)]
report={"schema":1,"started":utc(),"purpose":"Targeted existing pixel/UI verification of exact Auto software-environment early return and explicit GPU hot switching",
        "before":identity(),"environment":{"LIBGL_ALWAYS_SOFTWARE":os.environ.get("LIBGL_ALWAYS_SOFTWARE"),"ZTTY_DISPLAY":os.environ.get("ZTTY_DISPLAY")},
        "script_sha256":digest(__file__),"runs":[],"ok":False}
assert report["environment"]=={"LIBGL_ALWAYS_SOFTWARE":"1","ZTTY_DISPLAY":"x11"}
try:
 for name,cmd,count in cases:
  identity();log=D/(name+".log")
  with log.open("wb") as stream:
   p=subprocess.Popen(cmd,cwd=R,stdout=stream,stderr=subprocess.STDOUT,start_new_session=True)
   try:code=p.wait(timeout=180)
   except subprocess.TimeoutExpired:
    os.killpg(p.pid,signal.SIGTERM)
    try:p.wait(timeout=3)
    except subprocess.TimeoutExpired:os.killpg(p.pid,signal.SIGKILL);p.wait()
    raise
  rp=D/(name+".json")
  raw=json.loads(rp.read_text()) if rp.exists() else {}
  row={"name":name,"command":cmd,"exit_code":code,"report":str(rp),"report_sha256":digest(rp) if rp.exists() else None,"log_sha256":digest(log),
       "checks":len(raw.get("checks",[])),"ok":raw.get("ok"),"renderer_before":raw.get("renderer_before"),"renderer_after":raw.get("renderer_after"),
       "display_ownership":raw.get("display_ownership"),"display_cleanup":raw.get("display_cleanup"),
       "cleanup":raw.get("cleanup",{"owned_processes_remaining":raw.get("remaining_owned_pids")})}
  report["runs"].append(row)
  (D/"acceptance.json").write_text(json.dumps(report,indent=2)+"\n")
  print(json.dumps(row),flush=True)
  assert code==0 and raw["ok"] and len(raw["checks"])==count,raw.get("error")
  assert raw["binary_sha256"]==H
  for key,value in raw.get("source_sha256",{}).items():assert digest(R/key)==value,key
  assert row["cleanup"]["owned_processes_remaining"]==[]
  own=raw["display_ownership"];assert own["pid_lock_verified"] and own["filesystem_peer_pid_verified"]
  assert raw["renderer_before"]==raw["renderer_after"]
  rb=raw["renderer_before"]
  if name=="native-ui-auto":
   assert rb["requested"]=="auto" and rb["actual"]=="software" and rb["driver"]=="",rb
  else:
   assert rb["requested"]=="gpu" and rb["actual"]=="gpu" and rb["driver"],rb
   assert "live GPU/software switching preserves the PTY and returns to the requested backend" in raw["checks"]
  if row["display_cleanup"]:
   assert not any(row["display_cleanup"].get(k) for k in ["pid_lock_exists","socket_exists","lease_exists"])
 report["after"]=identity()
 cg=pathlib.Path("/proc/self/cgroup").read_text().strip().split("::")[-1]
 cp=pathlib.Path("/sys/fs/cgroup"+cg)
 report["resources"]={k:(cp/k).read_text().strip() for k in ["memory.max","memory.swap.max","memory.peak","memory.events","cpu.max"] if (cp/k).exists()}
 report["ok"]=True
finally:
 report["finished"]=utc()
 (D/"acceptance.json").write_text(json.dumps(report,indent=2)+"\n")
print(json.dumps({"ok":report["ok"],"checks":sum(x["checks"] for x in report["runs"]),"report_sha256":digest(D/"acceptance.json")}),flush=True)
