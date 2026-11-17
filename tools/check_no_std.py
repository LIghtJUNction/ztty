#!/usr/bin/env python3
"""Check shipped Rust dependencies and the linked executable for std usage."""
import json
import pathlib
import re
import subprocess


def main():
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1"], text=True
    ))
    packages = {p["id"]: p for p in metadata["packages"]}
    nodes = {n["id"]: n for n in metadata["resolve"]["nodes"]}
    root = metadata["resolve"]["root"]
    pending, seen = [root], set()
    while pending:
        package_id = pending.pop()
        if package_id in seen:
            continue
        seen.add(package_id)
        node, package = nodes[package_id], packages[package_id]
        assert "std" not in node["features"], (package["name"], node["features"])
        targets = [t for t in package["targets"] if "lib" in t["kind"] or "bin" in t["kind"]]
        for target in targets:
            source = pathlib.Path(target["src_path"]).read_text()
            assert re.search(r"#!\[no_std\]", source), target["src_path"]
        for dep in node["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                pending.append(dep["pkg"])
        print(f'{package["name"]} {package["version"]}: no_std, features={node["features"]}')
    binary = pathlib.Path(metadata["target_directory"]) / "release" / "ztty"
    symbols = subprocess.check_output(["nm", "-C", str(binary)], text=True)
    forbidden = [line for line in symbols.splitlines() if re.search(r"\bstd::", line)]
    assert not forbidden, forbidden[:10]
    print("release executable: no Rust std symbols")
    print("System C ABI libraries are listed separately by ldd; this check covers Rust std.")


if __name__ == "__main__":
    main()
