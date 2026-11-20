# Sixel GUI acceptance

The Linux X11 rendering gate passes all ten checks through a real persistent PTY,
MCP stdio, and an observer window. The native Hyprland/XWayland attempt is blocked
before any ztty window is created. These are distinct results.

## Reproduce

Install Xvfb, Python Xlib, and Pillow as test dependencies. The production binary
does not acquire these dependencies.

```sh
python3 tests/isolated_xvfb_test.py -v
python3 tests/sixel_gui.py \
  --binary target/release/ztty \
  --source-root . \
  --output artifacts/sixel-gui.json \
  --screenshot artifacts/sixel-gui.png

# Optional: the existing native desktop, with no focus or window movement command.
python3 tests/sixel_gui.py \
  --binary target/release/ztty --source-root . --display "$DISPLAY" \
  --output artifacts/sixel-native.json \
  --screenshot artifacts/sixel-native.png
```

The harness creates a private configuration, broker socket, persistent PTY child,
and MCP operator. It attaches the GUI with `attach --observe`. All test input and
PTY resize requests go through the operator. The observer's ownership and focus
are checked. Only its own window is resized. Its terminal replies establish the
cell geometry and primary DA response `62;4` before pixels are measured.

The image is decoded by ztty and captured using XGetImage from the actual window.
Expected RGB bytes are generated independently from the test pattern. The test
compares every pixel in each asserted region, including transparent holes and
cleared pixels. PNG capture happens after those assertions.

## Recorded development-candidate result

[Machine-readable passing report](measurements/sixel-gui-xvfb-2026-11-18.json),
[captured PNG（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view).

The run timestamp is 2026-11-17T22:15:39 UTC; the filenames use the 2026-11-18
development batch name. Binary SHA-256:

```text
794b053a3a299bac199a3d713f118cb1603064d619fb4f357efa9c7f03c4c716
```

This is preliminary development evidence. The binary was frozen before subsequent
integration fixes, and its supplied source checkout contains recorded tracked
changes. The report includes the source commit, tracked changes, relevant source
hashes, harness hash, helper hash, and binary hash. Source checkout hashes describe
the state at test start; they do not prove which sources produced a pre-existing
binary. Release acceptance requires a rerun against the final recorded build.

| Check | Actual assertion |
| --- | --- |
| Persistent observer | Real PTY, unchanged MCP input owner, unfocused observer, nonactivation hint |
| RGB image | Exact 320 × 180 pixels, repeat encoding and multiple color planes |
| Resize | Same image after MCP changes PTY from 80 × 30 to 90 × 32; owned window dimensions change |
| Transparent background | Every unpainted hole preserves the older green image |
| Opaque background | Every unpainted hole becomes current SGR RGB (23, 51, 77) |
| Erase display | ED2 removes visible image pixels |
| Oversized raster | No pixels appear; following text is present in the terminal screen |
| Unterminated DCS | No partial image is presented |
| CAN cancellation | No cancelled image is presented; following text recovers |
| Recovery | A complete valid image renders correctly after rejection and cancellation |

The report records ten passed checks and no cleanup errors. Measured cells are
10 × 25 pixels; the final window is 948 × 848. The final screenshot was inspected:
black background, consistent spacing, aligned colored blocks, readable title and
footer, and no overlapping text. Its SHA-256 is:

```text
aa64fca868aeca8b1f23b9fe34b39d61addefd77cb3ee7c710c1e5c535435d75
```

[Actual Sixel window capture（历史附件）](https://drive.google.com/file/d/10Lf0VuCzh5I2y54kYfwgHCcyFO7KXvpb/view)

## Native display blocker

[Machine-readable failed native attempt](measurements/sixel-gui-hyprland-2026-11-18.json)
records zero completed GUI checks. The existing desktop reports Hyprland 0.57.2
and DISPLAY=:0. Opening that X11 display fails with:

```text
DisplayConnectionError(':0', '[Errno 111] Connection refused')
```

Read-only endpoint inspection found the following:

| Resource | Observed state |
| --- | --- |
| /tmp/.X0-lock | Existing inode 552 |
| /tmp/.X11-unix/X0 | Filesystem path absent |
| /tmp/.X11-unix/X0_ | Existing socket inode 553 |
| Kernel listener for X0_ | Socket inode 60990 |
| Kernel listener for X0 | Socket inode 60991 |
| Abstract @/tmp/.X11-unix/X0 listener | Absent |

The lock and backup socket have ctime 2026-11-17T09:33:17.902941 UTC, over twelve
hours before this acceptance attempt. A separate read-only process inspection
associated the kernel listeners with Xwayland PID 3942, file descriptors 60 and
61, started with inherited `-listenfd` descriptors. An absent filesystem path can
therefore coexist with a live native server. The evidence establishes why this
client cannot connect; it does not establish how the endpoint became absent.

No native socket was renamed or removed, no compositor or native Xwayland process
was restarted, and no native window was opened by this failed attempt. Native
acceptance remains incomplete pending a connectable display.

## Shared private-display helper

`tools/isolated_xvfb.py` provides a standard-library-only helper for Linux GUI and
benchmark harnesses:

```python
from isolated_xvfb import start_xvfb

with start_xvfb(screen="1600x1000x24", stderr=log, dpi=None) as server:
    environment["DISPLAY"] = server.display
    # Launch and close the harness's own clients here.
    evidence = server.evidence
```

The helper selects an explicit random display in 100..30000 and excludes the
observed native display. It checks X11 locks, ordinary and abstract sockets,
backup socket names, and kernel Unix socket records before launch. An atomic
private lease prevents collisions between cooperating harnesses. After launch,
the lock PID, kernel listener inode ownership, and filesystem socket peer PID
must match the exact Xvfb child.

Cleanup terminates only that child and removes only its identity-checked lease.
X11 locks and sockets are left to Xvfb's own shutdown; the helper never manually
removes a native display resource. Callers must close their clients before
closing the server.

Two real regression tests pass: a live existing Xvfb survives a forced allocation
collision with its PID lock unchanged; an abstract-only listener also blocks
allocation without a lock file. The first test additionally compares the native
display's endpoint snapshot before and after cleanup.

## Later source-bound candidate checks

The preliminary snapshot above remains unchanged. Subsequent clean-source
Linux x86_64 candidates `fa2f201` and `2dc6769` each passed the same ten
Sixel GUI scenarios within the complete Linux release gate. Windows x86_64
also passed six native Sixel color, transparency, deletion, cancellation and
recovery groups. ARM64 failures and the remaining macOS pixel gap are recorded
in the [native candidate history](native-candidates-2026-11-18.md). None of
these Xvfb runs repairs or validates the unavailable current Hyprland endpoint.

## Acceptance boundaries

Xvfb validates X11 drawing and software-visible pixels. It does not validate
native Wayland, compositor scanout, GPU memory, display latency, macOS, or Windows.
Sixel protocol limits and independent decoder comparisons remain documented in
[protocols.md](protocols.md) and [the reference report](measurements/sixel-linux.json).
The renderer's cell-anchored images are drawn behind text; this gate does not
claim the full VT340 framebuffer's text/image overpainting semantics.
