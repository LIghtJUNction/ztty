//! Linux CLI output in termlens's emulator, not ztty native-window rendering.
#![cfg(target_os = "linux")]

use std::{path::PathBuf, time::Duration};
use termlens::Terminal;

fn spawn(args: &[&str], cols: u16, rows: u16) -> termlens::Result<Terminal> {
    let binary = PathBuf::from(std::env::var_os("ZTTY_BINARY").expect(
        "set ZTTY_BINARY to an absolute path to the real cargo build --locked --release executable",
    ));
    assert!(binary.is_absolute(), "ZTTY_BINARY must be absolute");
    Terminal::builder()
        .env_clear()
        .env("LANG", "C.UTF-8")
        .size(cols, rows)
        .timeout(Duration::from_secs(5))
        .args(args)
        .spawn(binary)
}

#[test]
fn help_preserves_unicode_and_wraps_at_pty_width() -> termlens::Result<()> {
    for cols in [40, 120] {
        // All help rows fit: avoid making scrollback timing part of this check.
        let mut terminal = spawn(&["--help"], cols, 200)?;
        let status = terminal.wait_exit()?;
        let screen = terminal.screen();
        assert_eq!(status.code(), Some(0), "{screen}");
        assert_eq!(screen.size(), (cols, 200));
        assert!(
            screen
                .row_text(0)
                .starts_with("ztty — shared no_std terminal"),
            "{screen}"
        );
        let text = screen.logical_text();
        assert!(
            text.contains("Usage: ztty [OPTIONS] [-e COMMAND [ARG...]]"),
            "{screen}"
        );
        assert!(
            text.contains("Use ztty mcp --help for transport options."),
            "{screen}"
        );
        assert_eq!(screen.row_wrapped(0), cols == 40, "{screen}");
        assert!(!screen.alternate_screen(), "{screen}");
    }
    Ok(())
}

#[test]
fn version_exits_successfully_without_display() -> termlens::Result<()> {
    let mut terminal = spawn(&["--version"], 80, 24)?;
    let status = terminal.wait_exit()?;
    let screen = terminal.screen();
    assert_eq!(status.code(), Some(0), "{screen}");
    let manifest = include_str!("../../../Cargo.toml");
    let version = manifest
        .split("[package]\n")
        .nth(1)
        .unwrap()
        .lines()
        .find_map(|line| {
            line.strip_prefix("version = \"")
                .and_then(|s| s.strip_suffix('"'))
        })
        .unwrap();
    assert_eq!(
        screen.row_text(0).trim_end(),
        format!("ztty {version}"),
        "{screen}"
    );
    assert!(!screen.alternate_screen(), "{screen}");
    Ok(())
}

#[test]
fn invalid_geometry_reports_error_and_exit_one() -> termlens::Result<()> {
    let mut terminal = spawn(&["--geometry", "1x24"], 100, 24)?;
    let status = terminal.wait_exit()?;
    let screen = terminal.screen();
    assert_eq!(status.code(), Some(1), "{screen}");
    assert_eq!(
        screen.row_text(0).trim_end(),
        "ztty: geometry must be 2..512 columns by 2..256 rows",
        "{screen}"
    );
    assert!(!screen.alternate_screen(), "{screen}");
    Ok(())
}
