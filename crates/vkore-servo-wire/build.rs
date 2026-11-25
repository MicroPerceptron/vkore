fn main() {
    println!("cargo:rerun-if-changed=src/macos_surface.m");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        cc::Build::new()
            .file("src/macos_surface.m")
            .flag("-fobjc-arc")
            .compile("vkore_macos_surface");
        for framework in ["AppKit", "QuartzCore", "OpenGL", "IOSurface"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
