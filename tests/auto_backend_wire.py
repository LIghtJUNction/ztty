#!/usr/bin/env python3
"""Exercise a real CLI against a same-UID, legacy-shaped fake broker.

The broker accepts only legacy _backend values and ignores optional fields.
It never creates a window or PTY. GUI/PTY acceptance is auto_backend_gui.py.
Only a bounded, non-secret subset of requests is written to the report.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import tempfile
import threading
import time


SAFE_FIELDS = ("op", "session", "name", "headless", "_backend", "_backend_auto",
               "observe", "target", "layout", "attachment")


def receive(channel):
    data = bytearray()
    while b"\n" not in data:
        chunk = channel.recv(65536)
        if not chunk:
            raise RuntimeError("CLI disconnected before its request")
        data.extend(chunk)
        if len(data) > 1024 * 1024:
            raise RuntimeError("request exceeds the broker protocol limit")
    return json.loads(data.split(b"\n", 1)[0])


class LegacyBroker:
    def __init__(self, case):
        self.case = case
        self.address = "@ztty-auto-wire-" + secrets.token_hex(12)
        self.requests = []
        self.errors = []
        self.stop = threading.Event()
        self.listener = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.listener.bind("\0" + self.address[1:])
        self.listener.listen(16)
        self.listener.settimeout(0.1)
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def session(self):
        backend = self.case.get("attached_backend", "x11")
        return {"uri": "ztty://local/wire-test", "name": "wire-test", "id": 1,
                "attached": backend is not None, "backend": backend,
                "attachment": 7 if backend is not None else None,
                "cols": 80, "rows": 24, "running": True, "read_only": False}

    def reply(self, request):
        self.requests.append({key: request[key] for key in SAFE_FIELDS if key in request})
        op = request.get("op")
        if op == self.case.get("fail_op"):
            return {"ok": False, "error": "intentional legacy broker rejection"}
        if op in ("create", "attach"):
            backend = request.get("_backend", "x11")
            if backend not in ("x11", "wayland"):
                return {"ok": False, "error": "unknown backend: " + str(backend)}
            if op == "attach" and self.case.get("attached_backend") is not None:
                if backend != self.case["attached_backend"]:
                    return {"ok": False, "error": "session already has a window on another backend"}
            result = self.session()
            result.update(attached=True, backend=backend, attachment=7)
            return {"ok": True, "result": result}
        if op == "info":
            return {"ok": True, "result": self.session()}
        if op == "wait":
            return {"ok": True, "result": {"exit_code": 0}}
        return {"ok": False, "error": "unexpected test operation: " + str(op)}

    def serve(self):
        while not self.stop.is_set():
            try:
                channel, _ = self.listener.accept()
            except socket.timeout:
                continue
            except OSError:
                break
            try:
                with channel:
                    channel.settimeout(2)
                    result = self.reply(receive(channel))
                    channel.sendall(json.dumps(result).encode() + b"\n")
            except Exception as error:
                self.errors.append(f"{type(error).__name__}: {error}")

    def close(self):
        self.stop.set()
        self.listener.close()
        self.thread.join(timeout=3)
        if self.thread.is_alive():
            self.errors.append("fake broker thread did not stop")


def cases():
    return [
        {"name": "default_create_legacy_x11", "args": [], "ops": ["create", "wait"],
         "backend": "x11", "auto": True},
        {"name": "explicit_auto_create_legacy_x11", "args": ["--backend", "auto"],
         "ops": ["create", "wait"], "backend": "x11", "auto": True},
        {"name": "explicit_x11_create", "args": ["--backend", "x11"],
         "ops": ["create", "wait"], "backend": "x11", "auto": False},
        {"name": "explicit_wayland_create", "args": ["--backend", "wayland"],
         "ops": ["create", "wait"], "backend": "wayland", "auto": False},
        {"name": "auto_attach_keeps_legacy_native_view", "args": ["attach", "wire-test"],
         "ops": ["info", "attach", "wait"], "backend": "wayland", "auto": True,
         "attached_backend": "wayland"},
        {"name": "explicit_auto_attach_keeps_legacy_native_view",
         "args": ["attach", "--backend", "auto", "wire-test"],
         "ops": ["info", "attach", "wait"], "backend": "wayland", "auto": True,
         "attached_backend": "wayland"},
        {"name": "auto_attach_keeps_legacy_x11_view", "args": ["attach", "wire-test"],
         "ops": ["info", "attach", "wait"], "backend": "x11", "auto": True,
         "attached_backend": "x11"},
        {"name": "auto_attach_detached_legacy_x11", "args": ["attach", "wire-test"],
         "ops": ["info", "attach", "wait"], "backend": "x11", "auto": True,
         "attached_backend": None},
        {"name": "explicit_attach_keeps_explicit_wayland",
         "args": ["attach", "--backend", "wayland", "wire-test"],
         "ops": ["attach", "wait"], "backend": "wayland", "auto": False,
         "attached_backend": "wayland"},
        {"name": "explicit_attach_conflict_never_falls_back",
         "args": ["attach", "--backend", "x11", "wire-test"],
         "ops": ["attach"], "backend": "x11", "auto": False,
         "attached_backend": "wayland", "failure": True},
        {"name": "auto_create_error_never_replays", "args": [], "ops": ["create"],
         "backend": "x11", "auto": True, "fail_op": "create", "failure": True},
        {"name": "auto_attach_info_error_never_mutates", "args": ["attach", "wire-test"],
         "ops": ["info"], "backend": None, "auto": True,
         "fail_op": "info", "failure": True},
        {"name": "auto_attach_error_never_replays", "args": ["attach", "wire-test"],
         "ops": ["info", "attach"], "backend": "wayland", "auto": True,
         "attached_backend": "wayland", "fail_op": "attach", "failure": True},
    ]


def run_case(binary, root, case, timeout):
    server = LegacyBroker(case)
    result = {"name": case["name"], "passed": False}
    try:
        env = os.environ.copy()
        env.pop("LD_PRELOAD", None)
        env.update(ZTTY_SOCKET=server.address, XDG_CONFIG_HOME=str(root / "config"),
                   XDG_CACHE_HOME=str(root / "cache"), XDG_STATE_HOME=str(root / "state"),
                   XDG_RUNTIME_DIR=str(root), LP_NUM_THREADS="2")
        arguments = case["args"][:]
        if not arguments or arguments[0] != "attach":
            arguments += ["--name", "wire-test", "-e", "/bin/true"]
        command = [str(binary), *arguments]
        process = subprocess.run(command, env=env, capture_output=True, text=True, timeout=timeout)
        result.update(returncode=process.returncode, stderr=process.stderr[-8192:])
        problems = []
        if (process.returncode != 0) != bool(case.get("failure")):
            problems.append("unexpected CLI exit status")
        if [request.get("op") for request in server.requests] != case["ops"]:
            problems.append("operation sequence differs; mutation may be missing or replayed")
        mutations = [r for r in server.requests if r.get("op") in ("create", "attach")]
        for request in mutations:
            if request.get("_backend") != case["backend"]:
                problems.append("legacy backend value was not preserved")
            if (request.get("_backend_auto") is True) != case["auto"]:
                problems.append("optional Auto flag differs from CLI preference")
        if any(r.get("_backend") == "auto" for r in server.requests):
            problems.append("unsupported auto backend was sent to a legacy broker")
        result["problems"] = problems
        result["passed"] = not problems
    except Exception as error:
        result["error"] = f"{type(error).__name__}: {error}"
    finally:
        server.close()
        result["requests"] = server.requests
        result["server_errors"] = server.errors
        result["passed"] = result["passed"] and not server.errors
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=8)
    args = parser.parse_args()
    binary, output = args.binary.resolve(), args.output.resolve()
    if not binary.is_file():
        parser.error("--binary must name a built ztty executable")
    output.mkdir(parents=True, exist_ok=True)
    started = time.monotonic()
    report = {"schema": 1, "scope": "real CLI against same-UID fake legacy broker; no PTY or GUI",
              "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
              "test_sha256": hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),
              "checks": [], "passed": False}
    try:
        with tempfile.TemporaryDirectory(prefix="ztty-auto-wire-") as temporary:
            root = Path(temporary)
            os.chmod(root, 0o700)
            (root / "config/ztty").mkdir(parents=True)
            (root / "config/ztty/ztty.conf").write_text("font=monospace:pixelsize=16\n")
            for case in cases():
                report["checks"].append(run_case(binary, root, case, args.timeout))
        report["passed"] = bool(report["checks"]) and all(c["passed"] for c in report["checks"])
    except Exception as error:
        report["error"] = f"{type(error).__name__}: {error}"
    try:
        report["binary_sha256_after"] = hashlib.sha256(binary.read_bytes()).hexdigest()
        report["binary_unchanged"] = report["binary_sha256_after"] == report["binary_sha256"]
    except OSError as error:
        report["binary_unchanged"] = False
        report["binary_after_error"] = f"{type(error).__name__}: {error}"
    report["passed"] = report["passed"] and report["binary_unchanged"]
    report["elapsed_seconds"] = time.monotonic() - started
    (output / "validation.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({"passed": report["passed"], "checks": len(report["checks"]),
                      "report": str(output / "validation.json")}))
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
