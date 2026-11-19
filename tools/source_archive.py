#!/usr/bin/env python3
"""Create a reproducible source archive including exact, initialized submodules."""
from __future__ import annotations

import argparse
import copy
import gzip
import io
import os
from pathlib import Path, PurePosixPath
import subprocess
import tarfile
import tempfile


def git(root: Path, *args: str) -> bytes:
    return subprocess.check_output(["git", "-C", str(root), *args], stderr=subprocess.PIPE)


def archives(root: Path, revision: str, prefix: str = "", depth: int = 0):
    if depth > 8:
        raise RuntimeError("submodule nesting exceeds eight levels")
    yield prefix, git(root, "archive", "--format=tar", revision)
    for entry in git(root, "ls-tree", "-rz", revision).split(b"\0"):
        if not entry:
            continue
        meta, raw_path = entry.split(b"\t", 1)
        mode, _, raw_sha = meta.split()
        if mode != b"160000":
            continue
        path = raw_path.decode("utf-8")
        relative = PurePosixPath(path)
        if relative.is_absolute() or ".." in relative.parts:
            raise RuntimeError("unsafe submodule path")
        module = root / path
        if not module.is_dir() or not (module / ".git").exists():
            raise RuntimeError(f"submodule is not initialized: {prefix}{path}")
        if not module.resolve().is_relative_to(root.resolve()):
            raise RuntimeError(f"submodule escapes its checkout: {prefix}{path}")
        expected = raw_sha.decode("ascii")
        actual = git(module, "rev-parse", "HEAD").decode().strip()
        if actual != expected:
            raise RuntimeError(f"submodule checkout does not match recorded commit: {prefix}{path}")
        for args in (("diff", "--quiet"), ("diff", "--cached", "--quiet")):
            result = subprocess.run(["git", "-C", str(module), *args], capture_output=True)
            if result.returncode:
                raise RuntimeError(f"submodule has tracked modifications: {prefix}{path}")
        yield from archives(module, expected, f"{prefix}{path}/", depth + 1)


def create(root: Path, revision: str, prefix: str, output: Path) -> None:
    top = PurePosixPath(prefix)
    if top.is_absolute() or ".." in top.parts or not prefix.strip("/"):
        raise ValueError("archive prefix must be a nonempty relative path")
    prefix = prefix.rstrip("/") + "/"
    epoch = int(git(root, "show", "-s", "--format=%ct", revision).strip())
    # Validate all gitlinks before publishing any output file.
    sources = list(archives(root, revision))
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=output.parent, delete=False) as raw:
            temporary = Path(raw.name)
            with gzip.GzipFile(fileobj=raw, mode="wb", filename="", mtime=0) as compressed:
                with tarfile.open(fileobj=compressed, mode="w|", format=tarfile.PAX_FORMAT) as destination:
                    for subprefix, data in sources:
                        with tarfile.open(fileobj=io.BytesIO(data), mode="r:") as source:
                            for original in source:
                                name = PurePosixPath(original.name)
                                if name.is_absolute() or ".." in name.parts:
                                    raise RuntimeError("unsafe source archive member")
                                member = copy.copy(original)
                                member.name = prefix + subprefix + original.name
                                member.uid = member.gid = 0
                                member.uname = member.gname = ""
                                member.mtime = epoch
                                member.pax_headers = {}
                                destination.addfile(member, source.extractfile(original) if original.isfile() else None)
        os.replace(temporary, output)
        temporary = None
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--revision", default="HEAD")
    parser.add_argument("--prefix", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    create(args.root.resolve(), args.revision, args.prefix, args.output)


if __name__ == "__main__":
    main()
