fn main() {
    println!("cargo:rerun-if-changed=src/remote_layer.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/remote_layer.m")
            .flag("-fobjc-arc")
            .compile("vkore_remote_layer");
        for framework in ["AppKit", "QuartzCore"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
