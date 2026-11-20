import pathlib,subprocess,json,hashlib,time,datetime,os,importlib.util,statistics,sys
ROOT=pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0,str(ROOT/'tools'))
spec=importlib.util.spec_from_file_location('terminal_bench',ROOT/'tools/benchmark_terminals.py')
bench=importlib.util.module_from_spec(spec);sys.modules[spec.name]=bench;spec.loader.exec_module(bench)
OUT=ROOT/'artifacts/benchmark-final/core-paired-6053.json'
assert not OUT.exists(),'Do not overwrite an earlier measurement'
before=ROOT/'artifacts/benchmark-final/before-zterm-benchmark'
after=ROOT/'artifacts/benchmark-final/after-zterm-benchmark'
expected={'before':'4b3f5d75d409cb697d46a41937e87d2123a59b67127dd2178c1e115423ee0d5d','after':'7c645edd3c7fe9feae80386c93be856003b1b938f8ad20b5f47dd0898b64d652'}
binaries={'before':before,'after':after}
head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()
report={'schema':1,'ok':False,'scope':'parser and screen model, not PTY or renderer or physical input latency','timestamp_utc':datetime.datetime.now(datetime.timezone.utc).isoformat(),'collector_source':head,'collector_sha256':hashlib.sha256(pathlib.Path(__file__).read_bytes()).hexdigest(),'before_source':'155fea51aa138b0503ea1d80be4252aacb2c61a7','after_application_build_context':'7544a0d7a6d01bfe149d743ed3147baff188d67a','after_application_consumer_source':'6053a97920c29c852cd92d8b96e35f62cff92ecb','after_library_source':'13d5e2c74571d35cccd920b1204798211b70c79f','binary_sha256':expected,'cpu':23,'nice':os.getpriority(os.PRIO_PROCESS,0),'profile':{'opt_level':3,'lto':'fat','codegen_units':1,'panic':'abort','strip':'debuginfo'},'example_sha256':'8551f815d1ca276154df9f2050a9039f0e9cd225da363137338d3c820ecc3624','runs':[],'status':'started'}
def save():OUT.write_text(json.dumps(report,indent=2)+'\n')
try:
 assert 23 in os.sched_getaffinity(0)
 for key,path in binaries.items():assert hashlib.sha256(path.read_bytes()).hexdigest()==expected[key]
 for rep in range(3):
  for key in (['before','after'] if rep%2==0 else ['after','before']):
   pressure=bench.host_pressure();reasons=bench.comparison_pressure_reasons(pressure)
   if reasons:
    report.update(status='deferred',defer_reasons=reasons,host_at_defer=pressure);save();raise SystemExit(2)
   command=['taskset','-c','23',str(binaries[key])]
   start=time.time();result=subprocess.run(command,text=True,capture_output=True,timeout=90)
   record={'side':key,'round':rep+1,'command':command,'exit_code':result.returncode,'elapsed_seconds':time.time()-start,'host_before':pressure,'host_after':bench.host_pressure(),'stderr':result.stderr}
   record['result']=json.loads(result.stdout) if result.returncode==0 else {'stdout':result.stdout}
   record['host_contention_after']=bench.comparison_pressure_reasons(record['host_after'])
   report['runs'].append(record);save()
   assert result.returncode==0
   if record['host_contention_after']:
    report.update(status='contended',defer_reasons=record['host_contention_after']);save();raise SystemExit(2)
   print(json.dumps({'side':key,'round':rep+1,'ok':True}),flush=True)
 keys=['ascii_mib_s','ascii_full_history_mib_s','sgr_mib_s','unicode_mib_s','initial_core_heap_bytes','steady_state_allocations']
 report['summary']={}
 for key in keys:
  a=[r['result'][key] for r in report['runs'] if r['side']=='before'];b=[r['result'][key] for r in report['runs'] if r['side']=='after']
  ma=statistics.median(a);mb=statistics.median(b)
  report['summary'][key]={'before_median':ma,'after_median':mb,'after_over_before':mb/ma if ma else None,'before_samples':a,'after_samples':b}
 for key,path in binaries.items():assert hashlib.sha256(path.read_bytes()).hexdigest()==expected[key]
 assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=ROOT,text=True).strip()==head
 report.update(ok=True,status='complete');save()
 print(json.dumps({'ok':True,'summary':report['summary']}),flush=True)
except Exception as error:
 report.update(status='failed',error=repr(error));save();raise
