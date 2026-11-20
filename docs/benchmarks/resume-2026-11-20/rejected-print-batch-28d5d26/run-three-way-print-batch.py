import argparse,datetime,hashlib,importlib.util,json,os,pathlib,statistics,subprocess,sys,time
WORK=pathlib.Path(__file__).resolve().parents[1]
ROOT=WORK.with_name("resume-20261120")
parser=argparse.ArgumentParser()
parser.add_argument("--root-source-sha",required=True)
parser.add_argument("--candidate-sha256",required=True)
parser.add_argument("--output",required=True,type=pathlib.Path)
parser.add_argument("--expected-nice",required=True,type=int)
parser.add_argument("--candidate-source",required=True)
parser.add_argument("--candidate-binary",required=True,type=pathlib.Path)
parser.add_argument("--build-record",required=True,type=pathlib.Path)
args=parser.parse_args()
OUT=args.output.resolve()
assert OUT.is_relative_to(WORK/"artifacts") and not OUT.exists(),"never overwrite a previous measurement"
OUT.mkdir()
sys.path.insert(0,str(ROOT/"tools"))
spec=importlib.util.spec_from_file_location("zterm_micro_host_pressure",ROOT/"tools/benchmark_terminals.py")
bench=importlib.util.module_from_spec(spec);sys.modules[spec.name]=bench;spec.loader.exec_module(bench)
candidate_source=args.candidate_source
binaries={"legacy_0_4":ROOT/"artifacts/benchmark-final/before-zterm-benchmark","published_0_5":ROOT/"artifacts/benchmark-final/after-zterm-benchmark","candidate_0_5_1":args.candidate_binary.resolve(strict=True)}
expected={"legacy_0_4":"4b3f5d75d409cb697d46a41937e87d2123a59b67127dd2178c1e115423ee0d5d","published_0_5":"7c645edd3c7fe9feae80386c93be856003b1b938f8ad20b5f47dd0898b64d652","candidate_0_5_1":args.candidate_sha256}
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def source(path):return subprocess.check_output(["git","rev-parse","HEAD"],cwd=path,text=True).strip()
def clean(path):return not subprocess.check_output(["git","status","--porcelain"],cwd=path,text=True)
original=ROOT/"artifacts/benchmark-final/core-paired-6053.json"
original_hash=digest(original)
assert original_hash=="bd01c057f00745fe5e186fd13d9ab4422eb4484b4027f8738193edd4263c43f6"
prior_nine=WORK/"artifacts/core-three-way-0.5.1/comparison.json"
prior_nine_hash=digest(prior_nine)
assert prior_nine_hash=="16e8662c14c0f92bcbfe0ebc75982802b066e28552ef5a66b01381c40a30080a"
prior_candidate=WORK/"artifacts/zterm-0.5.1-append-benchmark-bounded"
assert digest(prior_candidate)=="aaa2720f2988d571f85978402102bf1ba0f7aa05147c215ecbd2fa18ecd469ed"
prior_borrowed=WORK/"artifacts/core-three-way-a2074ba/comparison.json"
prior_borrowed_hash=digest(prior_borrowed)
assert prior_borrowed_hash=="765ffc3b74c2191795ed0b587de27a7164a339d5271678663e9ecbe3570abc43"
prior_borrowed_binary=WORK/"artifacts/zterm-0.5.1-borrowed-benchmark"
assert digest(prior_borrowed_binary)=="e6721b2bc2eece65993521695dbb9f2819cef68902ea8e510aaa02157d350bc6"
build=json.loads(args.build_record.resolve(strict=True).read_text())
assert build["passed"] and build["source"]==candidate_source and build["binary_sha256"]==args.candidate_sha256
orders=[["legacy_0_4","published_0_5","candidate_0_5_1"],["published_0_5","candidate_0_5_1","legacy_0_4"],["candidate_0_5_1","legacy_0_4","published_0_5"]]
def execution_context():
    relative=next(x.split("::",1)[1] for x in pathlib.Path("/proc/self/cgroup").read_text().splitlines() if x.startswith("0::"))
    mount=pathlib.Path("/sys/fs/cgroup")
    current=mount/relative.lstrip("/")
    limits=[]
    while current.is_relative_to(mount):
        entry={"path":str(current)}
        for name in ("cpu.max","cpu.weight","cpuset.cpus.effective"):
            if (current/name).exists():entry[name]=(current/name).read_text().strip()
        limits.append(entry)
        if current==mount:break
        current=current.parent
    return {"cgroup":relative,"ancestor_cpu_limits":limits,"nice":os.getpriority(os.PRIO_PROCESS,0),"execution":"Direct native exec; no task-created systemd build unit or CPU quota."}

report={"schema":1,"ok":False,"scope":"Parser and screen model only; no PTY, renderer, display or physical input latency.","timestamp_utc":datetime.datetime.now(datetime.timezone.utc).isoformat(),"collector_sha256":digest(pathlib.Path(__file__)),"host_pressure_helper_sha256":digest(ROOT/"tools/benchmark_terminals.py"),"root_source":args.root_source_sha,"sources":{"legacy_0_4":"155fea51aa138b0503ea1d80be4252aacb2c61a7","published_0_5":"13d5e2c74571d35cccd920b1204798211b70c79f","candidate_0_5_1":candidate_source},"binary_sha256":expected,"profile":build["profile"],"rustc":build["rustc"],"example_sha256":build["example_sha256"],"cpu":23,"nice":os.getpriority(os.PRIO_PROCESS,0),"order":orders,"order_note":"Three rotated orders give each version each position once; previous six samples remain separate.","original_six_samples_sha256":original_hash,"runs":[],"status":"started"}
report["execution_context_before"]=execution_context()
report["previous_nine_samples_sha256"]=prior_nine_hash
report["previous_borrowed_nine_samples_sha256"]=prior_borrowed_hash
report["build_record_sha256"]=digest(args.build_record)
report["candidate_strategy"]="Bounded print-run segmentation; complete scalar fallback remains the reference."
def save():(OUT/"comparison.json").write_text(json.dumps(report,indent=2)+"\n")
save()
try:
 assert report["nice"]==args.expected_nice
 assert all(entry.get("cpu.max","max 100000").split()[0]=="max" for entry in report["execution_context_before"]["ancestor_cpu_limits"])
 assert 23 in os.sched_getaffinity(0)
 assert source(ROOT)==args.root_source_sha and clean(ROOT)
 assert source(WORK)==candidate_source and clean(WORK)
 for name,path in binaries.items():assert digest(path)==expected[name],name
 for round_,order in enumerate(orders,1):
  for name in order:
   pressure=bench.host_pressure();reasons=bench.comparison_pressure_reasons(pressure)
   if reasons:
    report.update(status="deferred",defer_reasons=reasons,host_at_defer=pressure);save();raise SystemExit(2)
   command=["taskset","-c","23",str(binaries[name])]
   start=time.time();p=subprocess.run(command,text=True,capture_output=True,timeout=90)
   (OUT/f"{round_}-{name}.stdout.json").write_text(p.stdout)
   (OUT/f"{round_}-{name}.stderr.log").write_text(p.stderr)
   item={"side":name,"round":round_,"command":command,"exit_code":p.returncode,"elapsed_seconds":time.time()-start,"host_before":pressure,"host_after":bench.host_pressure(),"stderr":p.stderr}
   item["result"]=json.loads(p.stdout) if p.returncode==0 else {"stdout":p.stdout}
   item["host_contention_after"]=bench.comparison_pressure_reasons(item["host_after"])
   report["runs"].append(item);save()
   assert p.returncode==0
   if item["host_contention_after"]:
    report.update(status="contended",defer_reasons=item["host_contention_after"]);save();raise SystemExit(2)
   print(json.dumps({"side":name,"round":round_,"exit_code":0}),flush=True)
 metrics=["ascii_mib_s","ascii_full_history_mib_s","sgr_mib_s","unicode_mib_s","cell_bytes","initial_core_heap_bytes","steady_state_allocations"]
 report["summary"]={}
 for metric in metrics:
  samples={name:[r["result"][metric] for r in report["runs"] if r["side"]==name] for name in binaries}
  medians={name:statistics.median(values) for name,values in samples.items()}
  published=medians["published_0_5"];legacy=medians["legacy_0_4"];candidate=medians["candidate_0_5_1"]
  report["summary"][metric]={"samples":samples,"medians":medians,"candidate_over_published":candidate/published if published else None,"candidate_over_legacy":candidate/legacy if legacy else None}
 for name,path in binaries.items():assert digest(path)==expected[name]
 assert source(ROOT)==args.root_source_sha and clean(ROOT)
 assert source(WORK)==candidate_source and clean(WORK)
 assert digest(original)==original_hash
 assert digest(prior_nine)==prior_nine_hash
 assert digest(prior_borrowed)==prior_borrowed_hash
 assert digest(prior_borrowed_binary)=="e6721b2bc2eece65993521695dbb9f2819cef68902ea8e510aaa02157d350bc6"
 assert digest(prior_candidate)=="aaa2720f2988d571f85978402102bf1ba0f7aa05147c215ecbd2fa18ecd469ed"
 report["previous_nine_samples_and_candidate_unchanged"]=True
 report["source_and_binary_identities_preserved"]=True
 report["original_six_samples_unchanged"]=True
 report.update(ok=True,status="complete");save()
 print(json.dumps({"ok":True,"summary":report["summary"]}),flush=True)
except Exception as e:
 report.update(status="failed",error=repr(e));save();raise

finally:
 report["execution_context_after"]=execution_context()
 save()
