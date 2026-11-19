#!/usr/bin/env python3
"""Build the test-only input-method-v2 peer from the pinned protocol."""
import argparse
import os
from pathlib import Path
import shlex
import subprocess

parser = argparse.ArgumentParser()
parser.add_argument("--output", required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parent
out = Path(args.output).resolve()
out.mkdir(parents=True, exist_ok=True)
xml = root / "protocols/input-method-unstable-v2.xml"
header = out / "input-method-client-protocol.h"
generated = out / "input-method-protocol.c"
subprocess.run(["wayland-scanner", "client-header", str(xml), str(header)], check=True)
subprocess.run(["wayland-scanner", "private-code", str(xml), str(generated)], check=True)
flags = shlex.split(subprocess.check_output(
    ["pkg-config", "--cflags", "--libs", "wayland-client"], text=True))
subprocess.run([os.environ.get("CC", "cc"), "-std=c11", "-O2",
                "-Wall", "-Wextra", "-Werror", "-I", str(out),
                str(root / "wayland_ime_peer.c"), str(generated), *flags,
                "-o", str(out / "ime-peer")], check=True)
print(out / "ime-peer")
