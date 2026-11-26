fn main() {
    println!("cargo:rerun-if-changed=src/macos_surface.m");
    println!("cargo:rerun-if-env-changed=AR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") {
        let mut build = cc::Build::new();
        build.file("src/macos_surface.m").flag("-fobjc-arc");
        // Apple's linker reads only Apple's archive format. When building on
        // macOS, `ar` on PATH may be GNU binutils (Homebrew, conda), whose
        // archives it rejects ("archive member '/' not a mach-o file"). Use
        // the system archiver unless the caller chose one.
        if cfg!(target_os = "macos") && std::env::var_os("AR").is_none() {
            build.archiver("/usr/bin/ar");
        }
        build.compile("vkore_macos_surface");
        for framework in ["AppKit", "QuartzCore", "OpenGL", "IOSurface"] {
            println!("cargo:rustc-link-lib=framework={framework}");
        }
    }
}
