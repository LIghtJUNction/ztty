from pathlib import Path
import hashlib,json,subprocess,time
base=Path('artifacts/copy-usability')
out=base/'final'
out.mkdir(parents=True,exist_ok=True)
binary=Path('target/release/ztty').resolve()
runs=[
 ('release-build',['cargo','build','--locked','--release']),
 ('copy-paste-gui',['python3','tests/copy_paste_gui.py','--binary',str(binary),'--artifacts',str(out/'gui')]),
 ('clipboard-transfer',['python3','tests/clipboard_transfer_gui.py','--binary',str(binary),'--output',str(out/'clipboard-transfer.json')]),
 ('daily-workflow',['python3','tests/daily_workflow.py','--binary',str(binary),'--output',str(out/'daily-workflow.json'),'--screenshots',str(out/'daily-workflow')]),
]
report={'source_commit':subprocess.check_output(['git','rev-parse','HEAD'],text=True).strip(),'source_changes':subprocess.check_output(['git','status','--porcelain'],text=True).splitlines(),'steps':[],'ok':False}
summary=out/'validation.json'
for name,command in runs:
 started=time.time()
 step={'name':name,'command':command,'started_unix':started,'state':'running'}
 report['steps'].append(step)
 summary.write_text(json.dumps(report,indent=2)+'\n')
 with (out/(name+'.log')).open('wb') as log:
  try:
   result=subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,timeout=600 if name=='release-build' else 240)
   step.update(exit_code=result.returncode,state='completed',elapsed_seconds=round(time.time()-started,3))
  except Exception as exc:
   step.update(state='failed',error=repr(exc),elapsed_seconds=round(time.time()-started,3))
   summary.write_text(json.dumps(report,indent=2)+'\n')
   raise
 if name=='release-build' and result.returncode==0:
  report['binary_sha256']=hashlib.sha256(binary.read_bytes()).hexdigest()
 summary.write_text(json.dumps(report,indent=2)+'\n')
 if result.returncode:
  raise SystemExit(result.returncode)
report['ok']=True
summary.write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps(report,indent=2))
