#!/usr/bin/env python3
"""Compile actual native clipboard allocation and lifetime checks."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shlex
import subprocess
import time
parser = argparse.ArgumentParser()
parser.add_argument("--output", required=True)
parser.add_argument("--sanitize", action="store_true")
args = parser.parse_args()
root = Path(__file__).resolve().parents[1]
out = Path(args.output).resolve()
out.mkdir(parents=True, exist_ok=True)
protocols = Path(subprocess.check_output(
    ["pkg-config", "--variable=pkgdatadir", "wayland-protocols"], text=True).strip())
generated = []
commands = []
def run(argv, **kwargs):
    commands.append(argv)
    return subprocess.run(argv, check=True, **kwargs)
for name, relative in [
    ("primary-selection", "unstable/primary-selection/primary-selection-unstable-v1.xml"),
    ("text-input", "unstable/text-input/text-input-unstable-v3.xml"),
]:
    xml = protocols / relative
    header = out / (name + "-client-protocol.h")
    source = out / (name + "-protocol.c")
    run(["wayland-scanner", "client-header", str(xml), str(header)])
    run(["wayland-scanner", "private-code", str(xml), str(source)])
    generated.append(str(source))
flags = shlex.split(subprocess.check_output(
    ["pkg-config", "--cflags", "--libs", "wayland-client"], text=True))
binary = out / "wayland-services-budget"
command = [os.environ.get("CC", "cc"), "-std=c11", "-O1", "-g",
           "-Wall", "-Wextra", "-Werror", "-I", str(out),
           str(root / "tests/wayland_services_budget.c"), *generated, *flags,
           "-o", str(binary)]
if args.sanitize:
    command.extend(["-fsanitize=address,undefined", "-fno-omit-frame-pointer"])
run(command)
result = run([str(binary)], capture_output=True, text=True)
sources = {name: hashlib.sha256((root / name).read_bytes()).hexdigest() for name in [
    "native/wayland_services.c", "native/wayland_services.h",
    "tests/wayland_services_budget.c", "tests/wayland_services.py"]}
report = {"passed": True, "sanitizers": args.sanitize, "commands": commands,
          "source_sha256": sources, "stdout": result.stdout, "stderr": result.stderr,
          "finished": time.time()}
(out / "validation.json").write_text(json.dumps(report, indent=2))
print(result.stdout, end="")
