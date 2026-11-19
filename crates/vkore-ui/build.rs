mod build_support;

fn main() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    println!("cargo:rerun-if-env-changed=VKORE_BUILD_COMMIT");
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build_support.rs");
    let info = build_support::inspect(root, std::env::var("VKORE_BUILD_COMMIT").ok().as_deref())
        .expect("identify vKore build");
    for path in info.watch {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    println!("cargo:rustc-env=VKORE_BUILD_REVISION={}", info.revision);
}
