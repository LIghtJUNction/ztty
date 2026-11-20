import subprocess,pathlib,json,time,hashlib,os,sys,shutil
p=pathlib.Path.cwd();out=p/"artifacts/integration-validation-9855da5-20261120";expected="9855da563f3038adee2fa88e1e24e20eafb77010"
assert not (out/"local-gate.json").exists() and not (out/"local-gate.log").exists()
relative=next(line[3:] for line in pathlib.Path("/proc/self/cgroup").read_text().splitlines() if line.startswith("0::"))
cg=pathlib.Path("/sys/fs/cgroup")/relative.lstrip("/")
limits={name:(cg/name).read_text().strip() for name in ["memory.max","memory.swap.max","cpu.max"]}
assert int(limits["memory.max"])<=768*1024*1024 and limits["memory.swap.max"]=="0"
quota,period=limits["cpu.max"].split();assert quota!="max" and int(quota)<=int(period)
assert os.getpriority(os.PRIO_PROCESS,0)>=10
def git(*args):return subprocess.check_output(["git",*args],text=True).strip()
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
assert git("rev-parse","HEAD")==expected and not git("status","--porcelain")
result={"source":expected,"command":["make","check"],"limits":limits,"wrapper_override_scope":"this bounded systemd user unit only","started_at_unix":time.time(),"cargo_home":os.environ.get("CARGO_HOME"),"cargo_target_dir":os.environ.get("CARGO_TARGET_DIR"),"features":["wayland"],"bare_metal_required":True,"passed":False}
names=["wayland-services","wayland-ime-peer","bash-integration","mcp-wait","mcp-sse"]
snap=out/"historical-before";snap.mkdir();result["historical_outputs"]={}
for name in names:
 original=p/"artifacts"/name
 result["historical_outputs"][name]={"existed":original.exists()}
 if original.exists():
  shutil.copytree(original,snap/name)
  result["historical_outputs"][name]["sha256"]={str(f.relative_to(original)):sha(f) for f in original.rglob("*") if f.is_file()}
try:
 with (out/"local-gate.log").open("x") as log:
  proc=subprocess.run(["make","check"],stdout=log,stderr=subprocess.STDOUT)
 result["exit_code"]=proc.returncode
 result["source_after"]=git("rev-parse","HEAD");result["dirty_after"]=git("status","--porcelain")
 if proc.returncode==0:
  assert result["source_after"]==expected and not result["dirty_after"]
  frozen=p/"artifacts/final-elf-9855da5-20261120";frozen.mkdir(exist_ok=False)
  binary=p/"target/release/ztty";target=frozen/"ztty-9855da5-wayland";shutil.copy2(binary,target)
  result["wayland_elf"]={"path":str(target),"sha256":sha(target),"bytes":target.stat().st_size,"source":expected,"features":["wayland"]}
  result["rustc"]=subprocess.check_output(["rustc","--version","--verbose"],text=True)
  result["cargo_lock_sha256"]=sha(p/"Cargo.lock")
  result["gitlinks"]=subprocess.check_output(["git","ls-tree","HEAD","crates/zgpui","crates/zinput","crates/zpty","crates/zgraphics","crates/zterm"],text=True)
  result["passed"]=True
finally:
 fresh=out/"generated";fresh.mkdir(exist_ok=True)
 for name in names:
  original=p/"artifacts"/name
  if original.exists():shutil.move(str(original),str(fresh/name))
  if (snap/name).exists():shutil.copytree(snap/name,original)
  if result["historical_outputs"][name]["existed"]:
   actual={str(f.relative_to(original)):sha(f) for f in original.rglob("*") if f.is_file()}
   assert actual==result["historical_outputs"][name]["sha256"],"historical artifact changed"
 result["historical_outputs_preserved"]=True
 result.update(elapsed_seconds=round(time.time()-result["started_at_unix"],3),memory_peak_bytes=int((cg/"memory.peak").read_text()),memory_events=(cg/"memory.events").read_text(),swap_peak_bytes=int((cg/"memory.swap.peak").read_text()),log_sha256=sha(out/"local-gate.log"))
 (out/"local-gate.json").write_text(json.dumps(result,indent=2)+"\n")
 print(json.dumps({k:v for k,v in result.items() if k!="historical_outputs"}),flush=True)
 if result.get("exit_code")!=0:print((out/"local-gate.log").read_text()[-10000:],flush=True)
raise SystemExit(0 if result["passed"] else 1)
