"""Run existing performance collectors with frozen inputs and host guards."""
import argparse, datetime, hashlib, importlib.util, json, os, pathlib, subprocess, sys
ROOT=pathlib.Path(__file__).resolve().parents[2]
p=argparse.ArgumentParser()
p.add_argument("--phase", choices=("throughput","output","terminals"), required=True)
p.add_argument("--source",required=True)
p.add_argument("--binary-source",required=True)
p.add_argument("--binary",type=pathlib.Path,required=True)
p.add_argument("--binary-sha256",required=True)
p.add_argument("--output",type=pathlib.Path,required=True)
p.add_argument("--expected-nice",type=int,required=True)
a=p.parse_args()
a.binary=a.binary.resolve(strict=True)
a.output=a.output.resolve()
assert a.output.is_relative_to(ROOT/"artifacts") and not a.output.exists()
a.output.mkdir()
sys.path.insert(0,str(ROOT/"tools"))
spec=importlib.util.spec_from_file_location("terminal_perf_host",ROOT/"tools/benchmark_terminals.py")
bench=importlib.util.module_from_spec(spec);sys.modules[spec.name]=bench;spec.loader.exec_module(bench)
def sha(path):return hashlib.sha256(path.read_bytes()).hexdigest()
def git(*args):return subprocess.check_output(["git",*args],cwd=ROOT,text=True).strip()
def context():
 rel=next(x.split("::",1)[1] for x in pathlib.Path("/proc/self/cgroup").read_text().splitlines() if x.startswith("0::"))
 mount=pathlib.Path("/sys/fs/cgroup");cur=mount/rel.lstrip("/");limits=[]
 while cur.is_relative_to(mount):
  row={"path":str(cur)}
  for name in ("cpu.max","cpu.weight","cpuset.cpus.effective"):
   q=cur/name
   if q.exists():row[name]=q.read_text().strip()
  limits.append(row)
  if cur==mount:break
  cur=cur.parent
 return {"pid":os.getpid(),"nice":os.getpriority(os.PRIO_PROCESS,0),"affinity":sorted(os.sched_getaffinity(0)),"cgroup":rel,"ancestor_cpu_limits":limits}
tools=("benchmark_throughput.py","benchmark_output_pipeline.py","benchmark_terminals.py","measure_runtime.py","measure_multiwindow.py","isolated_xvfb.py","gui_renderer.py")
identities={name:sha(ROOT/"tools"/name) for name in tools}
original=ROOT/"artifacts/benchmark-final/before-ztty"
expected_old="9f58c1a679ccca62cffd09d9991fbbb9c35fff4254f7d64dfc5eedaa6a2bb704"
old_source="155fea51aa138b0503ea1d80be4252aacb2c61a7"
record={"schema":1,"phase":a.phase,"collector_source":a.source,"binary_source":a.binary_source,"binary":str(a.binary),"binary_sha256":a.binary_sha256,
 "collector_sha256":sha(pathlib.Path(__file__)),"helper_sha256":identities,"started":datetime.datetime.now(datetime.timezone.utc).isoformat(),
 "execution_before":context(),"runs":[],"ok":False,"status":"started",
 "scope":"Private X11 and logical PTY completion; no physical display fence or keyboard-to-photon claim."}
def save():(a.output/"receipt.json").write_text(json.dumps(record,indent=2)+"\n")
def verify():
 assert git("rev-parse","HEAD")==a.source and not git("status","--porcelain"),"source changed or dirty"
 git("diff","--exit-code",a.binary_source,a.source,"--","Cargo.toml","Cargo.lock","build.rs","build","src","native","crates")
 assert sha(a.binary)==a.binary_sha256 and sha(original)==expected_old,"binary identity changed"
 assert {name:sha(ROOT/"tools"/name) for name in tools}==identities,"collector inputs changed"
 c=context()
 assert c["nice"]==a.expected_nice,"unexpected nice"
 assert all(x.get("cpu.max","max 100000").split()[0]=="max" for x in c["ancestor_cpu_limits"]),"CPU quota"
def collect(name,command,path):
 verify()
 assert not path.exists(),"refuse overwrite"
 before=bench.host_pressure();reasons=bench.comparison_pressure_reasons(before)
 row={"name":name,"command":command,"result":str(path),"host_before":before,"before_reasons":reasons}
 record["runs"].append(row);save()
 if reasons:
  record.update(status="deferred",defer_reasons=reasons);save();raise SystemExit(2)
 with (a.output/(name+".stdout.log")).open("x") as out,(a.output/(name+".stderr.log")).open("x") as err:
  proc=subprocess.run(command,cwd=ROOT,stdout=out,stderr=err)
 row.update(exit_code=proc.returncode,host_after=bench.host_pressure())
 row["after_reasons"]=bench.comparison_pressure_reasons(row["host_after"])
 if path.exists():
  row["result_sha256"]=sha(path);result=json.loads(path.read_text());row["collector_ok"]=result.get("ok")
 save();verify()
 if proc.returncode!=0 or row.get("collector_ok") is not True:
  raise RuntimeError("collector failed: "+name)
 if row["after_reasons"]:
  record.update(status="contended",defer_reasons=row["after_reasons"]);save();raise SystemExit(2)
 print(json.dumps({"case":name,"passed":True,"result_sha256":row["result_sha256"]}),flush=True)
save()
try:
 verify()
 exe="/usr/bin/python3"
 if a.phase=="throughput":
  out=a.output/"paired.json"
  collect("paired",[exe,str(ROOT/"tools/benchmark_throughput.py"),"--before",str(original),"--after",str(a.binary),
   "--before-source",old_source,"--after-source",a.binary_source,"--counts","1,10","--rounds","3","--settle-seconds","1","--json",str(out)],out)
 elif a.phase=="output":
  for kind in ("ascii-log","inplace-ascii","unicode-log"):
   for mode in ("headless","window"):
    name=kind+"-"+mode;out=a.output/(name+".json")
    collect(name,[exe,str(ROOT/"tools/benchmark_output_pipeline.py"),"--binary",str(a.binary),"--binary-source",a.binary_source,
     "--output",str(out),"--kinds",kind,"--modes",mode,"--mib","8","--samples","3","--history","2000"],out)
 else:
  out=a.output/"terminals.json"
  collect("terminals",[exe,str(ROOT/"tools/benchmark_terminals.py"),"--ztty",str(a.binary),"--ghostty","/usr/bin/ghostty",
   "--kitty","/usr/bin/kitty","--json",str(out),"--svg",str(a.output/"terminals.svg"),"--counts","1,10","--modes","shared",
   "--workloads","idle","--apps","ztty,ghostty,kitty","--repetitions","3","--purpose","comparison","--settle-seconds","1",
   "--no-readme","--ztty-renderer","gpu"],out)
 verify();record.update(ok=True,status="complete")
except Exception as e:
 record.update(status="failed",error=repr(e));save();raise
finally:
 record["execution_after"]=context();record["finished"]=datetime.datetime.now(datetime.timezone.utc).isoformat();save()
