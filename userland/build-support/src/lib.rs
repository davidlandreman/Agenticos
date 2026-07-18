use std::path::PathBuf;

/// Emit the common linker contract for a freestanding AgenticOS binary.
pub fn configure(binary: &str) {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let linker_script: PathBuf = [&manifest_dir, "..", "..", "linker.ld"].iter().collect();
    let linker_script = linker_script
        .canonicalize()
        .expect("userland/linker.ld must exist");
    for argument in [
        format!("-T{}", linker_script.display()),
        "-static".into(),
        "-no-pie".into(),
        "-zmax-page-size=0x1000".into(),
        "-znoexecstack".into(),
        "-znow".into(),
        "-nostdlib".into(),
    ] {
        println!("cargo:rustc-link-arg-bin={binary}={argument}");
    }
    println!("cargo:rerun-if-changed={}", linker_script.display());
}
