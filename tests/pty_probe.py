#!/usr/bin/env python3
"""Run as the terminal's child: verify real PTY protocol replies and round trips."""
import json
import os
import re
import select
import statistics
import sys
import termios
import time
import tty


def read_reply(pattern, timeout=2.0):
    data = bytearray()
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        ready, _, _ = select.select([0], [], [], max(0, deadline - time.monotonic()))
        if not ready:
            break
        data.extend(os.read(0, 4096))
        found = re.search(pattern, data)
        if found:
            return bytes(found.group(0))
    raise AssertionError(f"reply {pattern!r} missing; received {bytes(data)!r}")


def exchange(command, expected):
    os.write(1, command)
    reply = read_reply(re.escape(expected))
    assert reply == expected, (command, reply, expected)


def main():
    result_path = sys.argv[1]
    query_only = "--query-only" in sys.argv[2:]
    previous = termios.tcgetattr(0)
    workload = "query-only" if query_only else "home-write-query"
    result = {"schema": 1, "checks": [], "workload": workload, "ok": False}
    try:
        tty.setraw(0)
        checks = [
            ("status", b"\x1b[5n", b"\x1b[0n"),
            ("cursor", b"\x1b[3;7H\x1b[6n", b"\x1b[3;7R"),
            ("private_cursor", b"\x1b[?6n", b"\x1b[?3;7R"),
            ("sync_reset", b"\x1b[?2026$p", b"\x1b[?2026;2$y"),
            ("sync_set", b"\x1b[?2026h\x1b[?2026$p", b"\x1b[?2026;1$y"),
            ("sync_end", b"\x1b[?2026l\x1b[?2026$p", b"\x1b[?2026;2$y"),
            ("bracketed_paste_set", b"\x1b[?2004h\x1b[?2004$p", b"\x1b[?2004;1$y"),
            ("kitty_push", b"\x1b[>1u\x1b[?u", b"\x1b[?1u"),
            ("kitty_alternate_stack", b"\x1b[?1049h\x1b[>8u\x1b[?u", b"\x1b[?8u"),
            ("kitty_primary_stack", b"\x1b[?1049l\x1b[?u", b"\x1b[?1u"),
            ("kitty_pop", b"\x1b[<u\x1b[?u", b"\x1b[?0u"),
            ("unknown_mode", b"\x1b[?65000$p", b"\x1b[?65000;0$y"),
        ]
        selected_checks = checks[:2] if "--benchmark-only" in sys.argv[2:] else checks
        for name, command, expected in selected_checks:
            exchange(command, expected)
            result["checks"].append(name)
        # DSR confirms logical cursor state, not rendering or presentation completion.
        if query_only:
            exchange(b"\x1b[1;2H\x1b[6n", b"\x1b[1;2R")
        timed_command = b"\x1b[6n" if query_only else b"\x1b[Hq\x1b[6n"
        samples = []
        for index in range(1100):
            start = time.perf_counter_ns()
            exchange(timed_command, b"\x1b[1;2R")
            duration = (time.perf_counter_ns() - start) / 1000
            if index >= 100:
                samples.append(duration)
        samples.sort()
        result["pty_roundtrip_us"] = {
            "p50": round(statistics.median(samples), 3),
            "p95": round(samples[int(len(samples) * .95)], 3),
            "p99": round(samples[int(len(samples) * .99)], 3),
            "samples": len(samples),
            "warmup_samples": 100,
            "workload": workload,
            "request_hex": timed_command.hex(),
            "scope": "child write, PTY, logical terminal state, response, PTY, child read; DSR does not wait for rendered frames or scanout",
        }
        result["environment"] = {name: os.environ.get(name) for name in ["TERM", "COLORTERM", "TERM_PROGRAM"]}
        result["ok"] = True
    except Exception as error:
        result["error"] = repr(error)
    finally:
        termios.tcsetattr(0, termios.TCSANOW, previous)
        with open(result_path, "w", encoding="utf-8") as output:
            json.dump(result, output, indent=2)
            output.write("\n")
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
