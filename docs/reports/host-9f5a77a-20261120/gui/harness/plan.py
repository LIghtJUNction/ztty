"""Exact final GUI coverage; this module never starts a process."""
from pathlib import Path
import sys
from wayland_cli import EXPECTED_CHECK_NAMES

DEFAULT_SWAY = Path("PROJECT_WORKTREE/artifacts/native-wayland-20261119/artifacts/wayland-test-tools/root/usr/bin/sway")


def build_plan(repo, binary, output, fixtures, sway, swaymsg, ime_peer, no_wayland_binary):
    steps = []

    def add(name, script, arguments=(), report="report.json", renderer=None,
            timeout=300, checks=None, local=False, required_check_names=(), strict_checks=False):
        directory = output / f"{len(steps) + 1:02d}-{name}"
        path = Path(__file__).with_name(script) if local else repo / "tests" / script
        argv = [sys.executable, str(path), "--binary", str(binary)]
        argv.extend(str(value).replace("{OUT}", str(directory)) for value in arguments)
        steps.append({
            "name": name, "command": argv, "directory": str(directory),
            "report": str(directory / report), "timeout_seconds": timeout,
            "renderer": renderer, "expected_checks": checks,
            "required_check_names": list(required_check_names), "strict_checks": strict_checks,
        })

    add("xim-all", "xim_gui.py",
        ["--scenario", "all", "--output", "{OUT}/report.json"], timeout=420)
    add("input-software", "input_gui.py",
        ["--source-root", repo, "--renderer", "software", "--output", "{OUT}/report.json",
         "--screenshot", "{OUT}/input.png"], renderer="software", timeout=420)
    add("clipboard-transfer", "clipboard_transfer_gui.py",
        ["--output", "{OUT}/report.json"])
    add("copy-paste", "copy_paste_gui.py", ["--artifacts", "{OUT}"],
        report="acceptance.json")
    add("osc52-gpu", "osc52_gui.py",
        ["--renderer", "gpu", "--artifacts", "{OUT}"],
        report="osc52.json", renderer="gpu")
    for renderer in ("software", "gpu"):
        add(f"graphics-{renderer}", "graphics_extensions_gui.py",
            ["--renderer", renderer, "--artifacts", "{OUT}"],
            report="graphics-extensions.json", renderer=renderer)
    for renderer in ("software", "gpu"):
        add(f"text-{renderer}", "text_gpu_gui.py",
            ["--renderer", renderer, "--output", "{OUT}/report.json",
             "--screenshots", "{OUT}/screenshots"], renderer=renderer)
    add("retained-frame-software", "retained_frame_gui.py",
        ["--renderer", "software", "--expect", "clean", "--output", "{OUT}/report.json"],
        renderer="software")
    for renderer in ("software", "gpu"):
        add(f"repaint-{renderer}", "repaint_gui.py",
            ["--renderer", renderer, "--output", "{OUT}/report.json",
             "--screenshots", "{OUT}/screenshots"], renderer=renderer, timeout=420)
    for renderer in ("software", "gpu"):
        add(f"native-ui-{renderer}", "native_ui_gui.py",
            ["--source-root", repo, "--renderer", renderer, "--output", "{OUT}/report.json",
             "--screenshot", "{OUT}/native-ui.png"], renderer=renderer, timeout=420)
    add("workspaces", "workspaces.py",
        ["--output", "{OUT}/report.json", "--screenshot", "{OUT}/workspaces.png"], timeout=420)
    add("shared-gui", "shared_gui_smoke.py",
        ["--output", "{OUT}/report.json"], timeout=420)
    add("plugin-cli-current", "plugin_cli.py",
        ["--example", fixtures / "example.so", "--fixtures", fixtures,
         "--output", "{OUT}/report.json"])
    add("plugin-cli-legacy", "plugin_cli.py",
        ["--example", fixtures / "before-example.so", "--fixtures", fixtures,
         "--output", "{OUT}/report.json"])
    add("plugin-gui-current", "plugin_gui.py",
        ["--example", fixtures / "example.so", "--fixture", fixtures / "fixture-valid.so",
         "--artifacts", "{OUT}"], report="plugin-gui.json", timeout=420)
    add("wayland-shared-gui", "wayland_gui.py",
        ["--sway", sway, "--swaymsg", swaymsg, "--ime-peer", ime_peer, "--output", "{OUT}"],
        report="validation.json", renderer="Wayland EGL on software Mesa", checks=65,
        required_check_names=(
            "ime_requests_terminal_without_fake_surrounding",
            "ime_preedit_is_visible_without_pty_input",
            "ime_commit_exact_utf8_once", "ime_chunk_boundaries_preserve_utf8",
            "ime_cannot_delete_unowned_shell_buffer", "ime_recovers_after_invalid_delete",
            "ime_peer_clean_exit",
            "x11_first_routes_both_backends_in_one_broker",
            "x11_first_keeps_separate_ptys", "x11_first_mixed_sessions_closed",
            "wayland_first_routes_both_backends_in_one_broker",
            "wayland_first_keeps_separate_ptys", "wayland_first_mixed_sessions_closed",
            "mixed-x11-first-x11_actual_x11_window",
            "mixed-wayland-first-x11_actual_x11_window",
            "mixed_backend_tests_preserve_original_session",
            "mcp_reads_same_native_pty", "native_observer_reuses_same_pty",
            "reattach_identity_stays_stable",
        ), strict_checks=True)
    add("wayland-cli", "wayland_cli.py",
        ["--no-wayland-binary", no_wayland_binary, "--output", "{OUT}/report.json"],
        checks=len(EXPECTED_CHECK_NAMES), timeout=120, local=True,
        required_check_names=EXPECTED_CHECK_NAMES, strict_checks=True)
    return steps
