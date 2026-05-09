//! Pass per-binary linker arguments for the `hello` app so the custom linker
//! script and `-z` security flags only apply to the final binary, not to the
//! `runtime` rlib or `core`/`compiler_builtins` artifacts.
//!
//! The linker script lives at `userland/linker.ld`. From this build script's
//! perspective (running with `CARGO_MANIFEST_DIR = userland/apps/hello`),
//! that's `../../linker.ld`. We resolve it to an absolute path so the linker
//! invocation is robust to whatever working directory rustc picks.

use std::path::PathBuf;

fn main() {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let linker_script: PathBuf = [&manifest_dir, "..", "..", "linker.ld"].iter().collect();
    let linker_script = linker_script
        .canonicalize()
        .expect("userland/linker.ld must exist");

    // -T <script>: use our linker script (sets ENTRY, base address, PHDRS).
    println!(
        "cargo:rustc-link-arg-bin=hello=-T{}",
        linker_script.display()
    );
    // Static, non-PIE — matches D3 (the U6 loader expects ET_EXEC).
    println!("cargo:rustc-link-arg-bin=hello=-static");
    println!("cargo:rustc-link-arg-bin=hello=-no-pie");
    // NX/WX hygiene at the ELF level (D11).
    println!("cargo:rustc-link-arg-bin=hello=-zmax-page-size=0x1000");
    println!("cargo:rustc-link-arg-bin=hello=-znoexecstack");
    println!("cargo:rustc-link-arg-bin=hello=-znow");
    // No dynamic interpreter — we're a freestanding binary.
    println!("cargo:rustc-link-arg-bin=hello=-nostdlib");

    // Re-run if the script changes.
    println!("cargo:rerun-if-changed={}", linker_script.display());
}
