fn main() {
    // These Windows Media Foundation/DirectShow import libraries are needed by
    // the static FFmpeg bundle on Windows. Gate them so non-Windows builds do
    // not fail while looking for Windows-only system libraries.
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("windows") {
        println!("cargo:rustc-link-lib=mfuuid");
        println!("cargo:rustc-link-lib=strmiids");
    }
}
