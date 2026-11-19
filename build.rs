#[path = "build/wayland.rs"]
mod wayland_build;

use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=native/font_bridge.c");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("linux") {
        return;
    }
    // Keep the original no_std executable linkage. Precompiled alloc may
    // reference cleanup even with panic=abort; put the real system unwinder
    // after Rust archives so --as-needed does not discard it.
    println!("cargo::rustc-link-arg-bin=ztty=-lgcc_s");
    let out = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR"));
    if env::var_os("CARGO_FEATURE_WAYLAND").is_some() {
        wayland_build::build(&out);
    }
    let object = out.join("font_bridge.o");
    let flags = Command::new("pkg-config")
        .args(["--cflags", "fontconfig", "freetype2", "harfbuzz"])
        .output()
        .expect("pkg-config is required to locate the native font libraries");
    assert!(
        flags.status.success(),
        "fontconfig, freetype2 and harfbuzz development headers are required: {}",
        String::from_utf8_lossy(&flags.stderr)
    );
    let compiler = env::var_os("CC").unwrap_or_else(|| "cc".into());
    let status = Command::new(compiler)
        .args(["-std=c11", "-O2", "-fPIC", "-Wall", "-Wextra", "-Werror"])
        .args(String::from_utf8(flags.stdout).unwrap().split_whitespace())
        .args(["-c", "native/font_bridge.c", "-o"])
        .arg(&object)
        .status()
        .expect("could not run the native C compiler");
    assert!(status.success(), "native font ABI bridge failed to compile");
    let status = Command::new(env::var_os("AR").unwrap_or_else(|| "ar".into()))
        .arg("crs")
        .arg(out.join("libztty_font_bridge.a"))
        .arg(object)
        .status()
        .expect("could not run ar");
    assert!(
        status.success(),
        "could not archive the native font ABI bridge"
    );
    println!("cargo:rustc-link-search=native={}", out.display());
    for library in [
        "static=ztty_font_bridge",
        "fontconfig",
        "freetype",
        "harfbuzz",
        "m",
        "EGL",
        "GLESv2",
    ] {
        println!("cargo:rustc-link-lib={library}");
    }
}
