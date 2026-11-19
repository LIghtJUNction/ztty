#!/usr/bin/env python3
"""Stage bounded release downloads while preserving every evidence file in a ZIP."""
from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path, PurePosixPath
import re
import stat
import zipfile

MAX_TOTAL = 512 * 1024 * 1024
MAX_STANDALONE_ASSETS = 64
# The workflow adds the source tarball and SHA256SUMS to this script's list.
SOURCE_ASSET_COUNT = 2


def require(condition, message):
    if not condition:
        raise ValueError(message)


def sha256(data):
    return hashlib.sha256(data).hexdigest()


def json_bytes(value):
    return (json.dumps(value, sort_keys=True, indent=2) + "\n").encode("utf-8")


def relative_name(name):
    require(isinstance(name, str) and name and "\\" not in name
            and all(ord(character) >= 32 for character in name), "unsafe evidence path")
    path = PurePosixPath(name)
    require(not path.is_absolute() and ".." not in path.parts and str(path) == name,
            "noncanonical evidence path")
    return path


def flat_name(name):
    require(re.fullmatch(r"[A-Za-z0-9][A-Za-z0-9._-]*", name) is not None,
            "unsafe standalone asset name")
    return name


def identity(version, source_sha):
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version) is not None,
            "expected stable X.Y.Z version")
    require(re.fullmatch(r"[0-9a-f]{40}", source_sha) is not None,
            "expected full source SHA")



def standalone_names(files):
    """Select distribution packages and provenance; raw evidence stays archived."""
    allowed = {"install.sh"}
    for platform in ("linux", "macos", "windows"):
        for arch in ("x86_64", "aarch64"):
            stem = f"ztty-{platform}-{arch}"
            allowed.update((stem + ".build.json", stem + ".build.json.sha256",
                            stem + ".native.json"))
            formats = ("tar.gz",) if platform == "linux" else (
                ("zip", "tar.gz") if platform == "macos" else ("zip",))
            for suffix in formats:
                allowed.update((stem + "." + suffix, stem + "." + suffix + ".sha256"))
            if platform in ("macos", "windows"):
                allowed.update((stem + ".manifest.json", stem + ".manifest.json.sha256"))
    selected = []
    for name, payload in files.items():
        if len(relative_name(name).parts) == 1:
            flat_name(name)
            if name in allowed:
                require(payload, "standalone release asset is empty: " + name)
                selected.append(name)
    # Include the generated evidence ZIP/checksum and the workflow's source
    # assets. Future allowlist growth must fail before an unbounded upload.
    require(len(selected) + 2 + SOURCE_ASSET_COUNT <= MAX_STANDALONE_ASSETS,
            "standalone release asset count exceeds its bound")
    return selected

def prepare(folder, archive_path, asset_list, version, source_sha):
    identity(version, source_sha)
    folder, archive_path, asset_list = (Path(path).resolve() for path in (folder, archive_path, asset_list))
    require(folder.is_dir(), "artifact directory is missing")
    for output in (archive_path, asset_list):
        require(output != folder and folder not in output.parents,
                "release metadata must be written outside the artifact directory")
    files, total = {}, 0
    for path in sorted(folder.rglob("*")):
        require(not path.is_symlink(), "symlink evidence is not supported")
        if path.is_dir():
            continue
        require(path.is_file(), "nonregular evidence is not supported")
        relative = path.relative_to(folder).as_posix()
        relative_name(relative)
        payload = path.read_bytes()
        total += len(payload)
        require(total <= MAX_TOTAL, "evidence exceeds its size bound")
        files[relative] = payload
    selected = standalone_names(files)
    manifest = {
        "schema": 1, "format": "ztty-evidence-files-v1", "source_sha": source_sha, "version": version,
        "files": {name: {"bytes": len(payload), "sha256": sha256(payload)}
                  for name, payload in files.items()},
    }
    archive_path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive_path, "w", compression=zipfile.ZIP_STORED) as archive:
        entries = {"manifest.json": json_bytes(manifest)}
        entries.update({"files/" + name: payload for name, payload in files.items()})
        for name, payload in sorted(entries.items()):
            entry = zipfile.ZipInfo(name, date_time=(1980, 1, 1, 0, 0, 0))
            entry.create_system = 3
            entry.external_attr = (stat.S_IFREG | 0o644) << 16
            entry.compress_type = zipfile.ZIP_STORED
            archive.writestr(entry, payload)
    checksum = archive_path.with_name(archive_path.name + ".sha256")
    flat_name(archive_path.name)
    checksum.write_text(sha256(archive_path.read_bytes()) + "  " + archive_path.name + "\n", encoding="ascii")
    uploads = [folder / name for name in selected]
    uploads.extend((archive_path, checksum))
    asset_list.parent.mkdir(parents=True, exist_ok=True)
    asset_list.write_bytes(b"".join(str(path).encode("utf-8") + b"\x00" for path in uploads))
    return {"source_sha": source_sha, "version": version, "files_preserved": len(files),
            "empty_files_preserved": sum(not data for data in files.values()),
            "standalone_assets": len(uploads), "archive_sha256": sha256(archive_path.read_bytes())}


def verify_and_restore_empty(archive_path, folder, version, source_sha, expected_archive):
    identity(version, source_sha)
    archive_path, folder = Path(archive_path), Path(folder).resolve()
    require(folder.is_dir(), "release download directory is missing")
    require(archive_path.resolve() != Path(expected_archive).resolve(), "readback must be a separate download")
    require(archive_path.read_bytes() == Path(expected_archive).read_bytes(),
            "downloaded evidence archive differs from the accepted upload")
    with zipfile.ZipFile(archive_path) as archive:
        names = archive.namelist()
        require(len(names) == len(set(names)), "duplicate evidence entries")
        for item in archive.infolist():
            relative_name(item.filename)
            require(not item.is_dir() and stat.S_IFMT(item.external_attr >> 16) in (0, stat.S_IFREG),
                    "unexpected evidence entry type")
        require(sum(item.file_size for item in archive.infolist()) <= MAX_TOTAL + 16 * 1024 * 1024,
                "evidence archive expands beyond its size bound")
        manifest = json.loads(archive.read("manifest.json"))
        require(manifest["schema"] == 1 and manifest["format"] == "ztty-evidence-files-v1"
                and manifest["source_sha"] == source_sha
                and manifest["version"] == version, "evidence identity mismatch")
        files = manifest["files"]
        require(set(names) == {"manifest.json"} | {"files/" + name for name in files},
                "evidence manifest and archive entries differ")
        verified = {}
        for name, expected in files.items():
            path = relative_name(name)
            payload = archive.read("files/" + name)
            require(type(expected["bytes"]) is int and expected["bytes"] >= 0
                    and len(payload) == expected["bytes"] and sha256(payload) == expected["sha256"],
                    "evidence payload mismatch: " + name)
            if len(path.parts) == 1:
                flat_name(name)
                verified[name] = payload
        standalone = set(standalone_names(verified))
        # Validate the full archive and every required standalone download
        # before restoring archive-only evidence for the platform ledgers.
        # A missing distribution package must never be replaced from the ZIP.
        for name, payload in verified.items():
            target = folder / name
            require(not target.is_symlink(), "release readback cannot follow symlinks")
            if name in standalone:
                require(target.is_file() and target.read_bytes() == payload,
                        "standalone release asset missing or changed: " + name)
            elif target.exists():
                require(target.is_file() and target.read_bytes() == payload,
                        ("empty evidence conflicts" if not payload else "archived evidence conflicts")
                        + " with an existing asset: " + name)
        restored = []
        for name, payload in verified.items():
            target = folder / name
            if name not in standalone and not target.exists():
                with target.open("xb") as stream:
                    stream.write(payload)
                restored.append(name)
    return {"passed": True, "source_sha": source_sha, "version": version,
            "files_verified": len(files), "restored_files": restored,
            "empty_files_restored": [name for name in restored if not verified[name]],
            "archive_sha256": sha256(archive_path.read_bytes())}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("mode", choices=("prepare", "verify"))
    parser.add_argument("--folder", type=Path, required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--asset-list", type=Path)
    parser.add_argument("--expected-archive", type=Path)
    parser.add_argument("--version", required=True)
    parser.add_argument("--source-sha", required=True)
    args = parser.parse_args()
    if args.mode == "prepare":
        if args.asset_list is None:
            parser.error("--asset-list is required for prepare")
        result = prepare(args.folder, args.archive, args.asset_list, args.version, args.source_sha)
    else:
        if args.expected_archive is None:
            parser.error("--expected-archive is required for verify")
        result = verify_and_restore_empty(args.archive, args.folder, args.version, args.source_sha, args.expected_archive)
    print(json.dumps(result, sort_keys=True))


if __name__ == "__main__":
    main()
