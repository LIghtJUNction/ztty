#!/usr/bin/env python3
"""Owned PTY child for the native Wayland integration test."""
import base64
import json
import os
from pathlib import Path
import select
import signal
import sys
import time
import tty

root = Path(sys.argv[1])
root.mkdir(parents=True, exist_ok=True)
tty.setraw(0)
generation = 0
last_command = -1
winch = False

def changed(_signal, _frame):
    global winch
    winch = True

signal.signal(signal.SIGWINCH, changed)

def state():
    global generation, winch
    size = os.get_terminal_size(0)
    generation += 1
    data = {"pid": os.getpid(), "start_ticks": Path("/proc/self/stat").read_text().split()[21],
            "cols": size.columns, "rows": size.lines, "generation": generation,
            "command": last_command, "pwd_matches_cwd": os.environ.get("PWD") == os.getcwd(),
            "term": os.environ.get("TERM"), "program": os.environ.get("TERM_PROGRAM")}
    temp = root / "state.tmp"
    temp.write_text(json.dumps(data))
    temp.replace(root / "state.json")
    winch = False

os.write(1, b"\x1b[?25l\x1b[2J\x1b[H\x1b[48;2;192;32;48m" + b" " * 20 + b"\x1b[0m\r\n"
         + "native Wayland | ztty\r\n中文 العربية 👩🏽‍💻\r\n".encode())
state()
while True:
    if winch:
        state()
    command = root / "command.json"
    if command.exists():
        value = json.loads(command.read_text())
        if value["id"] != last_command:
            last_command = value["id"]
            if "bytes" in value:
                os.write(1, base64.b64decode(value["bytes"]))
            if "exit" in value:
                state()
                sys.exit(value["exit"])
            state()
    ready, _, _ = select.select([0], [], [], 0.02)
    if ready:
        data = os.read(0, 65536)
        if not data:
            break
        with (root / "input.bin").open("ab") as output:
            output.write(data)
