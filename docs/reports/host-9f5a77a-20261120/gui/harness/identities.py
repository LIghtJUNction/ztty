"""Read immutable source, binary, tool, and plugin identities without executing them."""
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import shutil
import subprocess

LIBRARIES = {"example", "before-example", "fixture-valid", "fixture-wrong-interface", "fixture-pinned"}


def digest(path):
    with Path(path).open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def binary_identity(path, expected):
    if not re.fullmatch(r"[0-9a-f]{64}", expected):
        raise ValueError("sha256 must be 64 lowercase hexadecimal characters")
    with path.open("rb") as stream:
        if stream.read(4) != b"\x7fELF":
            raise ValueError("The final Linux binary must be ELF")
    if not os.access(path, os.X_OK):
        raise ValueError(f"ELF must be executable: {path}")
    actual = digest(path)
    if actual != expected:
        raise ValueError(f"ELF hash mismatch: expected {expected}, actual {actual}")
    stat = path.stat()
    return {"path": str(path), "sha256": actual, "bytes": stat.st_size}


def source_identity(repo, expected):
    if not re.fullmatch(r"[0-9a-f]{40}", expected):
        raise ValueError("source-sha must be a full lowercase commit SHA")

    def git(*args):
        return subprocess.check_output(["git", *args], cwd=repo, text=True).strip()

    actual = git("rev-parse", "HEAD")
    dirty = git("status", "--porcelain", "--untracked-files=normal", "--ignore-submodules=none")
    submodules = git("submodule", "status", "--recursive")
    if actual != expected or dirty:
        raise ValueError(f"Source must be clean at {expected}; HEAD={actual}; status={dirty!r}")
    if any(line.startswith(("-", "+", "U")) for line in submodules.splitlines()):
        raise ValueError("All submodules must match their recorded commits")
    return {"sha": actual, "tree": git("rev-parse", "HEAD^{tree}"), "clean": True,
            "submodules": submodules.splitlines()}


def plugin_identities(directory):
    path = directory / "libraries.json"
    manifest = json.loads(path.read_text())
    if manifest.get("ok") is not True:
        raise ValueError("Plugin build manifest is not successful")
    entries = manifest.get("libraries", [])
    if {entry["name"] for entry in entries} != LIBRARIES or len(entries) != len(LIBRARIES):
        raise ValueError("Unexpected plugin fixture set")
    verified = []
    for entry in entries:
        name = entry["name"]
        library = (directory / (name + ".so")).resolve(strict=True)
        if Path(entry["path"]).resolve(strict=True) != library or library.parent != directory:
            raise ValueError(f"Plugin path mismatch: {name}")
        actual = digest(library)
        if actual != entry["sha256"]:
            raise ValueError(f"Plugin hash mismatch: {name}")
        verified.append({"name": name, "path": str(library), "sha256": actual})
    return {"manifest": str(path), "manifest_sha256": digest(path), "libraries": verified,
            "old_plugin_source": manifest.get("old_plugin_source"),
            "old_plugin_zplugin": manifest.get("old_plugin_zplugin"),
            "new_plugin_zplugin": manifest.get("new_plugin_zplugin")}


def tool_identities(sway, swaymsg):
    required = ("rtk", "git", "Xvfb", "cc", "fcitx5", "qdbus6", "dbus-daemon",
                "dbus-run-session", "setxkbmap", "wtype", "grim", "wl-copy", "wl-paste", "touch")
    paths = {name: shutil.which(name) for name in required}
    missing = [name for name, path in paths.items() if path is None]
    missing.extend(name for name in ("PIL", "Xlib") if importlib.util.find_spec(name) is None)
    if missing:
        raise ValueError("Missing GUI dependencies: " + ", ".join(missing))
    for name, path in (("sway", sway), ("swaymsg", swaymsg)):
        if not path.is_file() or not os.access(path, os.X_OK):
            raise ValueError(f"{name} must identify an executable file")
        paths[name] = str(path)
    probe = Path(__file__).with_name("sway-dependencies.json")
    if probe.is_file():
        for item in json.loads(probe.read_text())["results"]:
            if item["path"] in (str(sway), str(swaymsg)) and digest(item["path"]) != item["sha256"]:
                raise ValueError("The previously verified private Sway tool changed")
    return {"executables": paths,
            "sway": {"path": str(sway), "sha256": digest(sway)},
            "swaymsg": {"path": str(swaymsg), "sha256": digest(swaymsg)},
            "private_library_path": None}


def harness_identities(repo, local):
    paths = list((repo / "tests").glob("*.py")) + list((repo / "tests").glob("*.c"))
    paths += [repo / "tools" / name for name in
              ("isolated_xvfb.py", "gui_renderer.py", "measure_runtime.py",
               "measure_multiwindow.py", "benchmark_terminals.py")]
    paths += [repo / "examples/native_ui.py",
              repo / "tests/protocols/input-method-unstable-v2.xml", *local.glob("*.py")]
    return {str(path): digest(path) for path in sorted(set(paths))}
