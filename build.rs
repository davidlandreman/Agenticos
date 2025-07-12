use std::path::PathBuf;

fn main() {
    // Tell cargo to re-run this script if the kernel binary changes
    println!("cargo:rerun-if-changed=target/x86_64-unknown-none/debug/agenticos");
    
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
        
        // create a BIOS disk image
        let bios_path = out_dir.join("bios.img");
        bootloader::BiosBoot::new(&kernel).create_disk_image(&bios_path).unwrap();
        
        // Also create a UEFI disk image
        let uefi_path = out_dir.join("uefi.img");
        bootloader::UefiBoot::new(&kernel).create_disk_image(&uefi_path).unwrap();
        
        println!("cargo:warning=✓ Bootloader images created successfully!");
    } else {
        println!("cargo:warning=Kernel does not have the compiled application code");
    }

    // Always set the environment variables
    println!("cargo:rustc-env=BIOS_IMAGE=target/bootloader/bios.img");
    println!("cargo:rustc-env=UEFI_IMAGE=target/bootloader/uefi.img");
}