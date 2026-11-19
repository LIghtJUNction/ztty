#!/usr/bin/env python3
import argparse, hashlib, json, os, statistics, subprocess, sys, time
from pathlib import Path
p=argparse.ArgumentParser()
p.add_argument('--candidate',required=True)
p.add_argument('--candidate-source',required=True)
p.add_argument('--output',required=True)
p.add_argument('--pairs',type=int,default=5)
p.add_argument('--mib',type=int,default=8)
p.add_argument('--samples',type=int,default=3)
a=p.parse_args()
out=Path(a.output);out.mkdir(parents=True,exist_ok=True)
versions={'baseline':('artifacts/output-throughput/baseline/ztty','db4253054dfad84fb1d5af64efd3d9efc4e048d1'),'candidate':(a.candidate,a.candidate_source)}
report={'ok':False,'pairs':a.pairs,'mib':a.mib,'samples':a.samples,'started_unix':time.time(),'runner_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),'runs':[]}
def save(): (out/'raw.json').write_text(json.dumps(report,ensure_ascii=False,indent=2)+'\n')
save()
for pair in range(a.pairs):
 for version in (['baseline','candidate'] if pair%2==0 else ['candidate','baseline']):
  binary,source=versions[version]
  path=out/f'{pair}-{version}.json'
  command=[sys.executable,'tools/benchmark_output_pipeline.py','--binary',binary,'--binary-source',source,'--output',str(path),'--mib',str(a.mib),'--samples',str(a.samples)]
  with path.with_suffix('.log').open('w') as log:
   result=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT)
  if result.returncode:
   report['failed']={'pair':pair,'version':version,'exit_code':result.returncode,'path':str(path)};save();sys.exit(result.returncode)
  data=json.loads(path.read_text());assert data['ok']
  report['runs'].append({'pair':pair,'version':version,'data':data});save()
  print(pair,version,'complete',flush=True)
summary=[]
for kind in report['runs'][0]['data']['settings']['kinds']:
 for mode in ['headless','window']:
  row={'kind':kind,'mode':mode}
  for version in versions:
   runs=[next(x for x in r['data']['runs'] if x['kind']==kind and x['mode']==mode) for r in report['runs'] if r['version']==version]
   med=[x['median_mib_s'] for x in runs]
   row[version]={'mib_s':statistics.median(med),'range_mib_s':[min(med),max(med)],'run_medians_mib_s':med,'idle_dsr_p50_ns':statistics.median(x['idle_dsr_p50_ns'] for x in runs),'idle_dsr_p95_ns':statistics.median(x['idle_dsr_p95_ns'] for x in runs)}
  row['ratio']=row['candidate']['mib_s']/row['baseline']['mib_s'];summary.append(row)
report['ok']=True;report['finished_unix']=time.time();save()
(out/'summary.json').write_text(json.dumps({'ok':True,'scope':'median of per-run throughput medians; range of those medians, not confidence intervals','rows':summary},indent=2)+'\n')
print(json.dumps(summary,indent=2))
