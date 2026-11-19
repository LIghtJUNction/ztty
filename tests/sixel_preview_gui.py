#!/usr/bin/env python3
"""Check image-to-text preview replacement through a real PTY and owned Xvfb."""
import argparse
import contextlib
import hashlib
import json
from pathlib import Path
import tempfile

from gui_smoke import Gui, ROOT
from graphics_smoke import BG, command, expect_pixel, pixel


RED = (255, 0, 0, 255)
GREEN = (0, 255, 0, 255)
BLUE = (0, 0, 255, 255)


def at(col, row):
    return f"\x1b[{row + 1};{col + 1}H"


def solid_sixel(width, height, color=(100, 0, 0)):
    body = f'"1;1;{width};{height}#1;2;{color[0]};{color[1]};{color[2]}'
    body += "-".join(f"!{width}~" for _ in range((height + 5) // 6))
    return "\x1bP0;1q" + body + "\x1b\\"


def show_preview(gui, kitty=False):
    gui.reset()
    gui.emit("\x1b[?1049h" + at(2, 1) + "Generated preview: image -> text")
    if kitty:
        payload = command(
            "a=T,f=32,s=1,v=1,i=991,c=20,r=6,C=1,q=2", bytes(RED)
        )
    else:
        payload = solid_sixel(20 * gui.cw, 6 * gui.ch)
    gui.emit(at(80, 4) + payload)
    expect_pixel(gui, 95, 7, RED, x=gui.cw // 2, y=gui.ch // 2)


def replace_with_text(gui, mode, artifacts):
    show_preview(gui, kitty=mode == "kitty-delete")
    if mode == "spaces":
        clear = "".join(at(80, row) + " " * 20 for row in range(4, 10))
    elif mode == "ECH":
        clear = "".join(at(80, row) + "\x1b[20X" for row in range(4, 10))
    elif mode == "EL":
        clear = "".join(at(80, row) + "\x1b[K" for row in range(4, 10))
    elif mode == "ED0":
        clear = at(80, 4) + "\x1b[J"
    elif mode == "ED1":
        clear = at(100, 10) + "\x1b[1J"
    elif mode == "ED2":
        clear = "\x1b[2J"
    else:
        clear = command("a=d,d=I,i=991,q=2")
    gui.emit(clear + at(80, 4) + "TEXT PREVIEW" + at(80, 5) + "plain text file" + at(0, 0))
    expect_pixel(gui, 95, 7, BG, x=gui.cw // 2, y=gui.ch // 2)
    filename = f"preview-{mode}.png"
    gui.stable_image().save(artifacts / filename)
    return {
        "clear_mode": mode,
        "expected": BG,
        "observed": pixel(gui, 95, 7, x=gui.cw // 2, y=gui.ch // 2),
        "screenshot": filename,
    }


def partial_and_independent_images(gui, artifacts):
    show_preview(gui)
    gui.emit(at(50, 5) + solid_sixel(4 * gui.cw, 2 * gui.ch, (0, 100, 0)))
    gui.emit(
        at(90, 6)
        + command("a=T,f=32,s=1,v=1,i=992,c=2,r=1,C=1,q=2", bytes(BLUE))
    )
    expect_pixel(gui, 51, 5, GREEN, x=gui.cw // 2, y=gui.ch // 2)
    expect_pixel(gui, 90, 6, BLUE, x=gui.cw // 2, y=gui.ch // 2)
    gui.emit(at(85, 7) + " " + at(0, 0))
    expect_pixel(gui, 85, 7, BG, x=gui.cw // 2, y=gui.ch // 2)
    # Probe both sides at the exact erased-cell boundary, not only its center.
    expect_pixel(gui, 85, 7, BG, x=0, y=gui.ch // 2)
    expect_pixel(gui, 84, 7, RED, x=gui.cw - 1, y=gui.ch // 2)
    expect_pixel(gui, 86, 7, RED, x=0, y=gui.ch // 2)
    expect_pixel(gui, 85, 6, RED, x=gui.cw // 2, y=gui.ch - 1)
    expect_pixel(gui, 85, 8, RED, x=gui.cw // 2, y=0)
    expect_pixel(gui, 51, 5, GREEN, x=gui.cw // 2, y=gui.ch // 2)
    expect_pixel(gui, 90, 6, BLUE, x=gui.cw // 2, y=gui.ch // 2)
    gui.stable_image().save(artifacts / "preview-partial.png")
    gui.emit("".join(at(80, row) + "\x1b[20X" for row in range(4, 10)) + at(0, 0))
    expect_pixel(gui, 95, 7, BG, x=gui.cw // 2, y=gui.ch // 2)
    expect_pixel(gui, 51, 5, GREEN, x=gui.cw // 2, y=gui.ch // 2)
    # Kitty remains even where the same cells erase the overlapping Sixel.
    expect_pixel(gui, 90, 6, BLUE, x=gui.cw // 2, y=gui.ch // 2)
    gui.stable_image().save(artifacts / "preview-isolated.png")
    return {
        "one_cell_erased": True,
        "four_uncovered_boundaries_preserved": True,
        "nonintersecting_sixel_preserved": True,
        "overlapping_kitty_preserved": True,
        "screenshots": ["preview-partial.png", "preview-isolated.png"],
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/ztty")
    parser.add_argument("--artifacts", type=Path, default=ROOT / "artifacts/sixel-preview")
    parser.add_argument("--font", default="monospace:size=11")
    args = parser.parse_args()
    args.artifacts.mkdir(parents=True, exist_ok=True)
    binary = args.binary.resolve()
    result = {
        "schema": 1,
        "ok": False,
        "binary": str(binary),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "scope": "owned Xvfb; generated images and text only",
        "checks": [],
    }
    gui = None
    with tempfile.TemporaryDirectory(prefix="ztty-sixel-preview-") as directory:
        try:
            gui = Gui(binary, args.font, directory)
            result["cell_pixels"] = [gui.cw, gui.ch]
            for mode in ["spaces", "ECH", "EL", "ED0", "ED1", "ED2", "kitty-delete"]:
                try:
                    detail = replace_with_text(gui, mode, args.artifacts)
                    result["checks"].append({"name": mode, "ok": True, **detail})
                except Exception as error:
                    result["checks"].append({"name": mode, "ok": False, "error": repr(error)})
            try:
                detail = partial_and_independent_images(gui, args.artifacts)
                result["checks"].append({"name": "partial-and-independent", "ok": True, **detail})
            except Exception as error:
                result["checks"].append({"name": "partial-and-independent", "ok": False, "error": repr(error)})
            result["ok"] = all(check["ok"] for check in result["checks"])
        except Exception as error:
            result["error"] = repr(error)
        finally:
            if gui is not None:
                with contextlib.suppress(Exception):
                    gui.close()
    result["passed"] = sum(check["ok"] for check in result["checks"])
    (args.artifacts / "sixel-preview.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result))
    return 0 if result["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
