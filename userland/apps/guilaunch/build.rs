//! Linker args for `guilaunch` — mirrors `userland/apps/hello/build.rs`.

use std::path::PathBuf;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let linker_script: PathBuf = [&manifest_dir, "..", "..", "linker.ld"].iter().collect();
    let linker_script = linker_script
        .canonicalize()
        .expect("userland/linker.ld must exist");

    println!(
        "cargo:rustc-link-arg-bin=guilaunch=-T{}",
        linker_script.display()
    );
    println!("cargo:rustc-link-arg-bin=guilaunch=-static");
    println!("cargo:rustc-link-arg-bin=guilaunch=-no-pie");
    println!("cargo:rustc-link-arg-bin=guilaunch=-zmax-page-size=0x1000");
    println!("cargo:rustc-link-arg-bin=guilaunch=-znoexecstack");
    println!("cargo:rustc-link-arg-bin=guilaunch=-znow");
    println!("cargo:rustc-link-arg-bin=guilaunch=-nostdlib");

    println!("cargo:rerun-if-changed={}", linker_script.display());
}
