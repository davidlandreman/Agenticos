use std::path::{Path, PathBuf};

/// Mint a blank 64 MiB FAT32 image at `path` if it doesn't already
/// exist. Subsequent `./build.sh` runs reuse the on-disk file so /data
/// state survives reboots; passing `--clean` to build.sh removes the
/// target dir (and with it data.img) for a fresh start.
fn ensure_data_image(path: &Path, size: u64) {
    if path.exists() {
        return;
    }
    use std::fs::OpenOptions;
    use std::io::{Seek, SeekFrom};
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .expect("create data.img");
    file.set_len(size).expect("set_len data.img");
    file.seek(SeekFrom::Start(0)).expect("seek data.img");
    let opts = fatfs::FormatVolumeOptions::new()
        .fat_type(fatfs::FatType::Fat32)
        .volume_label(*b"AGENTIC-DAT");
    fatfs::format_volume(&mut file, opts).expect("format data.img as FAT32");
    eprintln!(
        "Minted blank {} MiB FAT32 at {}",
        size / 1024 / 1024,
        path.display()
    );
}

fn main() {
    // Detect build profile (debug or release)
    let profile = std::env::var("PROFILE").unwrap_or_else(|_| "debug".to_string());

    // Resolve the manifest dir once; everything below is relative to it so
    // worktrees, conductor.build workspaces, and CI all build correctly.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Honor CARGO_TARGET_DIR if cargo set it; otherwise fall back to <manifest>/target.
    let target_dir = std::env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| manifest_dir.join("target"));

    // Tell cargo to re-run this script if the kernel binary changes
    let kernel_rel = format!("x86_64-unknown-none/{profile}/agenticos");
    println!(
        "cargo:rerun-if-changed={}",
        target_dir.join(&kernel_rel).display()
    );
    // Also re-run if assets directory changes
    println!(
        "cargo:rerun-if-changed={}",
        manifest_dir.join("assets").display()
    );

    let out_dir = target_dir.join("bootloader");

    // Create output directory if it doesn't exist
    std::fs::create_dir_all(&out_dir).ok();

    // Path to the kernel binary (debug or release based on profile)
    let kernel = target_dir.join(&kernel_rel);

    // Check if we're in the second pass (kernel exists)
    eprintln!("Build profile: {profile}");
    eprintln!("Checking for Kernel Code: {}", kernel.display());
    if kernel.exists() {
        eprintln!("Creating bootloader images...");

        // Create disk image builder with the kernel
        let mut builder = bootloader::DiskImageBuilder::new(kernel.clone());

        // Add assets folder to the disk image
        let assets_dir = manifest_dir.join("assets");
        if assets_dir.exists() {
            eprintln!("Adding assets to disk image...");

            // Read the assets directory and add each file
            if let Ok(entries) = std::fs::read_dir(&assets_dir) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_file() {
                            let file_name = entry.file_name().to_string_lossy().to_string();
                            let source_path = entry.path();
                            let dest_path = format!("/{file_name}");

                            eprintln!("  Adding {dest_path}");
                            builder.set_file(dest_path.clone(), source_path);
                        }
                    }
                }
            }
        }

        // create a BIOS disk image
        let bios_path = out_dir.join("bios.img");
        builder.create_bios_image(&bios_path).unwrap();

        // Create a new builder for UEFI (builders are consumed on use)
        let mut uefi_builder = bootloader::DiskImageBuilder::new(kernel);

        // Add assets to UEFI image too
        if assets_dir.exists() {
            if let Ok(entries) = std::fs::read_dir(&assets_dir) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_file() {
                            let file_name = entry.file_name().to_string_lossy().to_string();
                            let source_path = entry.path();
                            let dest_path = format!("/assets/{file_name}");

                            uefi_builder.set_file(dest_path, source_path);
                        }
                    }
                }
            }
        }

        // create a UEFI disk image
        let uefi_path = out_dir.join("uefi.img");
        uefi_builder.create_uefi_image(&uefi_path).unwrap();

        eprintln!("✓ Bootloader images created successfully!");

        // Phase C U7: mint the /data FAT32 image on first build. Lives
        // alongside the boot images so `cargo clean` wipes it (clean
        // slate); otherwise persists across kernel rebuilds so /data
        // contents survive recompiles.
        let data_path = out_dir.join("data.img");
        ensure_data_image(&data_path, 64 * 1024 * 1024);
        println!("cargo:rustc-env=DATA_IMAGE={}", data_path.display());
    } else {
        eprintln!("Kernel does not have the compiled application code");
    }

    // Always set the environment variables
    println!("cargo:rustc-env=BIOS_IMAGE=target/bootloader/bios.img");
    println!("cargo:rustc-env=UEFI_IMAGE=target/bootloader/uefi.img");
}
