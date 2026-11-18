#!/usr/bin/env python3
"""Build/run isolated consumers of selected, actually published crates.io packages.

Generated manifests contain only exact registry versions, never local paths.
Each consumer library is no_std; its executable is a std-based test harness.
"""
import argparse
import datetime
import json
import pathlib
import re
import subprocess
import tempfile
import tomllib


ROOT = pathlib.Path(__file__).resolve().parents[1]
PACKAGES = ("zinput", "zgraphics", "zpty", "zterm")


def release_versions():
    workspace = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]
    versions = {}
    for package in PACKAGES:
        manifest = tomllib.loads((ROOT / "crates" / package / "Cargo.toml").read_text())
        version = manifest["package"]["version"]
        if version == {"workspace": True}:
            version = workspace["version"]
        if not isinstance(version, str) or not version:
            raise ValueError(f"invalid release version for {package}")
        versions[package] = version
    return versions


VERSIONS = release_versions()
CRATES_IO_SOURCES = {
    "registry+https://github.com/rust-lang/crates.io-index",
    "registry+https://index.crates.io/",
    "registry+sparse+https://index.crates.io/",
}


def manifest(selected, name):
    dependencies = "\n".join(
        f'{package} = {{ version = "={VERSIONS[package]}", default-features = false }}'
        for package in selected
    )
    return f'''[package]
name = "{name}"
version = "0.0.0"
edition = "2024"

[workspace]

[dependencies]
{dependencies}
'''


def library(selected):
    source = "#![no_std]\nextern crate alloc;\n\npub fn exercise() {\n"
    if "zinput" in selected:
        source += r'''    let modes = zinput::Modes { kitty_keyboard: 1, ..Default::default() };
    let bytes = zinput::encode_key(zinput::Key::Character('c'),
        zinput::Modifiers(4), zinput::KeyEvent::Press, "", &modes);
    assert_eq!(bytes, b"\x1b[99;5u");
'''
    if "zgraphics" in selected:
        source += r'''    let mut scene = zgraphics::Graphics::default();
    let mut replies = alloc::vec::Vec::new();
    let context = zgraphics::Context { col: 0, row: 0, cols: 80, rows: 24,
        cell_width: 8, cell_height: 16 };
    scene.command(b"Ga=T,f=32,s=1,v=1,i=7,C=1;/wAA/w==", context, &mut replies);
    assert_eq!(scene.image(7).unwrap().pixels.as_slice(), &[255,0,0,255]);
    assert_eq!(scene.draws(0).count(), 1);
    scene.command(b"Ga=d,d=I,i=7", context, &mut replies);
    assert_eq!(scene.image_count(), 0);
'''
    if "zterm" in selected:
        source += r'''    let mut terminal = zterm::Terminal::new(80, 24, 20);
    terminal.feed("\x1b[31m你a\x1b[6n".as_bytes());
    assert_eq!(terminal.cell(0, 0).ch, '你');
    assert_eq!(terminal.cell(0, 0).width, 2);
    assert_eq!(terminal.responses(), b"\x1b[1;4R");
    assert!(!terminal.modes().input().bracketed_paste);
'''
        if "zinput" in selected:
            source += "    let _: zinput::Modes = terminal.modes().input();\n"
    if "zpty" in selected:
        source += '''    let size = zpty::Size::new(80, 24);
    assert_eq!((size.cols, size.rows), (80, 24));
'''
    return source + "}\n"


def executable(selected, crate_name):
    source = '''fn main() -> Result<(), Box<dyn std::error::Error>> {
''' + f"    {crate_name.replace('-', '_')}::exercise();\n"
    if "zpty" in selected:
        source += r'''    use std::time::{Duration, Instant};
    let argv = [c"sh", c"-c", c"printf registry-pty-ok"];
    let env = [c"PATH=/usr/bin:/bin", c"TERM=xterm-256color"];
    // This fixture is single-threaded and starts no callbacks before forkpty.
    let mut pty = unsafe { zpty::Pty::spawn(c"/bin/sh", &argv, &env,
        zpty::Size::new(80, 24), &[]) }?;
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut buffer = [0; 256];
    loop {
        match pty.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => output.extend_from_slice(&buffer[..n]),
            Err(error) if error.is_would_block() => std::thread::sleep(Duration::from_millis(1)),
            Err(error) => return Err(error.into()),
        }
        if Instant::now() > deadline { return Err("PTY consumer timed out".into()); }
    }
    assert_eq!(output, b"registry-pty-ok");
    assert!(pty.wait_timeout(1000)?.expect("child exited").success());
'''
    return source + '''    println!("registry consumer passed");
    Ok(())
}
'''


def safe_log(value):
    value = value or ""
    if isinstance(value, bytes):
        value = value.decode("utf-8", "replace")
    value = re.sub(r"(?i)(https?://)[^/\s:@]+:[^/\s@]+@", r"\1[redacted]@", value)
    value = re.sub(r"(?im)^.*(?:authorization:|bearer |(?:_TOKEN|token)\s*[=:]).*$", "[redacted credential-bearing line]", value)
    return value[-20000:]


def cargo(arguments, cwd, result, stage):
    command = ["cargo", "--config", "net.retry=1", "--config", "http.timeout=20"] + arguments
    record = {"stage": stage, "command": command}
    result["commands"].append(record)
    try:
        completed = subprocess.run(command, cwd=cwd, capture_output=True, text=True, timeout=90)
    except subprocess.TimeoutExpired as error:
        record.update(ok=False, error="cargo command exceeded 90 seconds", stderr=safe_log(error.stderr))
        raise RuntimeError(record["error"]) from None
    record.update(ok=completed.returncode == 0, exit_code=completed.returncode,
                  stderr=safe_log(completed.stderr))
    if stage != "metadata":
        record["stdout"] = safe_log(completed.stdout)
    if completed.returncode:
        raise RuntimeError(f"cargo {stage} failed with exit code {completed.returncode}")
    return completed.stdout


def check(selected, directory):
    name = "ztty-registry-check-" + "-".join(selected)
    result = {"selected": selected, "ok": False, "packages": [], "commands": [],
              "consumer_library_no_std": True, "consumer_dependency_kind": "exact crates.io versions; no path or git dependencies"}
    try:
        (directory / "src").mkdir(parents=True)
        (directory / "Cargo.toml").write_text(manifest(selected, name), encoding="utf-8")
        (directory / "src/lib.rs").write_text(library(selected), encoding="utf-8")
        (directory / "src/main.rs").write_text(executable(selected, name), encoding="utf-8")
        manifest_path = str(directory / "Cargo.toml")
        metadata = json.loads(cargo(["metadata", "--format-version", "1", "--manifest-path", manifest_path], directory, result, "metadata"))
        lock = tomllib.loads((directory / "Cargo.lock").read_text())
        checksums = {(p["name"], p["version"]): p.get("checksum") for p in lock.get("package", [])}
        found = set()
        for package in metadata["packages"]:
            if package["name"] not in PACKAGES:
                continue
            package_root = pathlib.Path(package["manifest_path"]).parent
            vcs_path = package_root / ".cargo_vcs_info.json"
            vcs = json.loads(vcs_path.read_text()) if vcs_path.is_file() else None
            library_targets = [target for target in package["targets"] if "lib" in target["kind"]]
            no_std = bool(library_targets) and all(
                re.search(r"(?m)^#!\[no_std\]\s*$", pathlib.Path(target["src_path"]).read_text())
                for target in library_targets
            )
            actual = {"name": package["name"], "version": package["version"], "source": package["source"],
                      "selected": package["name"] in selected, "vcs": vcs,
                      "cargo_lock_checksum": checksums.get((package["name"], package["version"])),
                      "library_source_has_no_std": bool(no_std)}
            result["packages"].append(actual)
            if actual["selected"]:
                found.add(package["name"])
                if package["version"] != VERSIONS[package["name"]]:
                    raise AssertionError(f"unexpected version for {package['name']}: {package['version']}")
                if package["source"] not in CRATES_IO_SOURCES:
                    raise AssertionError(f"{package['name']} did not resolve from crates.io")
                if not no_std:
                    raise AssertionError(f"{package['name']} library is missing its no_std crate attribute")
        if found != set(selected):
            raise AssertionError(f"missing packages in Cargo resolution: {sorted(set(selected) - found)}")
        cargo(["check", "--lib", "--locked", "--manifest-path", manifest_path], directory, result, "no_std_library_check")
        cargo(["run", "--locked", "--manifest-path", manifest_path], directory, result, "consumer_run")
        result["ok"] = True
    except Exception as error:
        result["error"] = safe_log(repr(error))
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--packages", nargs="+", choices=PACKAGES, default=list(PACKAGES),
                        help="crates to verify; default: zinput zgraphics zpty zterm")
    parser.add_argument("--output", type=pathlib.Path, default=ROOT / "artifacts/registry-consumer.json")
    args = parser.parse_args()
    selected = list(dict.fromkeys(args.packages))
    result = {"schema": 3, "timestamp_utc": datetime.datetime.now(datetime.timezone.utc).isoformat(),
              "requested_packages": selected, "versions": VERSIONS, "ok": False,
              "packages": [], "individual_checks": [], "combined_check": None,
              "scope": "actual crates.io consumers; library code is no_std, executable harness uses std; zpty runs a real PTY"}
    args.output.parent.mkdir(parents=True, exist_ok=True)

    def save():
        args.output.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    try:
        with tempfile.TemporaryDirectory(prefix="ztty-registry-") as temporary:
            root = pathlib.Path(temporary)
            for package in selected:
                checked = check([package], root / package)
                result["individual_checks"].append(checked)
                result["packages"].extend(p for p in checked["packages"] if p["selected"])
                save()
                print(json.dumps({"package": package, "ok": checked["ok"], "packages": checked["packages"],
                                  "error": checked.get("error")}, ensure_ascii=False), flush=True)
            if len(selected) > 1 and all(item["ok"] for item in result["individual_checks"]):
                result["combined_check"] = check(selected, root / "combined")
            result["ok"] = all(item["ok"] for item in result["individual_checks"]) and (
                result["combined_check"] is None or result["combined_check"]["ok"])
    except Exception as error:
        result["error"] = safe_log(repr(error))
    finally:
        save()
    print(json.dumps({"ok": result["ok"], "requested_packages": selected, "output": str(args.output)}, ensure_ascii=False), flush=True)
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
