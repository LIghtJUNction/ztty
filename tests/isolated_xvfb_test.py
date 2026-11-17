#!/usr/bin/env python3
"""Real-process and abstract-socket collision gates for private Xvfb allocation."""
import os
from pathlib import Path
import re
import secrets
import socket
import subprocess
import sys
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1]/"tools"))
import isolated_xvfb


class IsolationTests(unittest.TestCase):
    def test_existing_server_remains_alive_and_native_endpoint_unchanged(self):
        native = re.search(r":([0-9]+)(?:\.[0-9]+)?$", os.environ.get("DISPLAY", ""))
        before = isolated_xvfb.display_endpoints(int(native[1])) if native else None
        with isolated_xvfb.start_xvfb(screen="640x480x24", stderr=subprocess.DEVNULL) as server:
            self.assertGreaterEqual(server.number, 100)
            self.assertLessEqual(server.number, 30000)
            self.assertTrue(server.evidence["pid_lock_verified"])
            self.assertTrue(server.evidence["filesystem_peer_pid_verified"])
            self.assertTrue(server.evidence["owned_listeners"])
            with patch("isolated_xvfb.secrets.randbelow", return_value=server.number-100):
                with self.assertRaisesRegex(RuntimeError, "could not reserve"):
                    isolated_xvfb.start_xvfb(stderr=subprocess.DEVNULL)
            self.assertIsNone(server.process.poll())
            self.assertEqual(isolated_xvfb._pid_lock(server.number), server.process.pid)
        self.assertIsNotNone(server.process.poll())
        self.assertFalse(Path(f"/tmp/ztty-xvfb-{server.number}.lease").exists())
        if native:
            self.assertEqual(before, isolated_xvfb.display_endpoints(int(native[1])))

    def test_abstract_only_listener_blocks_allocation_without_a_lockfile(self):
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            for _ in range(64):
                number = 100+secrets.randbelow(29901)
                if isolated_xvfb._unavailable(number):
                    continue
                try:
                    listener.bind(f"\0/tmp/.X11-unix/X{number}")
                    break
                except OSError:
                    continue
            else:
                self.fail("cannot reserve an abstract-only test socket")
            listener.listen(1)
            self.assertFalse(Path(f"/tmp/.X{number}-lock").exists())
            self.assertFalse(Path(f"/tmp/.X11-unix/X{number}").exists())
            self.assertTrue(isolated_xvfb._unavailable(number))
            with patch("isolated_xvfb.secrets.randbelow", return_value=number-100):
                with self.assertRaisesRegex(RuntimeError, "could not reserve"):
                    isolated_xvfb.start_xvfb(stderr=subprocess.DEVNULL)
            self.assertGreaterEqual(listener.fileno(), 0)


if __name__ == "__main__":
    unittest.main()
