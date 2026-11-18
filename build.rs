fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("linux") {
        // Stable Rust's precompiled alloc can reference unwind cleanup even when
        // this binary uses panic=abort. Link the real system unwinder after the
        // Rust archives: an earlier -lgcc_s is discarded by GNU ld --as-needed.
        // link-arg-bin affects only ztty, not tests or other workspace crates.
        println!("cargo::rustc-link-arg-bin=ztty=-lgcc_s");
    }
}
