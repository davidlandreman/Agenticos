use std::path::PathBuf;

fn main() {
    // Tell cargo to re-run this script if the kernel binary changes
    println!("cargo:rerun-if-changed=target/x86_64-unknown-none/debug/agenticos");
    // Also re-run if assets directory changes
    println!("cargo:rerun-if-changed=assets");
    
    // Use relative paths in the target directory
    let target_dir = PathBuf::from("/Users/david/Projects/agenticos/target");
    let out_dir = target_dir.join("./bootloader");
    
    // Create output directory if it doesn't exist
    std::fs::create_dir_all(&out_dir).ok();
    
    // Path to the kernel binary
    let kernel = target_dir.join("x86_64-unknown-none/debug/agenticos");
    
    // Check if we're in the second pass (kernel exists)
    println!("cargo:warning=Checking for Kernel Code: {}", kernel.display());
    if kernel.exists() {
        println!("cargo:warning=Creating bootloader images...");
        
        // Create disk image builder with the kernel
        let mut builder = bootloader::DiskImageBuilder::new(kernel.clone());
        
        // Add assets folder to the disk image
        let assets_dir = PathBuf::from("/Users/david/Projects/agenticos/assets");
        if assets_dir.exists() {
            println!("cargo:warning=Adding assets to disk image...");
            
            // Read the assets directory and add each file
            if let Ok(entries) = std::fs::read_dir(&assets_dir) {
                for entry in entries.flatten() {
                    if let Ok(metadata) = entry.metadata() {
                        if metadata.is_file() {
                            let file_name = entry.file_name().to_string_lossy().to_string();
                            let source_path = entry.path();
                            let dest_path = format!("/assets/{}", file_name);
                            
                            println!("cargo:warning=  Adding {}", dest_path);
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
                            let dest_path = format!("/assets/{}", file_name);
                            
                            uefi_builder.set_file(dest_path, source_path);
                        }
                    }
                }
            }
        }
        
        // create a UEFI disk image
        let uefi_path = out_dir.join("uefi.img");
        uefi_builder.create_uefi_image(&uefi_path).unwrap();
        
        println!("cargo:warning=✓ Bootloader images created successfully!");
    } else {
        println!("cargo:warning=Kernel does not have the compiled application code");
    }

    // Always set the environment variables
    println!("cargo:rustc-env=BIOS_IMAGE=target/bootloader/bios.img");
    println!("cargo:rustc-env=UEFI_IMAGE=target/bootloader/uefi.img");
}