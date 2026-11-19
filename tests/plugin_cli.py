#!/usr/bin/env python3
"""Exercise real zplugin libraries and bounded native workers; no GUI or user sessions."""
import argparse
import ctypes
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def stat(pid):
    values = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return {"state": values[0], "parent": int(values[1]), "group": int(values[2]), "start": values[19]}


def wait(predicate, label, timeout=4):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = predicate()
        if value:
            return value
        time.sleep(.01)
    raise AssertionError(label)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", required=True, type=Path)
    parser.add_argument("--example", required=True, type=Path)
    parser.add_argument("--fixtures", required=True, type=Path)
    parser.add_argument("--output", type=Path, default=Path("artifacts/zplugin/cli.json"))
    args = parser.parse_args()
    binary, example, fixtures = args.binary.resolve(strict=True), args.example.resolve(strict=True), args.fixtures.resolve(strict=True)
    libraries = {name: fixtures / f"fixture-{name}.so" for name in ("valid", "wrong-interface", "pinned")}
    report = {"ok": False, "checks": [], "cleanup_errors": [], "binary": str(binary),
              "binary_sha256": digest(binary), "example_sha256": digest(example),
              "library_sha256": {name: digest(path) for name, path in libraries.items()},
              "source_commit": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
              "source_sha256": {name: digest(ROOT / name) for name in (
                  "src/plugin_host.rs", "crates/ztty-plugin-api/src/lib.rs",
                  "tests/fixtures/text-plugin/src/lib.rs", "tests/plugin_cli.py")}}
    # Adopt only this test's orphaned fault-fixture descendants, so even the
    # deliberate channel-leak and parent-death tests leave no zombies behind.
    assert ctypes.CDLL(None).prctl(36, 1, 0, 0, 0) == 0
    owned = {}
    parent = None
    with tempfile.TemporaryDirectory(prefix="ztty-plugin-cli-") as temporary:
        root = Path(temporary)
        try:
            def check(name, library, mode="run", data=b"", error=None, expected=None):
                marker = root / (name + ".log")
                env = dict(os.environ, ZTTY_PLUGIN_TEST_MARKER=str(marker))
                started = time.monotonic()
                result = subprocess.run([str(binary), "plugin", mode, str(library)], input=data,
                    capture_output=True, env=env, timeout=6)
                duration = time.monotonic() - started
                lines = marker.read_text().splitlines() if marker.exists() else []
                workers = [int(line.split()[1]) for line in lines if line.startswith("create ")]
                children = [int(line.split()[1]) for line in lines if line.startswith("child ")]
                for pid in children:
                    try:
                        state = stat(pid)
                        assert state["parent"] == os.getpid(), (pid, state)
                        owned[pid] = state["start"]
                        assert state["state"] == "Z", ("fixture descendant survived", pid, state)
                        waited, _ = os.waitpid(pid, 0)
                        assert waited == pid
                    except FileNotFoundError:
                        pass
                assert all(not Path(f"/proc/{pid}").exists() for pid in workers), "worker was not reaped"
                if error is None:
                    assert result.returncode == 0, result.stderr
                    if expected is not None:
                        assert result.stdout == expected, (name, len(result.stdout), result.stdout[:200])
                else:
                    assert result.returncode != 0 and not result.stdout, (name, result)
                    assert error.encode() in result.stderr, result.stderr
                report["checks"].append({"name": name, "ok": True, "exit_code": result.returncode,
                    "duration_ms": duration * 1000, "input_bytes": len(data), "output_bytes": len(result.stdout),
                    "output_sha256": hashlib.sha256(result.stdout).hexdigest(),
                    "stderr": result.stderr.decode(errors="replace"), "lifecycle": lines,
                    "worker_reaped": True, "descendants_reaped": len(children)})
                return result, lines

            checked, _ = check("example-load-and-unload", example, mode="check")
            metadata = json.loads(checked.stdout)
            assert metadata == {"interface": "dev.project-z.ztty.text", "interface_version": 1,
                                "plugin_version": [1, 0, 0], "unloaded": True}, metadata
            check("example-unicode-relative-indent", example,
                  data="    世界\r\n      café\r\n    end".encode(),
                  expected="世界\r\n  café\r\nend".encode())
            valid = libraries["valid"]
            _, lifecycle = check("instance-lifecycle", valid, data=b"plain", expected=b"plain")
            assert [line.split()[0] for line in lifecycle] == ["create", "transform", "destroy"]
            check("one-mib-roundtrip", valid, data=b"X" * (1024 * 1024), expected=b"X" * (1024 * 1024))
            _, lines = check("oversized-input-before-load", valid, data=b"X" * (1024 * 1024 + 1), error="input exceeds")
            assert not lines
            _, lines = check("invalid-input-before-load", valid, data=b"\xff", error="input must be UTF-8")
            assert not lines
            check("empty-output", valid, data=b"empty", expected=b"")
            check("invalid-result-count", valid, data=b"invalid-count", error="output exceeds")
            check("invalid-output-utf8", valid, data=b"invalid-utf8", error="invalid UTF-8")
            check("caught-plugin-panic", valid, data=b"panic", error="Panicked")
            check("native-plugin-crash", valid, data=b"crash", error="exited abnormally")
            check("wall-clock-timeout", valid, data=b"timeout", error="timed out")
            check("cpu-limit", valid, data=b"cpu", error="exited abnormally")
            check("child-timeout-process-group", valid, data=b"child-timeout", error="timed out")
            check("leaked-channel-process-group", valid, data=b"leaked-channel", error="timed out")
            check("unsolicited-stdout-rejected", valid, data=b"stdout-junk", error="invalid extension")
            check("interface-version-mismatch", libraries["wrong-interface"], mode="check", error="interface rejected")
            check("noncooperative-library-rejected", libraries["pinned"], mode="check", error="cannot load extension")
            check("missing-library", root / "missing.so", mode="check", error="cannot load extension")
            result, _ = check("resource-and-descriptor-limits", valid, data=b"limits")
            limits = result.stdout.decode()
            assert "fds=4\n" in limits, limits
            for name, value in (("Max cpu time", "1"), ("Max core file size", "0"),
                                ("Max open files", "64"), ("Max address space", "268435456")):
                row = next(line for line in limits.splitlines() if line.startswith(name))
                assert row[len(name):].split()[:2] == [value, value], row

            marker = root / "parent-death.log"
            parent = subprocess.Popen([str(binary), "plugin", "run", str(valid)],
                stdin=subprocess.PIPE, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                env=dict(os.environ, ZTTY_PLUGIN_TEST_MARKER=str(marker)))
            parent.stdin.write(b"timeout")
            parent.stdin.close()
            wait(lambda: marker.exists() and "transform" in marker.read_text(), "worker starts before parent death")
            worker = int(marker.read_text().splitlines()[0].split()[1])
            state = stat(worker)
            assert state["parent"] == parent.pid and state["group"] == worker, state
            owned[worker] = state["start"]
            parent.kill()
            parent.wait(timeout=3)
            parent = None
            wait(lambda: stat(worker)["state"] == "Z", "worker dies with its launching process")
            assert os.waitpid(worker, 0)[0] == worker
            report["checks"].append({"name": "parent-death-terminates-worker", "ok": True, "worker_reaped": True})
            report["ok"] = True
        except Exception as error:
            import traceback
            report.update(error=str(error), traceback=traceback.format_exc())
        finally:
            if parent is not None:
                parent.kill()
                parent.wait(timeout=3)
            for pid, token in owned.items():
                try:
                    state = stat(pid)
                    if state["start"] == token and state["parent"] == os.getpid():
                        os.kill(pid, signal.SIGKILL)
                        os.waitpid(pid, 0)
                except (FileNotFoundError, ProcessLookupError, ChildProcessError):
                    pass
                except Exception as error:
                    report["cleanup_errors"].append(str(error))
    report["ok"] &= not report["cleanup_errors"]
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n")
    print(json.dumps({"ok": report["ok"], "checks": len(report["checks"]), "output": str(args.output)}))
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
