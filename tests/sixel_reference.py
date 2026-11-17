#!/usr/bin/env python3
"""Compare zterm Sixel RGBA pixels with libsixel and independent fixture pixels.

Build the probe with cargo build -p zterm --example sixel_probe, then run this
script. Pillow and sixel2png are test dependencies, never production dependencies.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import platform
import struct
import subprocess
from datetime import datetime, timezone

from PIL import Image


def digest(data):
    return hashlib.sha256(data).hexdigest()


def run_length(data):
    result = bytearray()
    offset = 0
    while offset < len(data):
        end = offset + 1
        while end < len(data) and data[end] == data[offset]:
            end += 1
        length = end - offset
        if length >= 3:
            result.extend(b"!" + str(length).encode() + bytes([data[offset]]))
        else:
            result.extend(data[offset:end])
        offset = end
    return result


def encode(width, height, indices, definitions, tail=b"", controls=False):
    body = bytearray(f'"1;1;{width};{height}'.encode())
    for index, values in definitions.items():
        body.extend((f"#{index};" + ";".join(map(str, values))).encode())
    palette = sorted(set(indices))
    for y in range(0, height, 6):
        for index in palette:
            body.extend(f"#{index}".encode())
            row = bytearray()
            for x in range(width):
                bits = sum(1 << bit for bit in range(min(6, height-y))
                           if indices[(y+bit)*width+x] == index)
                row.append(63 + bits)
            body.extend(run_length(row))
            body.extend(b"$\r\n" if controls else b"$")
        if y + 6 < height:
            body.extend(b"-")
    return b"\x1bP7;1q" + body + tail + b"\x1b\\"


def fixtures():
    rgb_percent = [(0, 0, 0), (100, 0, 0), (0, 100, 0), (0, 0, 100),
                   (20, 40, 80), (100, 100, 100)]
    rgb = [tuple((v*255+50)//100 for v in color)+(255,) for color in rgb_percent]
    yield "rgb_raster_repeat", 37, 19, rgb, {i:(2, *c) for i,c in enumerate(rgb_percent)}, b"", False
    colors = [(0,0,255,255), (255,0,0,255), (0,255,0,255),
              (128,128,128,255), (0,0,0,255)]
    values = [(1,0,50,100), (1,120,50,100), (1,240,50,100),
              (1,0,50,0), (1,240,0,100)]
    yield "hls_dec_hues", 29, 17, colors, dict(enumerate(values)), b"", False
    yield "palette_redefinition", 19, 13, [(0,0,255,255), (0,255,0,255)], {0:(2,100,0,0),1:(2,0,100,0)}, b"#0;2;0;0;100$", False
    yield "embedded_controls", 41, 25, rgb, {i:(2, *c) for i,c in enumerate(rgb_percent)}, b"", True
    defaults = [(0,0,0),(20,20,80),(80,13,13),(20,80,20),
                (80,20,80),(20,80,80),(80,80,20),(53,53,53),
                (26,26,26),(33,33,60),(60,26,26),(33,60,33),
                (60,33,60),(33,60,60),(60,60,33),(80,80,80)]
    colors = [tuple((v*255+50)//100 for v in c)+(255,) for c in defaults]
    yield "vt340_palette", 67, 23, colors, {}, b"", False
    yield "hls_white_endpoint", 11, 7, [(255,255,255,255)], {0:(1,120,100,100)}, b"", False


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--probe", type=Path, default=Path("target/debug/examples/sixel_probe"))
    parser.add_argument("--output", type=Path, default=Path("docs/measurements/sixel-linux.json"))
    parser.add_argument("--artifacts", type=Path, default=Path("artifacts/sixel-reference"))
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[1]
    probe = args.probe.resolve()
    args.artifacts.mkdir(parents=True, exist_ok=True)
    results = []
    discrepancies = []
    version=subprocess.run(["sixel2png","--version"],capture_output=True,text=True,check=True).stdout.splitlines()[0]
    for name, width, height, colors, definitions, tail, controls in fixtures():
        indices = [(x//4 + y//6) % len(colors) for y in range(height) for x in range(width)]
        stream = encode(width, height, indices, definitions, tail, controls)
        expected = b"".join(bytes(colors[index]) for index in indices)
        (args.artifacts / (name+".six")).write_bytes(stream)
        reference = subprocess.run(["sixel2png"], input=stream, capture_output=True, check=True, timeout=10)
        png = Image.open(io.BytesIO(reference.stdout)).convert("RGBA")
        assert png.size == (width, height), (name, "libsixel dimensions", png.size)
        reference_matches = png.tobytes() == expected
        if not reference_matches:
            # libsixel 1.10.5's HLS conversion has a known observable endpoint
            # error. Keep the independent white expectation and record the
            # discrepancy instead of teaching zterm to reproduce that error.
            assert name == "hls_white_endpoint" and version == "sixel2png 1.10.5", (name, "unexpected libsixel discrepancy")
            assert png.tobytes() == bytes((1,255,254,255)) * (width*height)
            discrepancies.append({"fixture":name,"reference_decoder":version,
                "input_hls":[120,100,100],"expected_rgba":[255,255,255,255],
                "reference_rgba":[1,255,254,255],"zterm_matches_expected":True})
        png.save(args.artifacts / (name+".png"))
        chunks = [1, 2, 7, 256, 65536]
        for chunk in chunks:
            process = subprocess.run([str(probe), str(chunk)], input=stream, capture_output=True, check=True, timeout=10)
            size = struct.unpack("<II", process.stdout[:8])
            assert size == (width, height), (name, chunk, size)
            assert process.stdout[8:] == expected, (name, chunk, "zterm differs from fixture pixels")
            if reference_matches:
                assert process.stdout[8:] == png.tobytes(), (name, chunk, "zterm differs from libsixel")
        results.append({"fixture":name,"width":width,"height":height,
                        "stream_sha256":digest(stream),"rgba_sha256":digest(expected),
                        "chunk_sizes":chunks,"zterm_matches_fixture":True,"zterm_matches_libsixel":reference_matches})
    # A larger deterministic fixture for the real renderer/screenshot gate.
    width,height=320,180
    definitions={0:(2,0,0,0),1:(2,20,40,80),2:(2,0,100,0),3:(2,100,40,0)}
    indices=[0 if x%80<8 or y%60<8 else 1+(x//80+y//60)%3 for y in range(height) for x in range(width)]
    demo=encode(width,height,indices,definitions)
    (args.artifacts/"demo.six").write_bytes(demo)
    sources=["crates/zgraphics/src/sixel.rs","crates/zgraphics/src/lib.rs","crates/zterm/src/terminal.rs",
             "crates/zterm/examples/sixel_probe.rs","tests/sixel_reference.py"]
    report={"timestamp_utc":datetime.now(timezone.utc).isoformat(),"platform":platform.platform(),
            "reference_decoder":version,"probe_sha256":digest(probe.read_bytes()),
            "source_sha256":{name:digest((root/name).read_bytes()) for name in sources},
            "fixtures":results,"fixture_comparisons_passed":sum(len(r["chunk_sizes"]) for r in results),
            "reference_comparisons_passed":sum(len(r["chunk_sizes"]) for r in results if r["zterm_matches_libsixel"]),
            "reference_discrepancies":discrepancies,
            "limitations":["Pixel comparison is separate from native GUI presentation.",
                "The independent fixtures cover opaque RGB/HLS/palette data; transparency, cursor modes, cancellation and quotas are covered by Rust tests."]}
    args.output.parent.mkdir(parents=True,exist_ok=True)
    args.output.write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps({"fixture_passed":report["fixture_comparisons_passed"],
        "reference_passed":report["reference_comparisons_passed"],"reference_discrepancies":discrepancies,
        "output":str(args.output),"demo":str(args.artifacts/"demo.six")}))


if __name__ == "__main__":
    main()
