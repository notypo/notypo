fn main() {
    // libc's macOS bindings link libiconv, which we never call; drop unused
    // dylibs so dyld has one library less to load at startup.
    if std::env::var("CARGO_CFG_TARGET_VENDOR").as_deref() == Ok("apple") {
        println!("cargo:rustc-link-arg-bins=-Wl,-dead_strip_dylibs");
    }
    println!("cargo:rerun-if-changed=build.rs");
}
