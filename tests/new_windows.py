#!/usr/bin/env python3
"""Verify nonblocking batch creation on an owned X server, broker and PTYs."""
import argparse
import concurrent.futures
import json
import os
from pathlib import Path
import resource
import subprocess
import sys
import tempfile
import time

from Xlib import X, display, protocol

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "tools"))
from isolated_xvfb import start_xvfb


def wait_for(check):
    deadline = time.monotonic() + 6
    while not check():
        if time.monotonic() >= deadline:
            raise AssertionError("timed out waiting for fixture state")
        time.sleep(.02)


def exercise(binary, directory, server):
    env = dict(PATH=os.defpath, HOME=str(directory), SHELL="/bin/sh",
               DISPLAY=server.display, XDG_RUNTIME_DIR=str(directory), XMODIFIERS="@im=none",
               ZTTY_SOCKET=f"@ztty-new-{os.getpid()}-{time.monotonic_ns()}")
    if "LD_LIBRARY_PATH" in os.environ:
        env["LD_LIBRARY_PATH"] = os.environ["LD_LIBRARY_PATH"]
    config = directory / ".config" / "ztty"
    config.mkdir(parents=True)
    (config / "ztty.conf").write_text("renderer=software\n")
    connection = display.Display(server.display)

    def run(*args):
        return subprocess.run([binary, *args], env=env, cwd=directory,
                              capture_output=True, timeout=12)

    def cli(*args):
        proc = run(*args)
        assert proc.returncode == 0, (args, proc.stdout, proc.stderr)
        return json.loads(proc.stdout)

    def sessions():
        return {s["name"]: s for s in cli("list")["sessions"]}

    def mapped():
        return {w.id for w in connection.screen().root.query_tree().children
                if w.get_attributes().map_state == X.IsViewable}

    def open_windows(*args):
        result = cli("new", *args)
        created = result["created"]
        assert len(created) == result["requested"] and "error" not in result
        assert all(s["running"] and s["attached"] and s["workspace"] is None for s in created)
        ids = {s["window_id"] for s in created}
        assert len(ids) == len(created)
        assert len({s["pid"] for s in created}) == len(created)
        wait_for(lambda: ids <= mapped())
        return created

    def wm_close(session):
        window = connection.create_resource_object("window", session["window_id"])
        event = protocol.event.ClientMessage(
            window=window, client_type=connection.intern_atom("WM_PROTOCOLS"),
            data=(32, [connection.intern_atom("WM_DELETE_WINDOW"), X.CurrentTime, 0, 0, 0]))
        window.send_event(event, event_mask=0)
        connection.flush()
        wait_for(lambda: session["name"] not in sessions())

    assert run("new", "--count", "0").returncode == 1
    assert env["ZTTY_SOCKET"] not in Path("/proc/net/unix").read_text().split()
    # Existing ordinary launch blocks until its own window closes.
    keep = subprocess.Popen([binary, "--backend", "x11", "--name", "keep", "-e", "/bin/cat"],
                            env=env, cwd=directory, stdout=subprocess.DEVNULL)
    try:
        wait_for(lambda: "keep" in sessions())
        sentinel = sessions()["keep"]
        first, = open_windows()
        assert first["name"] == f"session-{first['id']}"
        wm_close(first)
        single, = open_windows("--count", "1", "--name", "named", "--backend", "x11", "--", "/bin/cat")

        # The command returns before any of the child jobs are allowed to finish.
        child = "import os,sys,time,pathlib; assert os.isatty(0); pathlib.Path(str(os.getpid())).write_text(os.ttyname(0)); time.sleep(120)"
        group = open_windows("--name", "work", "--count", "3", "-e", sys.executable, "-c", child)
        assert [s["name"] for s in group] == ["work-1", "work-2", "work-3"]
        files = [directory / str(s["pid"]) for s in group]
        wait_for(lambda: all(f.exists() for f in files))
        assert len({f.read_text() for f in files}) == 3
        wm_close(group[1])
        assert {group[0]["window_id"], group[2]["window_id"], sentinel["window_id"]} <= mapped()

        # A rename preserves the creation URI. Both URI and current name must
        # reserve their slots even if the collision occurs late in a batch.
        cli("rename", "work-3", "later-3")
        before = sessions()
        before_windows = mapped()
        bad = [("--count", v) for v in ("0", "-1", "17", "1.5", "abc", "99999999999999999999999")]
        bad += [("--count",), ("--count", "2", "--name", "."),
                ("--count", "2", "--name", "a"*63), ("--count", "3", "--name", "later"),
                ("--name", "work-3"), ("--name", "named"), ("--tab", "keep")]
        for args in bad:
            failed = run("new", *args)
            assert failed.returncode != 0, args
        assert before == sessions() and before_windows == mapped()

        # Boundaries include the generated numeric suffix, and automatic names
        # are checked even when a user previously chose a future session-N.
        boundary = open_windows("--count", "2", "--name", "a"*62)
        assert all(len(s["name"]) == 64 for s in boundary)
        assert boundary[0]["id"] == max(s["id"] for s in before.values()) + 1
        reserved, = open_windows("--name", f"session-{boundary[-1]['id']+3}")
        assert run("new", "--count", "2").returncode == 1
        assert set(sessions()) == set(before) | {s["name"] for s in [*boundary, reserved]}

        many = open_windows("--count", "16", "--name", "limit", "--geometry", "20x4")
        assert many[0]["id"] == reserved["id"] + 1
        for s in many:
            cli("close", s["uri"])
        with concurrent.futures.ThreadPoolExecutor(max_workers=2) as executor:
            races = list(executor.map(lambda _: run("new", "--count", "2", "--name", "race"), range(2)))
        assert sorted(p.returncode for p in races) == [0, 1]
        assert sorted(name for name in sessions() if name.startswith("race-")) == ["race-1", "race-2"]

        # Limit this fixture broker's descriptors so that PTY creation fails
        # during the batch. Restore its soft limit before any further operation.
        pid = sentinel["broker_pid"]
        limit = resource.prlimit(pid, resource.RLIMIT_NOFILE)
        fds = {int(p.name) for p in Path(f"/proc/{pid}/fd").iterdir()}
        # Leave nine free slots, irrespective of holes from closed windows.
        soft_limit = [fd for fd in range(max(fds) + 10) if fd not in fds][8] + 1
        try:
            resource.prlimit(pid, resource.RLIMIT_NOFILE, (soft_limit, limit[1]))
            partial = run("new", "--name", "partial", "--count", "16", "--geometry", "20x4")
        finally:
            resource.prlimit(pid, resource.RLIMIT_NOFILE, limit)
        assert partial.returncode == 1, (partial.stdout, partial.stderr)
        result = json.loads(partial.stdout)
        done = [s["name"] for s in result["created"]]
        assert result["error"] and 0 < len(done) < result["requested"] == 16
        assert result["failed_name"] == f"partial-{len(done)+1}"
        assert set(done) == {name for name in sessions() if name.startswith("partial-")}
        assert sessions()["keep"] == sentinel and keep.poll() is None
        wm_close(single)
        print(f"Verified independent windows, PTYs, immediate return, preflight, race and partial batch ({len(done)}/16)")
    finally:
        try:
            for session in sessions().values():
                cli("close", session["uri"])
            keep.wait(timeout=6)
            assert not sessions()
        finally:
            connection.close()
            if keep.poll() is None:
                keep.terminate()
                keep.wait(timeout=5)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--binary", type=Path, required=True)
    binary = parser.parse_args().binary.resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="ztty-new-") as tmp, start_xvfb() as server:
        exercise(binary, Path(tmp), server)
