//! Windows: links the app icon into the .exe files (Explorer, shortcuts,
//! Start menu, task bar). `assets/icon.res` is made by `assets/make_icon.py`
//! from `assets/icon.svg`: the build needs no resource compiler and no crate.

fn main() {
    println!("cargo:rerun-if-changed=assets/icon.res");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target.ends_with("windows-msvc") {
        let dir = std::env::var("CARGO_MANIFEST_DIR").expect("set by cargo");
        let res = std::path::Path::new(&dir).join("assets").join("icon.res");
        println!("cargo:rustc-link-arg-bins={}", res.display());
    }
}
