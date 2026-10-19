fn main() {
    // Debug builds look for an FFmpeg fetched into `binaries/` (named with the
    // target triple, as Tauri's externalBin convention requires).
    println!("cargo:rustc-env=SONORA_TARGET_TRIPLE={}", std::env::var("TARGET").unwrap_or_default());
    tauri_build::build()
}
