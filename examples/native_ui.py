#!/usr/bin/env python3
"""Small ZUI/1 producer: native scene + existing Kitty image ID + action intents.

Run inside ztty: python3 examples/native_ui.py
Or start directly: ztty -e python3 examples/native_ui.py

The demo interprets "continue" and "reset" locally. Actions never become shell
commands. Press Escape to leave button focus, then q to quit.
"""
import base64
import json
import os
import select
import sys
import termios
import time
import tty

PREFIX = b"\x1b_ZUI;1;"
END = b"\x1b\\"
IMAGE_ID = 7001


def write(data):
    pending = memoryview(data)
    while pending:
        pending = pending[os.write(1, pending):]


def frame(value):
    return PREFIX + json.dumps(value, ensure_ascii=False, separators=(",", ":")).encode() + END


def image_bytes():
    width, height = 128, 64
    pixels = bytearray()
    for y in range(height):
        for x in range(width):
            if x < 64:
                color = (230, 70 + y, 60, 255)
            else:
                color = (60, 130, 230 - y, 255)
            pixels.extend(color)
    return (f"\x1b_Ga=t,f=32,s={width},v={height},i={IMAGE_ID},q=2;".encode()
            + base64.b64encode(pixels) + END)


def scene(revision):
    return {
        "v": 1, "op": "replace", "surface": 1, "rev": revision, "events": True, "root": 1,
        "nodes": [
            {"id": 1, "type": "column", "children": [2, 3, 4, 5, 6, 9]},
            {"id": 2, "type": "text", "style": "heading", "text": "Native UI, over a terminal stream"},
            {"id": 3, "type": "text", "style": "muted",
             "text": "A bounded scene. One image ID. Structured actions back to the app."},
            {"id": 4, "type": "progress", "value": 6500, "label": "Preparing preview · 65%"},
            {"id": 5, "type": "image", "image_id": IMAGE_ID,
             "cols": 52, "rows": 9, "alt": "Red and blue demo image"},
            {"id": 6, "type": "row", "children": [7, 8]},
            {"id": 7, "type": "button", "label": "Continue", "action": "continue", "enabled": True},
            {"id": 8, "type": "button", "label": "Reset progress", "action": "reset", "enabled": True},
            {"id": 9, "type": "text", "style": "muted",
             "text": "Click to focus · Tab to select · Enter to activate · Esc, then q to quit"},
        ],
    }


class Decoder:
    def __init__(self):
        self.buffer = bytearray()

    def feed(self, data):
        self.buffer.extend(data)
        packets = []
        ordinary = bytearray()
        while self.buffer:
            start = self.buffer.find(PREFIX)
            if start < 0:
                # Retain a possible prefix split across reads.
                keep = next((n for n in range(min(len(PREFIX) - 1, len(self.buffer)), 0, -1)
                             if PREFIX.startswith(self.buffer[-n:])), 0)
                ordinary.extend(self.buffer[:-keep] if keep else self.buffer)
                self.buffer = self.buffer[-keep:] if keep else bytearray()
                break
            ordinary.extend(self.buffer[:start])
            del self.buffer[:start]
            end = self.buffer.find(END, len(PREFIX))
            if end < 0:
                if len(self.buffer) > 65536:
                    self.buffer.clear()
                break
            payload = bytes(self.buffer[len(PREFIX):end])
            del self.buffer[:end + len(END)]
            try:
                packets.append(json.loads(payload))
            except (ValueError, UnicodeError):
                pass
        return packets, bytes(ordinary)


def main():
    if not (os.isatty(0) and os.isatty(1)):
        raise SystemExit("Run the demo in a ztty terminal.")
    saved = termios.tcgetattr(0)
    decoder = Decoder()
    revision = None
    try:
        tty.setraw(0)
        write(frame({"v": 1, "op": "capabilities"}))
        deadline = time.monotonic() + 2
        while revision is None and time.monotonic() < deadline:
            ready, _, _ = select.select([0], [], [], max(0, deadline - time.monotonic()))
            if not ready:
                break
            packets, _ = decoder.feed(os.read(0, 8192))
            for packet in packets:
                if packet.get("type") == "capabilities" and packet.get("v") == 1:
                    if packet.get("supported") is True:
                        revision = packet["last_rev"] + 1
                    else:
                        deadline = 0
                        break
        if revision is None:
            write(b"ZUI/1 is unavailable in this terminal.\r\n")
            return
        write(b"\x1b[?1049h\x1b[?25l")
        write(image_bytes())
        write(frame(scene(revision)))
        while True:
            data = os.read(0, 8192)
            if not data:
                break
            packets, ordinary = decoder.feed(data)
            if b"q" in ordinary or b"\x03" in ordinary or b"\x04" in ordinary:
                break
            for packet in packets:
                if (packet.get("type") != "action" or packet.get("surface") != 1
                        or packet.get("rev") != revision):
                    continue
                action = packet.get("action")
                if action not in ("continue", "reset"):
                    continue
                progress = 10000 if action == "continue" else 0
                next_revision = revision + 1
                write(frame({
                    "v": 1, "op": "update", "surface": 1, "base_rev": revision, "rev": next_revision,
                    "set": [
                        {"id": 4, "value": progress,
                         "label": "Preview ready · 100%" if progress else "Ready to start · 0%"},
                        {"id": 9, "text": f"Received action {action!r}; event {packet['event_id']}. No shell command was executed."},
                    ],
                }))
                revision = next_revision
    finally:
        if revision is not None:
            write(frame({"v": 1, "op": "clear", "surface": 1, "base_rev": revision, "rev": revision + 1}))
        write(b"\x1b[?25h\x1b[?1049l")
        termios.tcsetattr(0, termios.TCSADRAIN, saved)


if __name__ == "__main__":
    main()
