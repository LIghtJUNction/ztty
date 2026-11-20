#!/usr/bin/env python3
"""Run only changed zterm and six-crate registry graphs using the tracked verifier."""
import datetime, hashlib, importlib.util, inspect, json, os, subprocess, sys, time, types
from pathlib import Path

ROOT = Path("WORKTREE")
OUT = Path(__file__).resolve().parent
SOURCE = "9f5a77a1299093d0ba609bfafa2cec3b0ba931dd"
LOCK = "b86e3438a14ff8710a28b09bafa33ab1861f8cf116b722dc94be83133d4543b5"
TOOL_HASH = "b1bae659a7f125309f8060e9cb3d8894a14665651770cbe3f5af18223925f51c"
PRIOR = ROOT / "artifacts/registry-consumers-36b367f-20261120/result.json"
TOOL = ROOT / "tools/verify_registry.py"
def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
def git(*args):
    return subprocess.check_output(["git", *args], cwd=ROOT, text=True).strip()
def snapshot():
    return {"source": git("rev-parse", "HEAD"), "status": git("status", "--porcelain"),
            "lock_sha256": sha(ROOT / "Cargo.lock"), "tool_sha256": sha(TOOL)}
def verify_snapshot(value):
    assert value == {"source": SOURCE, "status": "", "lock_sha256": LOCK, "tool_sha256": TOOL_HASH}, value
def save(name, value):
    path = OUT / name
    path.write_text(json.dumps(value, indent=2, ensure_ascii=False) + "\n")
def resources():
    path = next(line.split(":", 2)[2] for line in Path("/proc/self/cgroup").read_text().splitlines() if line.startswith("0::"))
    base = Path("/sys/fs/cgroup") / path.lstrip("/")
    return {"cgroup": path, "nice": os.getpriority(os.PRIO_PROCESS, 0),
            "files": {name: (base / name).read_text().strip() if (base / name).exists() else None
                      for name in ("cpu.max", "memory.max", "memory.swap.max", "memory.peak", "memory.swap.peak", "memory.events")}}
started = time.monotonic()
result = {"schema": 1, "source": SOURCE, "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
          "scope": "Changed-graph verification only: one zterm consumer plus one combined six-crate consumer; six Cargo commands.",
          "historical_full_six_individual_plus_combined": {"path": str(PRIOR), "sha256": sha(PRIOR),
              "source": "36b367ffda4cea9b3b3dfbf38dc6375567912323", "repeated_in_this_run": False},
          "runner_sha256": sha(Path(__file__)), "tool_sha256": sha(TOOL), "ok": False,
          "individual_checks": [], "combined_check": None, "raw_commands": []}
save("result.json", result)
try:
    result["before"] = snapshot()
    verify_snapshot(result["before"])
    result["resources_before"] = resources()
    assert os.environ["CARGO_HOME"] == str(ROOT / "artifacts/cargo-home-20261120")
    assert Path(os.environ["CARGO_TARGET_DIR"]).resolve() == OUT / "target"
    assert os.environ["RUSTC_WRAPPER"] == "" and os.environ["CARGO_BUILD_JOBS"] == "1"
    spec = importlib.util.spec_from_file_location("tracked_registry_verifier", TOOL)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    result["versions"] = module.VERSIONS
    result["function_sha256"] = {name: hashlib.sha256(inspect.getsource(getattr(module, name)).encode()).hexdigest()
                                 for name in ("check", "library", "executable")}
    previous = json.loads(PRIOR.read_text())
    assert previous["ok"]
    expected = {p["name"]: p for p in previous["packages"]}
    expected["zterm"] = dict(expected["zterm"], version="0.5.1",
                             vcs={"git": {"sha1": "a2074bac4c030cfd7c0b2514c852a89dcf54648f"}, "path_in_vcs": ""},
                             cargo_lock_checksum="fc48bcaf9892a77d17ab235316448c3e235b0dccd00e5963b542586585c26c91")
    assert module.VERSIONS == {k: v["version"] for k, v in expected.items()}
    assert expected["ztty-plugin-api"]["vcs"]["git"]["sha1"] == "36b367ffda4cea9b3b3dfbf38dc6375567912323"
    original_run = subprocess.run
    def logged_run(command, *args, **kwargs):
        index = len(result["raw_commands"]) + 1
        record = {"index": index, "command": command, "cwd": str(kwargs.get("cwd")), "timeout": kwargs.get("timeout")}
        result["raw_commands"].append(record)
        begin = time.monotonic()
        try:
            completed = original_run(command, *args, **kwargs)
            stdout, stderr = completed.stdout or "", completed.stderr or ""
            record["exit_code"] = completed.returncode
        except subprocess.TimeoutExpired as error:
            stdout, stderr = error.stdout or "", error.stderr or ""
            record["timeout_expired"] = True
            raise
        finally:
            record["elapsed_seconds"] = time.monotonic() - begin
            for stream, data in (("stdout", locals().get("stdout", "")), ("stderr", locals().get("stderr", ""))):
                raw = data if isinstance(data, bytes) else data.encode()
                path = OUT / f"{index:02d}-{stream}.log"
                path.write_bytes(raw)
                record[stream] = {"path": str(path), "bytes": len(raw), "sha256": sha(path)}
            save("result.json", result)
        return completed
    # Observation only: delegate the original subprocess invocation unchanged.
    module.subprocess = types.SimpleNamespace(run=logged_run, TimeoutExpired=subprocess.TimeoutExpired)
    checked = module.check(["zterm"], OUT / "consumers/zterm", module.DEFAULT_CARGO_TIMEOUT)
    result["individual_checks"].append(checked)
    save("result.json", result)
    print(json.dumps({"graph": "zterm", "ok": checked["ok"]}), flush=True)
    if not checked["ok"]:
        raise RuntimeError("zterm registry graph failed; no retry")
    combined = module.check(list(module.PACKAGES), OUT / "consumers/combined", module.DEFAULT_CARGO_TIMEOUT)
    result["combined_check"] = combined
    save("result.json", result)
    print(json.dumps({"graph": "combined", "ok": combined["ok"]}), flush=True)
    assert combined["ok"], combined.get("error")
    assert len(result["raw_commands"]) == 6
    for check in [checked, combined]:
        for actual in check["packages"]:
            prior = expected[actual["name"]]
            for key in ("version", "source", "vcs", "cargo_lock_checksum", "library_source_has_no_std"):
                assert actual[key] == prior[key], (actual["name"], key, actual[key], prior[key])
    result["consumer_files"] = {}
    for folder in (OUT / "consumers/zterm", OUT / "consumers/combined"):
        for relative in ("Cargo.toml", "Cargo.lock", "src/lib.rs", "src/main.rs"):
            path = folder / relative
            result["consumer_files"][str(path.relative_to(OUT))] = sha(path)
    result["after"] = snapshot()
    verify_snapshot(result["after"])
    assert sha(PRIOR) == result["historical_full_six_individual_plus_combined"]["sha256"]
    result["ok"] = True
except Exception as error:
    result["error"] = repr(error)
finally:
    result["elapsed_seconds"] = time.monotonic() - started
    result["resources_after"] = resources()
    save("result.json", result)
    print(json.dumps({"ok": result["ok"], "output": str(OUT / "result.json"),
                      "elapsed_seconds": result["elapsed_seconds"], "error": result.get("error")}), flush=True)
sys.exit(0 if result["ok"] else 1)
