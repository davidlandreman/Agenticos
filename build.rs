use std::path::{Path, PathBuf};
use std::process::Command;

/// Mint a blank 64 MiB ext2 image at `path` if it doesn't already
/// exist. Subsequent `./build.sh` runs reuse the on-disk file so /data
/// state survives reboots; passing `--clean` to build.sh removes the
/// target dir (and with it data-ext2.img) for a fresh start.
fn ensure_data_image(path: &Path, size: u64) {
    if path.exists() {
        validate_ext2_image(path);
        return;
    }
    use std::fs::OpenOptions;
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)
        .expect("create data-ext2.img");
    file.set_len(size).expect("set_len data-ext2.img");
    drop(file);
    let mke2fs = find_e2fs_tool("AGENTICOS_MKE2FS", "mke2fs");
    let status = Command::new(&mke2fs)
        .args([
            "-q",
            "-t",
            "ext2",
            "-F",
            "-b",
            "4096",
            "-I",
            "256",
            "-L",
            "AGENTIC-DATA",
            "-O",
            "none,filetype,sparse_super,large_file",
            "-E",
            "lazy_itable_init=0",
        ])
        .arg(path)
        .status()
        .unwrap_or_else(|error| panic!("failed to run {}: {error}", mke2fs.display()));
    if !status.success() {
        let _ = std::fs::remove_file(path);
        panic!(
            "{} failed while formatting {}",
            mke2fs.display(),
            path.display()
        );
    }
    validate_ext2_image(path);
    eprintln!(
        "Minted blank {} MiB ext2 at {}",
        size / 1024 / 1024,
        path.display()
    );
}

fn find_e2fs_tool(env_name: &str, tool: &str) -> PathBuf {
    if let Some(path) = std::env::var_os(env_name).map(PathBuf::from) {
        if path.is_file() {
            return path;
        }
        panic!("{env_name} is not an executable file: {}", path.display());
    }
    let mut candidates = vec![PathBuf::from(tool)];
    for prefix in ["/opt/homebrew/opt/e2fsprogs", "/usr/local/opt/e2fsprogs"] {
        candidates.push(PathBuf::from(prefix).join("sbin").join(tool));
        candidates.push(PathBuf::from(prefix).join("bin").join(tool));
    }
    for candidate in candidates {
        if candidate.components().count() == 1 {
            if Command::new(&candidate).arg("-V").output().is_ok() {
                return candidate;
            }
        } else if candidate.is_file() {
            return candidate;
        }
    }
    panic!(
        "{tool} is required to create AgenticOS's ext2 data image. Install e2fsprogs (macOS: brew install e2fsprogs) or set {env_name}."
    );
}

fn validate_ext2_image(path: &Path) {
    use std::io::{Read, Seek, SeekFrom};
    let mut file = std::fs::File::open(path).expect("open ext2 data image for validation");
    let mut superblock = [0u8; 1024];
    file.seek(SeekFrom::Start(1024))
        .expect("seek ext2 superblock");
    file.read_exact(&mut superblock)
        .expect("read ext2 superblock");
    let le32 =
        |offset: usize| u32::from_le_bytes(superblock[offset..offset + 4].try_into().unwrap());
    assert_eq!(
        &superblock[56..58],
        &[0x53, 0xef],
        "{} is not ext2",
        path.display()
    );
    let compat = le32(92);
    let incompat = le32(96);
    let ro_compat = le32(100);
    assert_eq!(compat, 0, "unsupported ext2 compat mask {compat:#x}");
    assert_eq!(
        incompat, 0x2,
        "unsupported ext2 incompat mask {incompat:#x}"
    );
    assert_eq!(
        ro_compat, 0x3,
        "unsupported ext2 ro-compat mask {ro_compat:#x}"
    );
}

fn main() {
    println!("cargo:rerun-if-env-changed=AGENTICOS_MKE2FS");
    for name in [
        "AGENTICOS_GIT_SHA",
        "AGENTICOS_GIT_DIRTY",
        "AGENTICOS_RUSTC_VERSION",
        "AGENTICOS_DIAGNOSTICS",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let git_sha = std::env::var("AGENTICOS_GIT_SHA").unwrap_or_else(|_| "unknown".into());
    let git_dirty = std::env::var("AGENTICOS_GIT_DIRTY").unwrap_or_else(|_| "unknown".into());
    let rustc = std::env::var("AGENTICOS_RUSTC_VERSION").unwrap_or_else(|_| "unknown".into());
    let diagnostics = std::env::var("AGENTICOS_DIAGNOSTICS").unwrap_or_else(|_| "minimal".into());
    println!("cargo:rustc-env=AGENTICOS_BUILD_GIT_SHA={git_sha}");
    println!("cargo:rustc-env=AGENTICOS_BUILD_GIT_DIRTY={git_dirty}");
    println!("cargo:rustc-env=AGENTICOS_BUILD_RUSTC={rustc}");
    println!("cargo:rustc-env=AGENTICOS_BUILD_DIAGNOSTICS={diagnostics}");
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

        // Mint the /data ext2 image on first build. Lives
        // alongside the boot images so `cargo clean` wipes it (clean
        // slate); otherwise persists across kernel rebuilds so /data
        // contents survive recompiles.
        let data_path = out_dir.join("data-ext2.img");
        ensure_data_image(&data_path, 64 * 1024 * 1024);
        println!("cargo:rustc-env=DATA_IMAGE={}", data_path.display());
    } else {
        eprintln!("Kernel does not have the compiled application code");
    }

    // Always set the environment variables
    println!("cargo:rustc-env=BIOS_IMAGE=target/bootloader/bios.img");
    println!("cargo:rustc-env=UEFI_IMAGE=target/bootloader/uefi.img");
}
