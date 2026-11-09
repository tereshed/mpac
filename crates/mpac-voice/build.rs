fn main() {
    println!("cargo:rerun-if-changed=src/apple.m");
    let os = std::env::var("CARGO_CFG_TARGET_OS").unwrap();
    if os == "macos" || os == "ios" {
        cc::Build::new()
            .file("src/apple.m")
            .flag("-fobjc-arc")
            .compile("mpac_voice_recording");
        println!("cargo:rustc-link-lib=framework=AVFoundation");
        println!("cargo:rustc-link-lib=framework=Foundation");
    }
}
