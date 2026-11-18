#!/usr/bin/env python3
"""Run source-bound Linux release acceptance on the actual native architecture.

This gate exercises real PTYs and X11 windows in guarded private Xvfb servers.
It does not claim native Wayland, GPU scanout, physical input latency, or hardware
IME coverage. Python Xlib and Pillow belong to the child GUI harnesses only.

    python3 tests/linux_native.py --binary target/release/ztty --source-root . \
      --source-sha COMMIT --arch x86_64 --version VERSION \
      --output dist/ztty-linux-x86_64.native.json \
      --evidence-dir dist/ztty-linux-x86_64.native
"""
import argparse
import contextlib
import ctypes
from datetime import datetime, timezone
import hashlib
import importlib.metadata
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tarfile
import time


ELF_MACHINES = {"x86_64": 62, "aarch64": 183}
RUN_MARKER = "ZTTY_LINUX_NATIVE_RUN"
CASES = ("workspaces", "daily_workflow", "session_switcher", "sixel_gui", "input_gui", "repaint_gui")
HARNESS_FILES = tuple(f"tests/{name}.py" for name in CASES)


class NativeGateInterrupted(RuntimeError):
    def __init__(self, signum):
        self.signum = signum
        super().__init__(f"native acceptance interrupted by signal {signum}")


@contextlib.contextmanager
def termination_scope():
    previous = {signum: signal.getsignal(signum) for signum in (signal.SIGTERM, signal.SIGINT)}

    def interrupted(signum, _frame):
        # Further termination requests must not interrupt the bounded owned
        # cleanup that the first request starts. SIGKILL cannot be handled.
        for key in previous:
            signal.signal(key, signal.SIG_IGN)
        raise NativeGateInterrupted(signum)

    try:
        for signum in previous:
            signal.signal(signum, interrupted)
        yield
    finally:
        for signum, handler in previous.items():
            signal.signal(signum, handler)


def sha256(path):
    digest = hashlib.sha256()
    with Path(path).open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def inspect_elf(binary, expected_arch):
    with Path(binary).open("rb") as stream:
        header = stream.read(64)
    if len(header) != 64 or header[:4] != b"\x7fELF":
        raise ValueError("binary is not a complete ELF64 executable")
    if header[4:7] != b"\x02\x01\x01":
        raise ValueError("expected a 64-bit little-endian ELF version 1 executable")
    kind, machine, version = struct.unpack_from("<HHI", header, 16)
    if kind not in (2, 3) or version != 1:
        raise ValueError("ELF header is not an executable or position-independent executable")
    if machine != ELF_MACHINES[expected_arch]:
        raise ValueError(f"ELF machine {machine} does not match required {expected_arch}")
    return {"class_bits": 64, "byte_order": "little", "machine": machine,
            "architecture": expected_arch, "type": kind}


def inspect_source(root):
    def git(*arguments):
        return subprocess.check_output(["git", *arguments], cwd=root, text=True, timeout=10).strip()
    return {"commit": git("rev-parse", "HEAD"),
            "tracked_changes": git("status", "--porcelain", "--untracked-files=no").splitlines()}


def require_source(snapshot, expected):
    if snapshot["commit"] != expected:
        raise ValueError(f"checkout {snapshot['commit']} does not match pinned source {expected}")
    if snapshot["tracked_changes"]:
        raise ValueError("release acceptance requires clean tracked sources: " +
                         repr(snapshot["tracked_changes"][:30]))


def process_identity(pid):
    try:
        text = Path(f"/proc/{pid}/stat").read_text()
        fields = text[text.rfind(")") + 2:].split()
        return fields[19]
    except (FileNotFoundError, ProcessLookupError):
        return None


def marked_processes(marker):
    expected = (RUN_MARKER + "=" + marker).encode()
    result = {}
    for entry in Path("/proc").iterdir():
        if not entry.name.isdecimal():
            continue
        try:
            if entry.stat().st_uid != os.getuid():
                continue
            if expected not in (entry / "environ").read_bytes().split(b"\0"):
                continue
            started = process_identity(int(entry.name))
            if started is not None:
                result[int(entry.name)] = started
        except (OSError, ValueError):
            continue
    return result


def reap_children():
    # Only descendants adopted by this wrapper can be waitpid children.
    for _ in range(4096):
        try:
            if os.waitpid(-1, os.WNOHANG)[0] == 0:
                break
        except ChildProcessError:
            break


def owned_leases(harness_pid, started_ns):
    leases = []
    for path in Path("/tmp").glob("ztty-xvfb-*.lease"):
        try:
            descriptor = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_CLOEXEC)
            try:
                info = os.fstat(descriptor)
                owner = os.read(descriptor, 64)
                if (stat.S_ISREG(info.st_mode) and info.st_uid == os.getuid() and
                        info.st_ctime_ns >= started_ns and
                        owner == f"{harness_pid}\n".encode()):
                    leases.append((path, info.st_dev, info.st_ino))
            finally:
                os.close(descriptor)
        except (FileNotFoundError, OSError):
            continue
    return leases


def cleanup_marker(marker, harness_pid, started_ns):
    observed = marked_processes(marker)
    leases = owned_leases(harness_pid, started_ns)
    for signum, grace in ((signal.SIGTERM, 3), (signal.SIGKILL, 2)):
        current = marked_processes(marker)
        for pid, started in current.items():
            if process_identity(pid) == started:
                with contextlib.suppress(ProcessLookupError):
                    os.kill(pid, signum)
        deadline = time.monotonic() + grace
        while time.monotonic() < deadline:
            reap_children()
            if not marked_processes(marker):
                break
            time.sleep(0.05)
    reap_children()
    remaining = marked_processes(marker)
    removed = []
    if not remaining:
        # A killed harness cannot run its helper's finally block. Only remove
        # its identity-checked private lease; never remove an X11 lock/socket.
        for path, device, inode in leases:
            with contextlib.suppress(FileNotFoundError):
                info = path.lstat()
                if (info.st_dev, info.st_ino) != (device, inode):
                    raise RuntimeError(f"private display lease changed ownership: {path}")
                path.unlink()
                removed.append(str(path))
    return {"leftover_owned_pids": sorted(observed), "remaining_owned_pids": sorted(remaining),
            "removed_owned_leases": removed}


def run_owned(command, directory, source_root, timeout):
    directory.mkdir(parents=True, exist_ok=False)
    marker = secrets.token_hex(24)
    environment = dict(os.environ, **{RUN_MARKER: marker}, PYTHONUNBUFFERED="1")
    result = {"command": list(map(str, command)), "timeout_seconds": timeout,
              "timed_out": False, "stdout": str(directory / "stdout.log"),
              "stderr": str(directory / "stderr.log")}
    process = None
    started = time.monotonic()
    started_ns = time.time_ns()
    try:
        with (directory / "stdout.log").open("wb") as stdout, (directory / "stderr.log").open("wb") as stderr:
            process = subprocess.Popen(command, cwd=source_root, env=environment,
                                       stdout=stdout, stderr=stderr, start_new_session=True)
            result["harness_pid"] = process.pid
            try:
                result["exit_code"] = process.wait(timeout=timeout)
            except subprocess.TimeoutExpired:
                result["timed_out"] = True
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=4)
                result["exit_code"] = process.returncode
    finally:
        if process is not None:
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=4)
            result["cleanup"] = cleanup_marker(marker, process.pid, started_ns)
        result["duration_seconds"] = round(time.monotonic() - started, 3)
    return result


def process_passed(result):
    cleanup = result.get("cleanup", {})
    return (result.get("exit_code") == 0 and not result.get("timed_out") and
            not any(cleanup.get(key) for key in
                    ("leftover_owned_pids", "remaining_owned_pids", "removed_owned_leases")))


def png_info(path):
    with path.open("rb") as stream:
        header = stream.read(24)
    if len(header) != 24 or header[:8] != b"\x89PNG\r\n\x1a\n" or header[12:16] != b"IHDR":
        raise ValueError(f"missing actual PNG capture: {path}")
    width, height = struct.unpack_from(">II", header, 16)
    if not (0 < width <= 16384 and 0 < height <= 16384):
        raise ValueError(f"invalid screenshot dimensions: {path}")
    return {"width": width, "height": height, "sha256": sha256(path)}


def case_command(name, root, binary, directory):
    command = [sys.executable, str(root / "tests" / f"{name}.py"),
               "--binary", str(binary), "--output", str(directory / "report.json")]
    if name in ("daily_workflow", "repaint_gui", "session_switcher"):
        command.extend(["--screenshots", str(directory / "screenshots")])
    else:
        command.extend(["--screenshot", str(directory / "screenshot.png")])
    if name in ("sixel_gui", "input_gui"):
        command.extend(["--source-root", str(root)])
    return command


def run_case(name, root, binary, directory, timeout, binary_hash):
    result = {"name": name, "passed": False}
    try:
        result.update(run_owned(case_command(name, root, binary, directory), directory, root, timeout))
        report_path = directory / "report.json"
        if report_path.stat().st_size > 8 * 1024 * 1024:
            raise ValueError("oversized child acceptance report")
        data = json.loads(report_path.read_text())
        checks = data.get("checks")
        if not isinstance(checks, list) or not checks:
            raise ValueError("child report has no completed acceptance checks")
        result["checks_completed"] = len(checks)
        result["report"] = str(report_path)
        if data.get("binary_sha256") != binary_hash:
            raise ValueError("child report tested a different binary hash")
        result["screenshots"] = {str(path.relative_to(directory)): png_info(path)
                                 for path in sorted(directory.rglob("*.png"))}
        if not result["screenshots"]:
            raise ValueError("child acceptance produced no PNG screenshot")
        result["passed"] = process_passed(result) and data.get("ok") is True
        if not result["passed"]:
            result["error"] = data.get("error", "child exit, acceptance, timeout, or cleanup gate failed")
    except NativeGateInterrupted:
        raise
    except Exception as error:
        result["error"] = repr(error)
    return result


def archive_evidence(evidence, output):
    """Preserve an exact flat release asset before removing its private tree.

    Failed packing or verification leaves the source evidence in place. The
    archive is installed without overwriting an earlier asset, and only this
    explicitly supplied directory is removed after every file is read back.
    """
    evidence, output = Path(evidence), Path(output)
    if evidence.is_symlink() or not evidence.is_dir():
        raise ValueError("native evidence must be a real directory")
    evidence, output = evidence.resolve(), output.absolute()
    if output.resolve().is_relative_to(evidence):
        raise ValueError("native archive must be outside the evidence directory")
    if output.exists() or output.is_symlink():
        raise FileExistsError(output)
    root_identity = (evidence.stat().st_dev, evidence.stat().st_ino)
    snapshot = {}
    for path in sorted(evidence.rglob("*")):
        info = path.lstat()
        if stat.S_ISDIR(info.st_mode):
            continue
        if not stat.S_ISREG(info.st_mode):
            raise ValueError(f"refusing non-regular native evidence: {path}")
        snapshot[path.relative_to(evidence).as_posix()] = {
            "bytes": info.st_size, "sha256": sha256(path)}
    if not snapshot:
        raise ValueError("native evidence contains no files")
    output.parent.mkdir(parents=True, exist_ok=True)
    temporary = output.with_name(output.name + "." + secrets.token_hex(8) + ".tmp")
    try:
        with temporary.open("xb") as stream, tarfile.open(fileobj=stream, mode="w:gz") as archive:
            for relative in snapshot:
                archive.add(evidence / relative, arcname=evidence.name + "/" + relative, recursive=False)
        with tarfile.open(temporary, "r:gz") as archive:
            members = archive.getmembers()
            expected = {evidence.name + "/" + name: data for name, data in snapshot.items()}
            if len(members) != len(expected) or {item.name for item in members} != set(expected):
                raise ValueError("native archive member set does not match evidence")
            for member in members:
                if not member.isfile():
                    raise ValueError("native archive contains a non-regular member")
                data = archive.extractfile(member)
                if data is None:
                    raise ValueError("native archive member cannot be read back")
                digest = hashlib.sha256()
                size = 0
                with data:
                    for chunk in iter(lambda: data.read(1024 * 1024), b""):
                        size += len(chunk)
                        digest.update(chunk)
                if {"bytes": size, "sha256": digest.hexdigest()} != expected[member.name]:
                    raise ValueError("native archive did not preserve evidence bytes")
        current = {path.relative_to(evidence).as_posix(): {"bytes": path.stat().st_size, "sha256": sha256(path)}
                   for path in evidence.rglob("*") if path.is_file() and not path.is_symlink()}
        if current != snapshot or (evidence.stat().st_dev, evidence.stat().st_ino) != root_identity:
            raise ValueError("native evidence changed during archival")
        # Hard-link installation fails if another asset appeared in the meantime.
        os.link(temporary, output)
        temporary.unlink()
        shutil.rmtree(evidence)
        return {"file": str(output), "bytes": output.stat().st_size,
                "sha256": sha256(output), "evidence_files": len(snapshot)}
    finally:
        with contextlib.suppress(FileNotFoundError):
            temporary.unlink()


def save_report(path, report):
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(path.name + ".tmp")
    temporary.write_text(json.dumps(report, indent=2, ensure_ascii=False) + "\n")
    temporary.replace(path)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--source-root", type=Path, required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--arch", choices=tuple(ELF_MACHINES), required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--evidence-dir", type=Path, required=True)
    parser.add_argument("--case-timeout", type=float, default=180)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}|[0-9a-f]{64}", args.source_sha):
        parser.error("--source-sha must be a full lowercase Git commit hash")
    if not 0 < args.case_timeout <= 600:
        parser.error("--case-timeout must be within 0..600 seconds")
    binary, root = args.binary.resolve(), args.source_root.resolve()
    output, evidence = args.output.resolve(), args.evidence_dir.resolve()
    report = {"schema": 1, "passed": False, "status": "preflight", "cases": [],
              "timestamp_utc": datetime.now(timezone.utc).isoformat(),
              "source_sha": args.source_sha, "architecture": args.arch,
              "host_machine": platform.machine(), "platform": platform.platform(),
              "python": sys.version, "binary": str(binary), "evidence_dir": str(evidence),
              "scope": "Native Linux ELF and real PTY/X11 GUI acceptance under guarded private Xvfb",
              "pixel_readback": "not_performed", "ime_keyboard": "synthetic XTEST input; no hardware IME acceptance",
              "limitations": ["No native Wayland or hardware GPU/scanout acceptance.",
                              "No physical input-to-display latency measurement."],
              "wrapper_sha256": sha256(Path(__file__))}
    try:
        if platform.system() != "Linux" or platform.machine() != args.arch or struct.calcsize("P") != 8:
            raise ValueError(f"requires native 64-bit Linux {args.arch}; observed {platform.system()} {platform.machine()}")
        report["elf"] = inspect_elf(binary, args.arch)
        report["binary_sha256"] = sha256(binary)
        report["source_before"] = inspect_source(root)
        require_source(report["source_before"], args.source_sha)
        if output.is_relative_to(evidence):
            raise ValueError("summary output must be outside the evidence directory")
        if evidence.exists() and any(evidence.iterdir()):
            raise ValueError("choose an empty evidence directory; previous evidence is preserved")
        evidence.mkdir(parents=True, exist_ok=True)
        files = (*HARNESS_FILES, "tests/ioctl_resize_fault.c", "tools/isolated_xvfb.py", "tests/linux_native.py")
        subprocess.run(["git", "ls-files", "--error-unmatch", "--", *files], cwd=root,
                       stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, timeout=10, check=True)
        report["source_files_sha256"] = {name: sha256(root / name) for name in files}
        report["dependencies"] = {name: importlib.metadata.version(name) for name in ("python-xlib", "Pillow")}
        report["programs"] = {name: shutil.which(name) for name in ("Xvfb", "cc", "fc-match")}
        if not all(report["programs"].values()):
            raise ValueError(f"missing native GUI test dependencies: {report['programs']}")
        if ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) != 0:
            raise OSError("PR_SET_CHILD_SUBREAPER failed")
        report["status"] = "running"
        save_report(output, report)
        version = run_owned([str(binary), "--version"], evidence / "version", root, 30)
        version["stdout_text"] = (evidence / "version/stdout.log").read_text()[:4000]
        version["passed"] = process_passed(version) and version["stdout_text"].strip() == "ztty " + args.version
        report["version_check"] = version
        if not version["passed"]:
            raise ValueError("native version/startup check failed")
        for name in CASES:
            result = run_case(name, root, binary, evidence / name, args.case_timeout, report["binary_sha256"])
            report["cases"].append(result)
            save_report(output, report)
            print(json.dumps({"case": name, "passed": result["passed"],
                              "checks": result.get("checks_completed", 0), "error": result.get("error")}), flush=True)
            if sha256(binary) != report["binary_sha256"]:
                raise ValueError("binary changed during native acceptance")
        report["source_after"] = inspect_source(root)
        require_source(report["source_after"], args.source_sha)
        if any(sha256(root / name) != value for name, value in report["source_files_sha256"].items()):
            raise ValueError("acceptance source files changed while tests were running")
        report["binary_sha256_after"] = sha256(binary)
        report["passed"] = len(report["cases"]) == len(CASES) and all(case["passed"] for case in report["cases"])
        if any(case.get("screenshots") for case in report["cases"]):
            report["pixel_readback"] = "Actual XGetImage captures from owned X11 windows in private Xvfb"
    except Exception as error:
        report["passed"] = False
        report["error"] = repr(error)
        if isinstance(error, NativeGateInterrupted):
            report["interrupted_by_signal"] = error.signum
    report["status"] = ("passed" if report["passed"] else
                        "interrupted" if "interrupted_by_signal" in report else "failed")
    if evidence.is_dir():
        report["evidence_files"] = {
            str(path.relative_to(evidence)): {"bytes": path.stat().st_size, "sha256": sha256(path)}
            for path in sorted(evidence.rglob("*")) if path.is_file()}
    save_report(output, report)
    print(json.dumps({"passed": report["passed"], "source_sha": args.source_sha,
                      "architecture": args.arch, "output": str(output), "error": report.get("error")}), flush=True)
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    with termination_scope():
        raise SystemExit(main())
