#!/usr/bin/env python3
"""Create a Windows archive with a pinned official ConPTY redistributable.

The Rust executable is supplied by the caller. This command never compiles,
signs, publishes, modifies release workflows, or searches for credentials.
"""

import argparse
import hashlib
import io
import json
from pathlib import Path
import struct
import tempfile
import tomllib
import urllib.request
import zipfile


CONPTY_VERSION = "1.25.261109001"
CONPTY_SHA256 = "3d77f535df848e44225c2a8912ac844fecb23f3057732b639f3034b587496f09"
CONPTY_URL = (
    "https://api.nuget.org/v3-flatcontainer/microsoft.windows.console.conpty/"
    f"{CONPTY_VERSION}/microsoft.windows.console.conpty.{CONPTY_VERSION}.nupkg"
)
MIT_LICENSE = """MIT License

Copyright (c) Microsoft Corporation.

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""


def digest(data):
    return hashlib.sha256(data).hexdigest()


def check_pe(data, expected_machine, label):
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise ValueError(f"{label}: not a PE executable")
    offset, = struct.unpack_from("<I", data, 0x3C)
    if offset + 6 > len(data) or data[offset:offset + 4] != b"PE\0\0":
        raise ValueError(f"{label}: invalid PE header")
    machine, = struct.unpack_from("<H", data, offset + 4)
    if machine != expected_machine:
        raise ValueError(f"{label}: machine 0x{machine:04x} does not match 0x{expected_machine:04x}")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--arch", choices=["x64", "arm64"], default="x64")
    parser.add_argument("--package", type=Path,
                        help="reuse an already downloaded nupkg; the pinned hash is still required")
    args = parser.parse_args()
    root = Path(__file__).resolve().parent.parent
    version = tomllib.loads((root / "Cargo.toml").read_text(encoding="utf-8"))["package"]["version"]
    machine = {"x64": 0x8664, "arm64": 0xAA64}[args.arch]
    binary = args.binary.read_bytes()
    check_pe(binary, machine, "ztty.exe")
    if args.package:
        package = args.package.read_bytes()
    else:
        with urllib.request.urlopen(CONPTY_URL, timeout=30) as response:
            package = response.read(8 * 1024 * 1024 + 1)
    if digest(package) != CONPTY_SHA256:
        raise ValueError("ConPTY package SHA-256 does not match the pinned official package")
    with zipfile.ZipFile(io.BytesIO(package)) as archive:
        conpty = archive.read(f"runtimes/win-{args.arch}/native/conpty.dll")
        console = archive.read(f"build/native/runtimes/{args.arch}/OpenConsole.exe")
        nuspec = archive.read("Microsoft.Windows.Console.ConPTY.nuspec").decode("utf-8-sig")
    if f"<version>{CONPTY_VERSION}</version>" not in nuspec:
        raise ValueError("ConPTY package version differs from its pin")
    check_pe(conpty, machine, "conpty.dll")
    check_pe(console, machine, "OpenConsole.exe")
    files = {
        "ztty.exe": binary,
        "conpty.dll": conpty,
        "OpenConsole.exe": console,
        "LICENSE-ztty.txt": (root / "LICENSE").read_bytes(),
        "LICENSE-ConPTY.txt": MIT_LICENSE.encode("utf-8"),
    }
    manifest = {
        "ztty_version": version,
        "architecture": args.arch,
        "minimum_windows_build": 17763,
        "conpty": {"package": "Microsoft.Windows.Console.ConPTY",
                   "version": CONPTY_VERSION, "url": CONPTY_URL,
                   "package_sha256": CONPTY_SHA256, "license": "MIT"},
        "files": {name: {"bytes": len(data), "sha256": digest(data)} for name, data in files.items()},
        "validation": "Packaging verifies hashes and PE architecture; native runtime tests are separate.",
    }
    files["manifest.json"] = (json.dumps(manifest, indent=2) + "\n").encode("utf-8")
    args.output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(prefix=".ztty-windows-", suffix=".zip", dir=args.output.parent,
                                     delete=False) as temporary:
        temporary_path = Path(temporary.name)
    try:
        with zipfile.ZipFile(temporary_path, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for name, data in files.items():
                entry = zipfile.ZipInfo(name, date_time=(2026, 11, 18, 0, 0, 0))
                entry.compress_type = zipfile.ZIP_DEFLATED
                entry.external_attr = 0o100644 << 16
                archive.writestr(entry, data)
        with zipfile.ZipFile(temporary_path) as archive:
            if archive.testzip() is not None:
                raise ValueError("archive verification failed")
            for name, data in files.items():
                if digest(archive.read(name)) != digest(data):
                    raise ValueError(f"archive round trip changed {name}")
        temporary_path.replace(args.output)
    finally:
        temporary_path.unlink(missing_ok=True)
    print(json.dumps({"output": str(args.output.resolve()), "bytes": args.output.stat().st_size,
                      "sha256": digest(args.output.read_bytes()), "manifest": manifest}, indent=2))


if __name__ == "__main__":
    main()
