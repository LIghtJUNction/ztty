import pathlib,subprocess,json,hashlib,time,os,sys,shutil
w=pathlib.Path.cwd();a=w/"artifacts";out=a/"release-build-print-batch";out.mkdir()
root=w.with_name("resume-20261120");published=w.with_name("zterm-20261120")
source="28d5d266375ff67562e16652a3eef10bd36b4756"
def digest(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def git(*args):return subprocess.check_output(["git",*args],cwd=w,text=True).strip()
assert git("rev-parse","HEAD")==source and not git("status","--porcelain")
assert os.environ.get("RUSTC_WRAPPER")==""
assert os.environ.get("CARGO_BUILD_JOBS")=="1"
raw=root/"artifacts/benchmark-final/core-paired-6053.json";raw_hash=digest(raw)
protected=[a/"core-three-way-0.5.1/comparison.json",a/"unicode-profile-85f/gdb/profile.json",a/"zterm-0.5.1-append-benchmark-bounded"]
protected += [a/"core-three-way-a2074ba/comparison.json",a/"zterm-0.5.1-borrowed-benchmark"]
protected_hashes={str(p):digest(p) for p in protected}
cg=pathlib.Path("/sys/fs/cgroup")/next(x.split("::",1)[1] for x in pathlib.Path("/proc/self/cgroup").read_text().splitlines() if x.startswith("0::")).lstrip("/")
assert (cg/"memory.max").read_text().strip()=="805306368" and (cg/"memory.swap.max").read_text().strip()=="0"
cfg=["--config",'profile.release.lto="fat"',"--config","profile.release.codegen-units=1","--config",'profile.release.panic="abort"',"--config",'profile.release.strip="debuginfo"']
cmd=["cargo","build","--locked","--offline","--release","--example","benchmark",*cfg]
r={"source":source,"version":"0.5.1","command":cmd,"benchmarks_executed":False,"profile":{"opt_level":3,"lto":"fat","codegen_units":1,"panic":"abort","strip":"debuginfo"},"start":time.time(),"cargo_home":os.environ["CARGO_HOME"],"target":os.environ["CARGO_TARGET_DIR"],"rustc_wrapper":os.environ["RUSTC_WRAPPER"],"rustc":subprocess.check_output(["rustc","-vV"],text=True)}
assert r["rustc"]==json.loads((root/"artifacts/benchmark-final/after-core-build.json").read_text())["rustc"]
r["script_sha256"]=digest(pathlib.Path(__file__))
r["protected_before"]=protected_hashes
r["example_sha256"]=digest(w/"examples/benchmark.rs")
assert r["example_sha256"]=="8551f815d1ca276154df9f2050a9039f0e9cd225da363137338d3c820ecc3624"
(out/"build.json").write_text(json.dumps(r,indent=2)+"\n")
try:
 with (out/"build.raw.log").open("w") as f:p=subprocess.run(cmd,stdout=f,stderr=subprocess.STDOUT)
 r["exit_code"]=p.returncode
 with (out/"build.rtk.log").open("w") as f:q=subprocess.run(["rtk","log",str(out/"build.raw.log")],stdout=f,stderr=subprocess.STDOUT)
 r["rtk_exit_code"]=q.returncode
 assert p.returncode==0
 binary=pathlib.Path(os.environ["CARGO_TARGET_DIR"])/"release/examples/benchmark"
 frozen=a/"zterm-0.5.1-print-batch-benchmark"
 assert binary.is_file() and not frozen.exists()
 shutil.copy2(binary,frozen);frozen.chmod(0o555)
 r.update(binary=str(frozen),binary_sha256=digest(frozen),binary_bytes=frozen.stat().st_size,passed=True)
except BaseException as e:r.update(passed=False,error=repr(e))
finally:
 r["source_after"]=git("rev-parse","HEAD");r["clean_after"]=not git("status","--porcelain")
 r["source_identity_preserved"]=r["source_after"]==source and r["clean_after"]
 r["memory_peak_bytes"]=int((cg/"memory.peak").read_text());r["memory_events"]=(cg/"memory.events").read_text();r["swap_peak_bytes"]=int((cg/"memory.swap.peak").read_text())
 r["elapsed_seconds"]=time.time()-r["start"];r["original_benchmark_unchanged"]=raw_hash==digest(raw)
 r["protected_unchanged"]=protected_hashes=={str(p):digest(p) for p in protected}
 r["published_head"]=subprocess.check_output(["git","-C",str(published),"rev-parse","HEAD"],text=True).strip()
 r["published_clean"]=not subprocess.check_output(["git","-C",str(published),"status","--porcelain"],text=True)
 r["passed"]=bool(r.get("passed") and r["source_identity_preserved"] and r["original_benchmark_unchanged"] and r["protected_unchanged"] and r["published_head"]=="13d5e2c74571d35cccd920b1204798211b70c79f" and r["published_clean"])
 (out/"build.json").write_text(json.dumps(r,indent=2)+"\n")
print(json.dumps({k:v for k,v in r.items() if k!="rustc"}),flush=True)
sys.exit(0 if r["passed"] else 1)
