#!/usr/bin/env python3
import datetime, hashlib, json, os, pathlib, re, shutil, subprocess, sys, time
ROOT=pathlib.Path("WORKTREE")
OUT=ROOT/"artifacts/integration-validation-zterm051-20261120/default-egl"
SOURCE="9f5a77a1299093d0ba609bfafa2cec3b0ba931dd"
LOCK="b86e3438a14ff8710a28b09bafa33ab1861f8cf116b722dc94be83133d4543b5"
CORE="a2074bac4c030cfd7c0b2514c852a89dcf54648f"
WAYLAND=ROOT/"artifacts/final-elf-zterm051-20261120/ztty-9f5a77a-wayland"
WAYLAND_SHA="c7935e9e76476a9cd7d0186f23f5806e3e2e2fc556a494b0e3741a2c893b3807"
DEFAULT=WAYLAND.with_name("ztty-9f5a77a-default")
EXPECTED={
"gpu::tests::native_fonts_shape_joining_scripts_and_complete_emoji_clusters",
"gpu::tests::font_reload_keeps_the_surface_and_image_cache_and_updates_ui_metrics",
"gpu::tests::image_texture_coordinates_preserve_every_column_at_four_k_width",
"gpu::tests::egl_composites_clipping_alpha_and_replaced_images_with_exact_pixels",
"gpu::tests::shared_display_survives_interleaved_windows_resize_and_context_destruction",
"gpu::tests::shaped_terminal_cells_render_whole_clusters_and_reuse_the_bounded_atlas"}
def stamp():return datetime.datetime.now(datetime.timezone.utc).isoformat()
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def git(*args):return subprocess.check_output(["git",*args],cwd=ROOT,text=True).strip()
def source():
    assert git("rev-parse","HEAD")==SOURCE
    assert git("status","--porcelain")==""
    assert sha(ROOT/"Cargo.lock")==LOCK
    assert git("rev-parse","HEAD:crates/zterm")==CORE
    assert git("-C","crates/zterm","rev-parse","HEAD")==CORE
    assert git("-C","crates/zterm","status","--porcelain")==""
    assert sha(WAYLAND)==WAYLAND_SHA
    return {"root":SOURCE,"zterm":CORE,"cargo_lock_sha256":LOCK,"wayland_sha256":WAYLAND_SHA}
def cgroup():
    rel=next(line.split("::",1)[1] for line in pathlib.Path("/proc/self/cgroup").read_text().splitlines() if line.startswith("0::"))
    return pathlib.Path("/sys/fs/cgroup")/rel.lstrip("/")
CG=cgroup()
def cg(name):return (CG/name).read_text().strip()
def save():
    tmp=OUT/"default-egl.json.tmp";tmp.write_text(json.dumps(record,indent=2)+"\n");tmp.replace(OUT/"default-egl.json")
def run(name,command):
    path=OUT/(name+".log")
    record["steps"].append({"name":name,"command":command,"started_utc":stamp(),"log":str(path)})
    save();print("START "+name,flush=True)
    with path.open("x") as log:
        result=subprocess.run(command,cwd=ROOT,env=env,stdout=log,stderr=subprocess.STDOUT)
    step=record["steps"][-1];step.update(exit_code=result.returncode,finished_utc=stamp(),log_sha256=sha(path))
    compact=subprocess.run(["rtk","log",str(path)],capture_output=True,text=True)
    (OUT/(name+".rtk.log")).write_text(compact.stdout+compact.stderr)
    print("\n".join(compact.stdout.splitlines()[-12:]),flush=True)
    save()
    if result.returncode:raise RuntimeError(name+" failed: "+str(result.returncode))
    source()
    return path.read_text(errors="replace")
record={"schema":1,"source":SOURCE,"zterm":CORE,"started_utc":stamp(),"status":"preflight","passed":False,"steps":[],"script_sha256":sha(pathlib.Path(__file__)),"scope":"Default-feature release ELF plus the six explicitly ignored headless EGL/native-font tests; no performance measurements"}
env=dict(os.environ,CARGO_HOME=str(ROOT/"artifacts/cargo-home-20261120"),CARGO_TARGET_DIR=str(ROOT/"target"),RUSTUP_TOOLCHAIN="1.100.0",RUSTC_WRAPPER="",CARGO_BUILD_JOBS="1")
try:
    assert not DEFAULT.exists(),"Refuse to overwrite a frozen default ELF"
    active=subprocess.run(["systemctl","--user","show","ztty-final-core-9f5a77a-20261120.service","--property=ActiveState","--value"],capture_output=True,text=True)
    assert active.returncode==0 and active.stdout.strip() in ("inactive","failed"),active.stdout+active.stderr
    record["root_gate_unit"]=active.stdout.strip()
    record["limits"]={name:cg(name) for name in ("memory.max","memory.swap.max","cpu.max")}
    record["nice"]=os.getpriority(os.PRIO_PROCESS,0)
    assert record["limits"]=={"memory.max":"805306368","memory.swap.max":"0","cpu.max":"100000 100000"}
    assert record["nice"]==10
    record["source_before"]=source()
    record["root_gate_receipt_sha256"]=sha(OUT.parent/"local-gate.json")
    record["environment"]={key:env[key] for key in ("CARGO_HOME","CARGO_TARGET_DIR","RUSTUP_TOOLCHAIN","RUSTC_WRAPPER","CARGO_BUILD_JOBS")}
    record["rustc"]=subprocess.check_output(["rustc","-vV"],cwd=ROOT,env=env,text=True)
    record["status"]="running";save()
    run("default-build",["cargo","build","--locked","--offline","--release","--bin","ztty","--no-default-features","-j","1"])
    original=ROOT/"target/release/ztty"
    digest=sha(original)
    assert digest!=WAYLAND_SHA,"Default and feature-enabled ELFs must differ"
    with original.open("rb") as src,DEFAULT.open("xb") as dst:shutil.copyfileobj(src,dst)
    DEFAULT.chmod(original.stat().st_mode & 0o777)
    assert sha(DEFAULT)==digest
    record["default_binary"]={"path":str(DEFAULT),"sha256":digest,"bytes":DEFAULT.stat().st_size,"features":[]}
    save()
    log=run("egl-tests",["cargo","test","--locked","--offline","--lib","--no-default-features","-j","1","gpu::tests::","--","--ignored","--test-threads=1","--nocapture"])
    passed=set(re.findall(r"^test (gpu::tests::\S+) \.\.\. ok$",log,re.M))
    assert passed==EXPECTED,{"expected":sorted(EXPECTED),"observed":sorted(passed)}
    assert re.search(r"test result: ok\. 6 passed; 0 failed; 0 ignored;",log)
    record["egl_tests"]={"count":6,"passed":sorted(passed)}
    record["source_after"]=source()
    assert sha(DEFAULT)==record["default_binary"]["sha256"]
    record.update(status="complete",passed=True)
except BaseException as error:
    record.update(status="failed",error=repr(error));raise
finally:
    record["finished_utc"]=stamp()
    record["memory_peak_bytes"]=int(cg("memory.peak"))
    record["memory_events"]=cg("memory.events")
    record["swap_peak_bytes"]=int(cg("memory.swap.peak"))
    save()
    print(json.dumps({"passed":record["passed"],"status":record["status"],"receipt":str(OUT/"default-egl.json"),"default_binary":record.get("default_binary")}),flush=True)
