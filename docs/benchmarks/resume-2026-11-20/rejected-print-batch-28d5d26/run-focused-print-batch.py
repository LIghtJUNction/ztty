"""One focused comparison of the incremental print-batch change."""
import argparse,datetime,hashlib,importlib.util,json,os,pathlib,resource,statistics,subprocess,sys,time
W=pathlib.Path(__file__).resolve().parents[1];R=W.with_name("resume-20261120")
p=argparse.ArgumentParser();p.add_argument("--root-source",required=True);p.add_argument("--output",required=True,type=pathlib.Path);p.add_argument("--expected-nice",required=True,type=int);a=p.parse_args()
out=a.output.resolve();assert out.is_relative_to(W/"artifacts") and not out.exists();out.mkdir()
sys.path.insert(0,str(R/"tools"));spec=importlib.util.spec_from_file_location("focused_host",R/"tools/benchmark_terminals.py");b=importlib.util.module_from_spec(spec);sys.modules[spec.name]=b;spec.loader.exec_module(b)
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def git(w,*args):return subprocess.check_output(["git",*args],cwd=w,text=True).strip()
def context():
 rel=next(x.split("::",1)[1] for x in pathlib.Path("/proc/self/cgroup").read_text().splitlines() if x.startswith("0::"))
 mount=pathlib.Path("/sys/fs/cgroup");cur=mount/rel.lstrip("/");rows=[]
 while cur.is_relative_to(mount):
  row={"path":str(cur)}
  for name in ("cpu.max","cpu.weight","cpuset.cpus.effective"):
   q=cur/name
   if q.exists():row[name]=q.read_text().strip()
  rows.append(row)
  if cur==mount:break
  cur=cur.parent
 return {"nice":os.getpriority(os.PRIO_PROCESS,0),"cgroup":rel,"ancestor_cpu_limits":rows}
sources={"borrowed":"a2074bac4c030cfd7c0b2514c852a89dcf54648f","print_batch":"28d5d266375ff67562e16652a3eef10bd36b4756"}
binary={"borrowed":W/"artifacts/zterm-0.5.1-borrowed-benchmark","print_batch":W/"artifacts/zterm-0.5.1-print-batch-benchmark"}
hashes={"borrowed":"e6721b2bc2eece65993521695dbb9f2819cef68902ea8e510aaa02157d350bc6","print_batch":"0ad515013c182f9edcd4ffac152f0646659507e7676679504ee6ba80cfd6e2d5"}
build_paths={"borrowed":W/"artifacts/release-build-borrowed/build.json","print_batch":W/"artifacts/release-build-print-batch/build.json"}
builds={k:json.loads(v.read_text()) for k,v in build_paths.items()}
prior={R/"artifacts/benchmark-final/core-paired-6053.json":"bd01c057f00745fe5e186fd13d9ab4422eb4484b4027f8738193edd4263c43f6",W/"artifacts/core-three-way-0.5.1/comparison.json":"16e8662c14c0f92bcbfe0ebc75982802b066e28552ef5a66b01381c40a30080a",W/"artifacts/core-three-way-a2074ba/comparison.json":"765ffc3b74c2191795ed0b587de27a7164a339d5271678663e9ecbe3570abc43",W/"artifacts/core-three-way-28d5d26/comparison.json":"215fcf2b8d7cf70f92dd8de08d7093e45e370864aed14bdb6772ce18e8bec13d"}
orders=[["borrowed","print_batch"] if i%2==0 else ["print_batch","borrowed"] for i in range(6)]
report={"schema":1,"ok":False,"status":"started","timestamp_utc":datetime.datetime.now(datetime.timezone.utc).isoformat(),"purpose":"Resolve whether incremental batching improves Unicode at a consistent ASCII/SGR cost; the preceding three-way record remains unchanged.","scope":"Parser/screen only; no PTY, renderer or physical latency.","root_source":a.root_source,"sources":sources,"binary_sha256":hashes,"build_record_sha256":{k:sha(v) for k,v in build_paths.items()},"collector_sha256":sha(pathlib.Path(__file__)),"host_helper_sha256":sha(R/"tools/benchmark_terminals.py"),"orders":orders,"cpu":23,"execution_before":context(),"prior_records":{str(k):v for k,v in prior.items()},"runs":[]}
def save():(out/"comparison.json").write_text(json.dumps(report,indent=2)+"\n")
def verify():
 assert git(R,"rev-parse","HEAD")==a.root_source and not git(R,"status","--porcelain")
 assert git(W,"rev-parse","HEAD")==sources["print_batch"] and not git(W,"status","--porcelain")
 assert all(sha(path)==hashes[k] for k,path in binary.items())
 assert all(sha(path)==h for path,h in prior.items())
 assert all(builds[k]["passed"] and builds[k]["source"]==sources[k] and builds[k]["binary_sha256"]==hashes[k] for k in binary)
 assert builds["borrowed"]["profile"]==builds["print_batch"]["profile"]
 assert builds["borrowed"]["rustc"]==builds["print_batch"]["rustc"]
 assert all(x["example_sha256"]=="8551f815d1ca276154df9f2050a9039f0e9cd225da363137338d3c820ecc3624" for x in builds.values())
 c=context();assert c["nice"]==a.expected_nice and 23 in os.sched_getaffinity(0)
 assert all(x.get("cpu.max","max 100000").split()[0]=="max" for x in c["ancestor_cpu_limits"])
save()
try:
 verify()
 for round_,order in enumerate(orders,1):
  for side in order:
   before=b.host_pressure();reasons=b.comparison_pressure_reasons(before)
   if reasons:report.update(status="deferred",reasons=reasons,host_at_defer=before);save();raise SystemExit(2)
   usage=resource.getrusage(resource.RUSAGE_CHILDREN);start=time.perf_counter()
   command=["taskset","-c","23",str(binary[side])]
   proc=subprocess.run(command,text=True,capture_output=True,timeout=90)
   elapsed=time.perf_counter()-start;after_usage=resource.getrusage(resource.RUSAGE_CHILDREN);after=b.host_pressure()
   (out/f"{round_}-{side}.stdout.json").write_text(proc.stdout);(out/f"{round_}-{side}.stderr.log").write_text(proc.stderr)
   row={"round":round_,"side":side,"command":command,"exit_code":proc.returncode,"elapsed_seconds":elapsed,"child_user_seconds":after_usage.ru_utime-usage.ru_utime,"child_system_seconds":after_usage.ru_stime-usage.ru_stime,"host_before":before,"host_after":after,"host_reasons_after":b.comparison_pressure_reasons(after),"result":json.loads(proc.stdout) if proc.returncode==0 else None}
   report["runs"].append(row);save();assert proc.returncode==0
   if row["host_reasons_after"]:report.update(status="contended",reasons=row["host_reasons_after"]);save();raise SystemExit(2)
   print(json.dumps({"round":round_,"side":side,"exit_code":0}),flush=True)
 metrics=["ascii_mib_s","ascii_full_history_mib_s","sgr_mib_s","unicode_mib_s","cell_bytes","initial_core_heap_bytes","steady_state_allocations"];report["summary"]={}
 for metric in metrics:
  samples={k:[r["result"][metric] for r in report["runs"] if r["side"]==k] for k in binary};med={k:statistics.median(v) for k,v in samples.items()}
  pairs=[]
  for round_ in range(1,7):
   vals={r["side"]:r["result"][metric] for r in report["runs"] if r["round"]==round_}
   pairs.append(vals["print_batch"]/vals["borrowed"] if vals["borrowed"] else None)
  report["summary"][metric]={"samples":samples,"medians":med,"batch_over_borrowed":med["print_batch"]/med["borrowed"] if med["borrowed"] else None,"per_round_batch_over_borrowed":pairs}
 verify();report.update(ok=True,status="complete",identities_and_prior_records_preserved=True);save();print(json.dumps({"ok":True,"summary":report["summary"]}),flush=True)
except Exception as e:
 report.update(status="failed",error=repr(e));save();raise
finally:
 report["execution_after"]=context();save()
