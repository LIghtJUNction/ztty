#!/usr/bin/env python3
"""Build independently linked example/fault libraries for plugin_cli.py and plugin_gui.py."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import time

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=ROOT / "artifacts/zplugin")
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    env = dict(os.environ, CARGO_BUILD_JOBS="1")
    report = {"ok": False, "steps": []}
    cases = [("example", "examples/dedent-plugin", "ztty_dedent_plugin", "")]
    cases += [("fixture-" + (feature or "valid"), "tests/fixtures/text-plugin", "ztty_text_plugin_fixture", feature)
              for feature in ("", "wrong-interface", "pinned")]
    try:
        for name, directory, crate, feature in cases:
            command = ["cargo", "build", "--locked", "--release", "--manifest-path",
                       directory + "/Cargo.toml", "--message-format=json-render-diagnostics"]
            if feature:
                command += ["--features", feature]
            started = time.monotonic()
            artifacts_path = output / (name + "-artifacts.jsonl")
            with (output / (name + "-build.log")).open("w") as log, artifacts_path.open("w") as artifacts:
                result = subprocess.run(command, cwd=ROOT, env=env, stdout=artifacts, stderr=log)
            step = {"name": name, "command": command, "exit_code": result.returncode,
                    "duration_ms": (time.monotonic() - started) * 1000}
            report["steps"].append(step)
            if result.returncode:
                raise RuntimeError(name + " build failed")
            library = output / (name + ".so")
            with artifacts_path.open() as artifacts:
                sources = {
                    Path(filename)
                    for item in map(json.loads, artifacts)
                    if item.get("reason") == "compiler-artifact"
                    and item.get("target", {}).get("name") == crate
                    and "cdylib" in item["target"].get("crate_types", [])
                    for filename in item["filenames"]
                    if filename.endswith(".so")
                }
            if len(sources) != 1:
                raise RuntimeError(name + " did not produce exactly one native plugin library")
            source = sources.pop()
            shutil.copy2(source, library)
            step.update(source=str(source), path=str(library),
                        sha256=hashlib.sha256(library.read_bytes()).hexdigest())
        report["ok"] = True
    except Exception as error:
        report["error"] = str(error)
    (output / "libraries.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "output": str(output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
