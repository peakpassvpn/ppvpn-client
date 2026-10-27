//! macOS: compiles src/objc_catch.m, which keeps Objective-C exceptions out
//! of Rust (autostart.rs).

fn main() {
    println!("cargo:rerun-if-changed=src/objc_catch.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("macos") {
        return;
    }
    cc::Build::new()
        .file("src/objc_catch.m")
        .flag("-fobjc-exceptions")
        .compile("ppvpn_objc_catch");
    println!("cargo:rustc-link-lib=framework=Foundation");
}
