#!/usr/bin/env python3
"""Raw PTY child controlled by gui_smoke.py through a private Unix socket."""
import argparse
import json
import os
import select
import signal
import socket
import termios
import time
import tty


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--socket", required=True)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    original = termios.tcgetattr(0)
    tty.setraw(0)
    incoming = bytearray()
    winches = 0
    running = True
    log = open(args.log, "w", encoding="utf-8", buffering=1)

    def record(kind, **data):
        log.write(json.dumps({"event": kind, "monotonic": time.monotonic(), **data}) + "\n")

    def size():
        value = os.get_terminal_size(0)
        return {"cols": value.columns, "rows": value.lines}

    def changed(_signal, _frame):
        nonlocal winches
        winches += 1
        record("resize", **size())

    def drain():
        while select.select([0], [], [], 0)[0]:
            data = os.read(0, 65536)
            if not data:
                break
            if len(incoming) + len(data) > 8 * 1024 * 1024:
                raise RuntimeError("GUI test input exceeded 8 MiB")
            incoming.extend(data)
            record("input", hex=data.hex())

    signal.signal(signal.SIGWINCH, changed)
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(args.socket)
    os.chmod(args.socket, 0o600)
    server.listen(1)
    clients = {}
    record("ready", pid=os.getpid(), **size())
    try:
        while running:
            ready, _, _ = select.select([0, server, *clients], [], [], 0.2)
            if 0 in ready:
                drain()
            if server in ready:
                connection, _ = server.accept()
                clients[connection] = bytearray()
            for connection in [item for item in ready if item in clients]:
                data = connection.recv(65536)
                if not data:
                    del clients[connection]
                    connection.close()
                    continue
                clients[connection].extend(data)
                while b"\n" in clients[connection]:
                    line, _, rest = clients[connection].partition(b"\n")
                    clients[connection] = bytearray(rest)
                    command = json.loads(line)
                    action = command["action"]
                    reply = {"ok": True}
                    if action == "write":
                        data = command["text"].encode("utf-8")
                        while data:
                            written = os.write(1, data)
                            data = data[written:]
                    elif action == "clear":
                        drain()
                        incoming.clear()
                    elif action == "snapshot":
                        drain()
                        reply.update(input_hex=incoming.hex(), winches=winches, pid=os.getpid(), **size())
                    elif action == "quit":
                        running = False
                    else:
                        raise ValueError("unknown child action: " + action)
                    record("command", action=action)
                    connection.sendall(json.dumps(reply).encode("utf-8") + b"\n")
    finally:
        for connection in clients:
            connection.close()
        server.close()
        termios.tcsetattr(0, termios.TCSANOW, original)
        log.close()


if __name__ == "__main__":
    main()
