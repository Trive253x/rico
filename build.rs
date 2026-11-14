//! Windows: links the app icon into the .exe files (Explorer, shortcuts,
//! Start menu, task bar). `assets/icon.res` is made by `assets/make_icon.py`
//! from `assets/icon.svg`: the build needs no resource compiler and no crate.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.res");
    nasm_check();
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.ends_with("windows-msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo");
        let res = std::path::Path::new(&dir).join("assets").join("icon.res");
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
}

/// OpenH264 (stream video) uses its SIMD assembly only if `nasm` is on the
/// PATH, and silently goes without otherwise: say so, the video is then
/// several times slower.
fn nasm_check() {
    let arch = std::env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_default();
    if !matches!(arch.as_str(), "x86" | "x86_64") || std::env::var_os("OPENH264_NO_ASM").is_some() {
        return;
    }
    let found = std::process::Command::new("nasm")
        .arg("-v")
        .output()
        .is_ok_and(|o| o.status.success());
    if !found {
        println!(
            "cargo:warning=NASM not found: OpenH264 is built without its assembly and stream video is several times slower (install NASM, then `cargo clean -p openh264-sys2`)"
        );
    }
}
