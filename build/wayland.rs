//! Build the optional Wayland ABI bridge using system protocol definitions.
use std::{env, path::Path, process::Command};

fn output(program: &str, args: &[&str]) -> String {
    let result = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("cannot run {program}: {error}"));
    assert!(
        result.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    String::from_utf8(result.stdout).expect("tool output must be UTF-8")
}
fn run(command: &mut Command) {
    let status = command.status().expect("cannot run Wayland build tool");
    assert!(status.success(), "Wayland build tool failed: {command:?}");
}
pub fn build(out: &Path) {
    println!("cargo:rerun-if-changed=native/wayland_bridge.c");
    println!("cargo:rerun-if-changed=build/wayland.rs");
    let protocols = output(
        "pkg-config",
        &["--variable=pkgdatadir", "wayland-protocols"],
    );
    let mut generated = Vec::new();
    for (name, relative) in [
        ("xdg-shell", "stable/xdg-shell/xdg-shell.xml"),
        (
            "primary-selection",
            "unstable/primary-selection/primary-selection-unstable-v1.xml",
        ),
        (
            "text-input",
            "unstable/text-input/text-input-unstable-v3.xml",
        ),
    ] {
        let xml = Path::new(protocols.trim()).join(relative);
        println!("cargo:rerun-if-changed={}", xml.display());
        let header = out.join(format!("{name}-client-protocol.h"));
        let protocol = out.join(format!("{name}-protocol.c"));
        run(Command::new("wayland-scanner")
            .arg("client-header")
            .arg(&xml)
            .arg(&header));
        run(Command::new("wayland-scanner")
            .arg("private-code")
            .arg(&xml)
            .arg(&protocol));
        generated.push(protocol);
    }
    println!("cargo:rerun-if-changed=native/wayland_services.h");
    println!("cargo:rerun-if-changed=native/wayland_services.c");
    let flags = output(
        "pkg-config",
        &[
            "--cflags",
            "wayland-client",
            "wayland-egl",
            "wayland-cursor",
            "xkbcommon",
        ],
    );
    let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let mut objects = Vec::new();
    let mut sources = vec![
        Path::new("native/wayland_bridge.c").to_path_buf(),
        Path::new("native/wayland_services.c").to_path_buf(),
    ];
    sources.extend(generated);
    for source in sources {
        let object = out.join(source.file_name().unwrap()).with_extension("o");
        run(Command::new(&compiler)
            .args(["-std=c11", "-O2", "-fPIC", "-Wall", "-Wextra", "-Werror"])
            .args(flags.split_whitespace())
            .arg("-I")
            .arg(out)
            .arg("-c")
            .arg(source)
            .arg("-o")
            .arg(&object));
        objects.push(object);
    }
    run(
        Command::new(env::var_os("AR").unwrap_or_else(|| "ar".into()))
            .arg("crs")
            .arg(out.join("libztty_wayland_bridge.a"))
            .args(objects),
    );
    println!("cargo:rustc-link-lib=static=ztty_wayland_bridge");
    for library in [
        "wayland-client",
        "wayland-egl",
        "wayland-cursor",
        "xkbcommon",
    ] {
        println!("cargo:rustc-link-lib={library}");
    }
}
