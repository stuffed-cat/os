fn main() {
    // Get manifest dir to find source files
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let out_dir = std::env::var("OUT_DIR").unwrap();
    
    // Assemble boot.s
    let boot_src = std::path::PathBuf::from(&manifest_dir).join("src/boot.s");
    let boot_obj = std::path::PathBuf::from(&out_dir).join("boot.o");
    
    let status = std::process::Command::new("as")
        .args(&["--64"])
        .args(&["-o", boot_obj.to_str().unwrap()])
        .arg(boot_src.to_str().unwrap())
        .status()
        .expect("Failed to assemble boot.s");
    
    if !status.success() {
        panic!("Assembler failed");
    }
    
    println!("cargo::rustc-link-search=native={}", out_dir);
    
    // Link boot.o BEFORE other object files
    println!("cargo::rustc-link-arg={}", boot_obj.display());
    
    println!("cargo::rerun-if-changed=src/boot.s");
    println!("cargo::rerun-if-changed=multiboot2.ld");
}

