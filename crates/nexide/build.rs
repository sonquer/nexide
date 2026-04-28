//! Build script: re-exports nexide's `napi_*` symbols from final
//! binaries so that `dlopen`'d native addons can resolve them.

fn main() {
    let target = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    match target.as_str() {
        "linux" | "android" | "freebsd" | "netbsd" | "dragonfly" | "openbsd" => {
            println!("cargo:rustc-link-arg-bins=-Wl,--export-dynamic");
            println!("cargo:rustc-link-arg-tests=-Wl,--export-dynamic");
        }
        "macos" | "ios" => {
            println!("cargo:rustc-link-arg-bins=-Wl,-export_dynamic");
            println!("cargo:rustc-link-arg-tests=-Wl,-export_dynamic");
        }
        _ => {}
    }
}
