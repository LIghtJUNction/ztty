#!/usr/bin/env python3
"""Preparation-only validation. Never executes a product binary, compiler, compositor, or GUI test."""
import ast
from datetime import datetime, timezone
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
LEGACY = REPO / "artifacts/final-gui-20261120"
EXPECTED_LEGACY = {
    "run.py": "9fbbfd9944888b563f4efb8fe585061a7ef9d5505758c996fd82374c1278215a",
    "owned_process.py": "e17b1aa0f5a1fbde443698ae6f615296d46ba78e39dfb2bd9679f94a7cc686a7",
    "identities.py": "eeeb3344db3d80d78086f19184d86b9d188f0dbe5901d159ccf55ce2919d0be1",
    "plan.py": "6e54417163b871a4353c7a9757faf72401468674b9eb4ca8523d5e6d605b0467",
    "wayland_cli.py": "a967bfdb315cb15ae714bf2d9961cd0ab5199c2105863a3dd8c714064060b968",
}


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def save_json(path, value):
    path.write_text(json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def arguments(path):
    found = {}
    for node in ast.walk(ast.parse(path.read_text(), filename=str(path))):
        if isinstance(node, ast.Call) and isinstance(node.func, ast.Attribute) and node.func.attr == "add_argument":
            names = [arg.value for arg in node.args if isinstance(arg, ast.Constant) and isinstance(arg.value, str)]
            metadata = {}
            for keyword in node.keywords:
                if keyword.arg in ("required", "choices"):
                    metadata[keyword.arg] = ast.literal_eval(keyword.value)
            for name in names:
                found[name] = {"aliases": names, **metadata}
    return found


def main():
    report = {"prepared_only": True, "complete": False,
              "recorded_utc": datetime.now(timezone.utc).isoformat(),
              "execution_started": False,
              "scope": "Python AST, argument errors, and --plan only; no builds or GUI."}
    legacy_snapshot = HERE / "legacy-runner-7544"
    legacy_snapshot.mkdir(exist_ok=True)
    legacy = []
    for name, expected in EXPECTED_LEGACY.items():
        source = LEGACY / name
        assert digest(source) == expected, (name, "legacy changed")
        destination = legacy_snapshot / name
        if destination.exists():
            assert destination.read_bytes() == source.read_bytes(), (name, "snapshot differs")
        else:
            shutil.copyfile(source, destination)
        legacy.append({"original": str(source), "snapshot": str(destination), "sha256": expected})
    report["legacy_runner_preserved"] = legacy
    history = {
        "runs/integration-7544/acceptance.json": "67cd0c2f256df985cca27a7cc8c4bbd8213707f092207668a51f6b333a05b364",
        "runs/diagnostic-wayland-7544/diagnostic.json": "c6ecd7c715119708e1dbe77e71eee7ce9cc0a8e52a6275acf4d82288c4a16515",
    }
    report["legacy_results_preserved"] = []
    for name, expected in history.items():
        assert digest(LEGACY / name) == expected, name
        report["legacy_results_preserved"].append({"path": str(LEGACY / name), "sha256": expected})
    for name in ("sway-dependencies.json",):
        source, destination = LEGACY / name, HERE / name
        if destination.exists():
            assert destination.read_bytes() == source.read_bytes(), name
        else:
            shutil.copyfile(source, destination)
    for path in HERE.glob("*.py"):
        ast.parse(path.read_text(), filename=str(path))
    assert digest(HERE / "owned_process.py") == EXPECTED_LEGACY["owned_process.py"]
    env = os.environ.copy()
    env["PYTHONDONTWRITEBYTECODE"] = "1"
    planned = subprocess.run([sys.executable, str(HERE / "run.py"), "--plan",
                              "--output", str(HERE / "runs/prepared-preview")],
                             cwd=REPO, env=env, capture_output=True, text=True)
    assert planned.returncode == 0, planned.stderr
    plan = json.loads(planned.stdout)
    assert plan["prepared_only"] is True and plan["stages"] == 21
    assert not (HERE / "runs/prepared-preview").exists()
    save_json(HERE / "plan.json", plan)
    parameter_checks = []
    for step in plan["steps"]:
        command = step["command"]
        schema = arguments(Path(command[1]))
        supplied = {value for value in command[2:] if value.startswith("--")}
        unknown = sorted(supplied - schema.keys())
        missing = sorted({name for name, value in schema.items()
                          if value.get("required") and not (set(value["aliases"]) & supplied)})
        assert not unknown and not missing, (step["name"], unknown, missing)
        for option in supplied:
            choices = schema[option].get("choices")
            if choices:
                assert command[command.index(option) + 1] in choices, (step["name"], option)
        parameter_checks.append({"stage": step["name"], "valid": True})
    shared, cli = plan["steps"][19], plan["steps"][20]
    assert shared["expected_checks"] == 65 and shared["strict_checks"] is True
    assert "--ime-peer" in shared["command"]
    assert cli["expected_checks"] == 14 and cli["strict_checks"] is True
    assert "--no-wayland-binary" in cli["command"]
    assert len(cli["required_check_names"]) == 14
    source = (REPO / "tests/wayland_gui.py").read_text()
    tree = ast.parse(source)
    strings = {node.value for node in ast.walk(tree)
               if isinstance(node, ast.Constant) and isinstance(node.value, str)}
    ime_names = [name for name in shared["required_check_names"] if name.startswith("ime_")]
    assert len(ime_names) == 7 and all(name in strings for name in ime_names)
    order_pairs = []
    for node in ast.walk(tree):
        if isinstance(node, ast.For) and isinstance(node.target, ast.Tuple):
            names = [item.id for item in node.target.elts if isinstance(item, ast.Name)]
            if names == ["first", "second"]:
                order_pairs = ast.literal_eval(node.iter)
    assert order_pairs == [("x11", "wayland"), ("wayland", "x11")]
    report["plan"] = {"stages": 21, "wayland_checks": 65, "cli_checks": 14,
                      "required_wayland_checks": shared["required_check_names"],
                      "ime_check_names": ime_names, "mixed_orders": order_pairs,
                      "parameter_validation": parameter_checks}
    required = {
        "--binary": "/ABSOLUTE/FROZEN/ztty",
        "--sha256": "1" * 64,
        "--source-sha": "4" * 40,
        "--no-wayland-binary": "/ABSOLUTE/FROZEN/ztty-no-wayland",
        "--no-wayland-sha256": "2" * 64,
        "--ime-peer": "/ABSOLUTE/FROZEN/ime-peer",
        "--ime-peer-sha256": "3" * 64,
    }
    report["missing_argument_errors"] = []
    with tempfile.TemporaryDirectory(prefix="ztty-runner-args-") as temporary:
        for omitted in required:
            output = Path(temporary) / omitted[2:]
            command = [sys.executable, str(HERE / "run.py")]
            for flag, value in required.items():
                if flag != omitted:
                    command += [flag, value]
            command += ["--output", str(output)]
            result = subprocess.run(command, cwd=REPO, env=env, capture_output=True, text=True)
            passed = result.returncode == 2 and omitted in result.stderr and not output.exists()
            report["missing_argument_errors"].append(
                {"omitted": omitted, "exit_code": result.returncode, "passed": passed,
                 "output_created": output.exists(), "stderr": result.stderr})
            assert passed, omitted
    required_tools = ("rtk", "git", "Xvfb", "cc", "fcitx5", "qdbus6",
                      "dbus-daemon", "dbus-run-session", "setxkbmap", "wtype", "grim",
                      "wl-copy", "wl-paste", "touch")
    report["runtime_tool_paths"] = {name: shutil.which(name) for name in required_tools}
    report["missing_runtime_tools"] = [name for name, path in report["runtime_tool_paths"].items() if not path]
    report["prepared_source_hashes"] = {
        name: digest(REPO / name) for name in
        ("tests/wayland_gui.py", "tests/wayland_child.py", "tests/build_wayland_ime_peer.py",
         "tests/wayland_ime_peer.c", "tests/protocols/input-method-unstable-v2.xml",
         "tools/isolated_xvfb.py")}
    report["runner_sha256"] = {path.name: digest(path) for path in HERE.glob("*.py")}
    report["complete"] = not report["missing_runtime_tools"]
    save_json(HERE / "legacy-preservation.json", {
        "runner": legacy, "historical_results": report["legacy_results_preserved"]})
    save_json(HERE / "prepared.json", report)
    print(json.dumps({"prepared_only": True, "complete": report["complete"],
                      "stages": 21, "wayland_checks": 65, "cli_checks": 14,
                      "missing_argument_guards": len(report["missing_argument_errors"]),
                      "ime_assertions_required": len(ime_names),
                      "mixed_orders": order_pairs, "missing_runtime_tools": report["missing_runtime_tools"],
                      "report": str(HERE / "prepared.json")}))
    return 0 if report["complete"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
