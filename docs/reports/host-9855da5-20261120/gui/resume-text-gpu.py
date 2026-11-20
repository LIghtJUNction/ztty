import pathlib,sys,os,json,hashlib,subprocess,datetime,signal
R=pathlib.Path("WORKTREE");D=pathlib.Path(__file__).resolve().parent
S="9855da563f3038adee2fa88e1e24e20eafb77010";B=R/"artifacts/final-elf-9855da5-20261120/ztty-9855da5-wayland"
H="9c0297477b2a24355f865d12413135a327a69a7a8f7dd50e40927ae9e21f1e66"
def sha(p):return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def utc():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def identity():
 assert subprocess.check_output(["git","rev-parse","HEAD"],cwd=R,text=True).strip()==S
 assert not subprocess.check_output(["git","status","--porcelain"],cwd=R,text=True)
 assert sha(B)==H
 return {"source":S,"source_clean":True,"binary_sha256":H}
def source_check(raw):
 assert raw["binary_sha256"]==H
 for name,digest in raw["source_sha256"].items():assert sha(R/name)==digest,name
report={"schema":1,"started":utc(),"before":identity(),"runs":[],"ok":False,
 "original_wrapper_receipt":{"path":str(D/"acceptance.json"),"sha256":sha(D/"acceptance.json"),
 "status":"preserved exit 1: native UI report uses xvfb, not display_ownership; passed product stage was not rerun",
 "legitimate_window_recreation":"Native UI closes the first view and creates its producer view, so compare renderer semantics without requiring the same window_id."}}
try:
 auto_path=D/"native-ui-auto.json";assert sha(auto_path)=="80ce004845467e2df85f0c0aa9615ca94becbd9e34a92ca12f599cf3e2a12035"
 a=json.loads(auto_path.read_text());source_check(a)
 assert a["ok"] and len(a["checks"])==24 and a["source_commit"]==S and not a["source_changes"]
 assert a["cleanup"]["owned_processes_remaining"]==[]
 own=a["xvfb"];assert own["pid_lock_verified"] and own["filesystem_peer_pid_verified"]
 num=int(own["display"].lstrip(":"));assert 100<=num<=30000
 ac={k:p.exists() for k,p in {"pid_lock_exists":pathlib.Path(f"/tmp/.X{num}-lock"),"socket_exists":pathlib.Path(f"/tmp/.X11-unix/X{num}"),"lease_exists":pathlib.Path(f"/tmp/ztty-xvfb-{num}.lease")}.items()}
 assert not any(ac.values()),ac
 for name in ["renderer_before","renderer_after"]:
  rr=a[name];assert rr["requested"]=="auto" and rr["actual"]=="software" and rr["driver"]=="",rr
 report["runs"].append({"name":"native-ui-auto","checks":24,"product_exit_code":0,"ok":True,"raw_sha256":sha(auto_path),"renderer_before":a["renderer_before"],"renderer_after":a["renderer_after"],"owned_remaining":[],"display_cleanup":ac})
 (D/"verified-acceptance.json").write_text(json.dumps(report,indent=2)+"\n")
 p=D/"text-gpu.json";assert not p.exists()
 cmd=[sys.executable,str(R/"tests/text_gpu_gui.py"),"--binary",str(B),"--renderer","gpu","--output",str(p),"--screenshots",str(D/"text-gpu")]
 with (D/"text-gpu.log").open("wb") as log:
  child=subprocess.Popen(cmd,cwd=R,stdout=log,stderr=subprocess.STDOUT,start_new_session=True)
  try:code=child.wait(timeout=180)
  except subprocess.TimeoutExpired:
   os.killpg(child.pid,signal.SIGTERM)
   try:child.wait(timeout=3)
   except subprocess.TimeoutExpired:os.killpg(child.pid,signal.SIGKILL);child.wait()
   raise
 g=json.loads(p.read_text());source_check(g)
 assert code==0 and g["ok"] and len(g["checks"])==8,g.get("error")
 assert g["remaining_owned_pids"]==[]
 own=g["display_ownership"];assert own["pid_lock_verified"] and own["filesystem_peer_pid_verified"]
 assert not any(g["display_cleanup"].get(k) for k in ["pid_lock_exists","socket_exists","lease_exists"])
 for name in ["renderer_before","renderer_after"]:
  rr=g[name];assert rr["requested"]=="gpu" and rr["actual"]=="gpu" and rr["driver"],rr
 assert g["renderer_before"]==g["renderer_after"]
 assert "live GPU/software switching preserves the PTY and returns to the requested backend" in g["checks"]
 report["runs"].append({"name":"text-gpu","command":cmd,"checks":8,"product_exit_code":code,"ok":True,"raw_sha256":sha(p),"renderer_before":g["renderer_before"],"renderer_after":g["renderer_after"],"owned_remaining":[],"display_cleanup":g["display_cleanup"],"backend_font_boxes":g["backend_font_boxes"]})
 report["after"]=identity()
 cg=pathlib.Path("/proc/self/cgroup").read_text().strip().split("::")[-1];cp=pathlib.Path("/sys/fs/cgroup"+cg)
 report["resources"]={k:(cp/k).read_text().strip() for k in ["memory.max","memory.swap.max","memory.peak","memory.events","cpu.max"] if (cp/k).exists()}
 report["ok"]=True
finally:
 report["finished"]=utc();(D/"verified-acceptance.json").write_text(json.dumps(report,indent=2)+"\n")
print(json.dumps({"ok":report["ok"],"checks":sum(x["checks"] for x in report["runs"]),"sha256":sha(D/"verified-acceptance.json"),"runs":report["runs"]}),flush=True)
