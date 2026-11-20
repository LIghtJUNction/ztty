import pathlib,sys,os,json,hashlib,subprocess,time,threading,re,ctypes,datetime
R=pathlib.Path("WORKTREE")
D=pathlib.Path(__file__).resolve().parent
SOURCE="6885cafdf05903ba5a4a9d3e56121e3a3684bc2e"
BINARY=R/"artifacts/final-elf-zterm051-20261120/ztty-9f5a77a-wayland"
DIGEST="c7935e9e76476a9cd7d0186f23f5806e3e2e2fc556a494b0e3741a2c893b3807"
def digest(p): return hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
def utc(): return datetime.datetime.now(datetime.timezone.utc).isoformat()
def identity():
 head=subprocess.check_output(["git","rev-parse","HEAD"],cwd=R,text=True).strip()
 status=subprocess.check_output(["git","status","--porcelain"],cwd=R,text=True)
 assert head==SOURCE and status=="",(head,status)
 assert digest(BINARY)==DIGEST
 return {"source":head,"clean":True,"binary_sha256":DIGEST}
def birth(pid):
 text=pathlib.Path(f"/proc/{pid}/stat").read_text()
 return text.rsplit(")",1)[1].split()[19]
def owned():
 seen=set();todo=[os.getpid()]
 while todo:
  parent=todo.pop()
  try: children=pathlib.Path(f"/proc/{parent}/task/{parent}/children").read_text().split()
  except (OSError,ProcessLookupError): continue
  for child in children:
   pid=int(child)
   if pid not in seen: seen.add(pid);todo.append(pid)
 return seen
def snapshot(pid,start,mode,index):
 if birth(pid)!=start: return None
 proc=pathlib.Path(f"/proc/{pid}")
 maps=proc.joinpath("maps").read_text()
 smaps=proc.joinpath("smaps").read_text()
 rollup=proc.joinpath("smaps_rollup").read_text()
 if birth(pid)!=start: return None
 groups={}; current=None
 for line in smaps.splitlines():
  if re.match(r"^[0-9a-f]+-[0-9a-f]+ ",line):
   parts=line.split(None,5);current=parts[5] if len(parts)>5 else "[anonymous]"
   groups.setdefault(current,{"Rss_bytes":0,"Pss_bytes":0,"Private_bytes":0})
  elif current and line.startswith(("Rss:","Pss:","Private_Clean:","Private_Dirty:")):
   key,value,*_=line.split();field={"Rss:":"Rss_bytes","Pss:":"Pss_bytes","Private_Clean:":"Private_bytes","Private_Dirty:":"Private_bytes"}[key]
   groups[current][field]+=int(value)*1024
 stem=f"{mode}-{pid}-{start}-{index}"
 mp=D/(stem+".maps");rp=D/(stem+".smaps_rollup")
 mp.write_text(maps);rp.write_text(rollup)
 record={"pid":pid,"start_ticks":start,"captured_utc":utc(),"maps":mp.name,"maps_sha256":digest(mp),
         "rollup":rp.name,"rollup_sha256":digest(rp),
         "groups":[{"path":k,**v} for k,v in sorted(groups.items(),key=lambda kv:-kv[1]["Pss_bytes"])]}
 (D/(stem+".json")).write_text(json.dumps(record,indent=2)+"\n")
 return record
def watch(mode,stop,records,errors):
 seen={};taken=set()
 while not stop.is_set():
  for pid in owned():
   try:
    if pathlib.Path(f"/proc/{pid}/exe").resolve()!=BINARY.resolve(): continue
    start=birth(pid);key=(pid,start)
    seen.setdefault(key,time.monotonic())
    age=time.monotonic()-seen[key]
    for index,threshold in enumerate((2.0,4.0)):
     if age>=threshold and (key,index) not in taken:
      taken.add((key,index))
      row=snapshot(pid,start,mode,index)
      if row: records.append(row)
   except (FileNotFoundError,ProcessLookupError): pass
   except Exception as exc: errors.append(repr(exc))
  stop.wait(.05)
sys.path.insert(0,str(R/"tools"))
import benchmark_terminals as bench
assert ctypes.CDLL(None).prctl(36,1,0,0,0)==0
report={"schema":1,"purpose":"one-run-per-mode PSS and owned mapping diagnosis; timing output is incidental and not a new performance comparison",
        "started":utc(),"binary_source":"9f5a77a1299093d0ba609bfafa2cec3b0ba931dd","before":identity(),
        "script_sha256":digest(__file__),"collector_sha256":digest(R/"tools/benchmark_terminals.py"),
        "runs":[],"ok":False}
try:
 for mode in ("auto","software"):
  identity();records=[];errors=[];stop=threading.Event()
  monitor=threading.Thread(target=watch,args=(mode,stop,records,errors),daemon=True)
  monitor.start()
  try:
   raw=bench.measure("ztty",BINARY,1,4.0,count=1,purpose="comparison",ztty_renderer=mode)
  finally: stop.set();monitor.join()
  raw_path=D/(mode+".json");raw_path.write_text(json.dumps(raw,indent=2)+"\n")
  item={"mode":mode,"raw":raw_path.name,"raw_sha256":digest(raw_path),
        "mapping_snapshots":[{k:r[k] for k in ["pid","start_ticks","captured_utc","maps","maps_sha256","rollup","rollup_sha256"]} for r in records],
        "monitor_errors":errors,"ok":raw["ok"],"remaining_owned_pids":raw["remaining_owned_pids"],
        "application":raw.get("application"),"renderers":raw.get("renderers")}
  report["runs"].append(item)
  (D/"diagnostic.json").write_text(json.dumps(report,indent=2)+"\n")
  print(json.dumps({"mode":mode,"ok":raw["ok"],"application":raw.get("application"),"renderers":raw.get("renderers"),"snapshots":len(records),"monitor_errors":errors,"remaining":raw["remaining_owned_pids"]}),flush=True)
  assert raw["ok"] and raw["remaining_owned_pids"]==[],raw.get("error")
  assert records and not errors,"mapping capture incomplete"
 report["after"]=identity()
 cg=pathlib.Path("/proc/self/cgroup").read_text().strip().split("::")[-1]
 cp=pathlib.Path("/sys/fs/cgroup"+cg)
 report["resource"]={name:(cp/name).read_text().strip() for name in ["memory.max","memory.swap.max","memory.peak","memory.events","cpu.max"] if (cp/name).exists()}
 report["ok"]=True
finally:
 report["finished"]=utc()
 (D/"diagnostic.json").write_text(json.dumps(report,indent=2)+"\n")
print(json.dumps({"ok":report["ok"],"report":str(D/"diagnostic.json"),"sha256":digest(D/"diagnostic.json")}),flush=True)
