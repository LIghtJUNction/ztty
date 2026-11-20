#!/usr/bin/env python3
"""Check shared-Wayland CLI errors using feature-enabled and feature-disabled frozen ELFs."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import shutil
import subprocess
import tempfile

EXPECTED_CHECK_NAMES = [
    "invalid_backend", "missing_backend", "attach_invalid_backend", "attach_missing_backend",
    "feature_disabled", "feature_disabled_no_child",
    "missing_compositor_named", "missing_compositor_named_no_child",
    "missing_compositor_observe", "missing_compositor_observe_no_child",
    "software_renderer_requires_gpu", "software_renderer_requires_gpu_no_child",
    "wayland_workspace_rejected", "wayland_workspace_rejected_no_child",
]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--no-wayland-binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    core = args.no_wayland_binary.resolve(strict=True)
    report = {"passed": False, "checks": [], "commands": [], "binary": str(binary),
              "no_wayland_binary": str(core), "expected_checks": EXPECTED_CHECK_NAMES}
    env_base = os.environ.copy()
    for key in ("DISPLAY", "WAYLAND_DISPLAY", "WAYLAND_SOCKET", "SWAYSOCK"):
        env_base.pop(key, None)
    feature_guard = "native Wayland is optional; build with --features wayland"
    missing_window = "cannot create native Wayland window"
    try:
        if core == binary:
            raise ValueError("Feature guard requires a distinct feature-disabled ELF")
        with binary.open("rb") as stream:
            report["binary_sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
        with core.open("rb") as stream:
            report["no_wayland_binary_sha256"] = hashlib.file_digest(stream, "sha256").hexdigest()
        touch = shutil.which("touch")
        if not touch:
            raise ValueError("touch is required for the no-child sentinel checks")
        # name, executable, arguments, expected stderr, command sentinel, software renderer
        cases = [
            ("invalid_backend", binary, ["--backend", "wrong"],
             "--backend must be x11 or wayland", False, False),
            ("missing_backend", binary, ["--backend"],
             "--backend needs x11 or wayland", False, False),
            ("attach_invalid_backend", binary, ["attach", "--backend", "wrong", "unused"],
             "--backend needs x11 or wayland", False, False),
            ("attach_missing_backend", binary, ["attach", "--backend"],
             "--backend needs x11 or wayland", False, False),
            ("feature_disabled", core, ["--backend", "wayland"],
             feature_guard, True, False),
            ("missing_compositor_named", binary, ["--backend", "wayland", "--name", "cli-named"],
             missing_window, True, False),
            ("missing_compositor_observe", binary, ["--backend", "wayland", "--observe"],
             missing_window, True, False),
            ("software_renderer_requires_gpu", binary, ["--backend", "wayland"],
             "native Wayland requires renderer=auto or renderer=gpu", True, True),
            ("wayland_workspace_rejected", binary, ["--backend", "wayland", "--split-right", "unused"],
             "Wayland workspaces are not available; use independent session windows", True, False),
        ]
        with tempfile.TemporaryDirectory(prefix="ztty-wayland-cli-") as temporary:
            parent = Path(temporary)
            parent.chmod(0o700)
            for index, (name, executable, arguments, message, sentinel_check, software) in enumerate(cases):
                root = parent / str(index)
                root.mkdir(mode=0o700)
                env = env_base.copy()
                env.update(XDG_RUNTIME_DIR=str(root),
                           ZTTY_SOCKET="@ztty-wayland-cli-" + secrets.token_hex(12))
                config = root / "minimal.conf"
                config.write_text("font=monospace:pixelsize=16\n" +
                                  ("renderer=software\n" if software else ""))
                sentinel = root / "should-not-exist"
                command = [str(executable)]
                if arguments[0] != "attach":
                    command += ["--config", str(config)]
                command += arguments
                if sentinel_check:
                    command += ["-e", touch, str(sentinel)]
                entry = {"name": name, "command": command, "timeout_seconds": 8,
                         "private_runtime": str(root), "private_broker": env["ZTTY_SOCKET"]}
                report["commands"].append(entry)
                try:
                    result = subprocess.run(command, env=env, capture_output=True, text=True, timeout=8)
                except subprocess.TimeoutExpired as error:
                    entry.update(timed_out=True, stdout=str(error.stdout), stderr=str(error.stderr))
                    raise
                entry.update(exit_code=result.returncode, stdout=result.stdout, stderr=result.stderr)
                report["checks"].append({
                    "name": name, "passed": result.returncode == 1 and message in result.stderr,
                    "expected_exit_code": 1, "expected_stderr_fragment": message,
                })
                if sentinel_check:
                    report["checks"].append({"name": name + "_no_child",
                                             "passed": not sentinel.exists(),
                                             "sentinel": str(sentinel)})
            report["passed"] = ([check["name"] for check in report["checks"]] == EXPECTED_CHECK_NAMES
                                and all(check["passed"] for check in report["checks"]))
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    finally:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "checks": len(report["checks"]),
                      "report": str(args.output)}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
