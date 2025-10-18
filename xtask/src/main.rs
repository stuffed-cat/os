use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
};

use anyhow::{bail, Context, Result};
#[cfg(feature = "bootimage")]
use bootloader::{BootConfig, DiskImageBuilder};
use cargo_metadata::MetadataCommand;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(author, version, about = "Developer utilities for the hybrid OS", long_about = None)]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Build a bootable disk image using the vendored bootloader（需启用 `bootimage` feature）。
    Bootimage {
        /// Package to target (defaults to kernel-bin).
        #[arg(short = 'p', long = "package")]
        package: Option<String>,
        /// Override the manifest path (ignored, supported for compatibility).
        #[arg(long = "manifest-path")]
        manifest_path: Option<PathBuf>,
        /// Build the release profile instead of debug.
        #[arg(long)]
        release: bool,
        /// Override the target triple used for the kernel binary.
        #[arg(long)]
        target: Option<String>,
        /// Output path for the generated disk image.
        #[arg(long)]
        output: Option<PathBuf>,
        /// Feature list passed through to the kernel build.
        #[arg(long = "features", value_delimiter = ',')]
        features: Option<Vec<String>>,
        /// Suppress informational output.
        #[arg(long)]
        quiet: bool,
        /// Collect any extra arguments for forward compatibility.
        #[arg(last = true)]
        extra: Vec<String>,
    },
    /// Regenerate command binaries and rebuild the root filesystem image.
    Rootfs,
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        #[cfg(feature = "bootimage")]
        Command::Bootimage {
            package,
            manifest_path,
            release,
            target,
            output,
            features,
            quiet: _quiet,
            extra,
        } => build_bootimage(
            package.as_deref(),
            manifest_path.as_deref(),
            release,
            target.as_deref(),
            output.as_deref(),
            features,
            &extra,
        )?,
        #[cfg(not(feature = "bootimage"))]
        Command::Bootimage { .. } => {
            bail!(
                "bootimage support is disabled. Re-run with `cargo run -p xtask --features bootimage -- bootimage ...`"
            );
        }
        Command::Rootfs => rebuild_rootfs()?,
    }
    Ok(())
}

#[cfg(feature = "bootimage")]
fn build_bootimage(
    package: Option<&str>,
    _manifest_path: Option<&Path>,
    release: bool,
    target: Option<&str>,
    output: Option<&Path>,
    features: Option<Vec<String>>,
    extra_args: &[String],
) -> Result<()> {
    if let Some(additional) = extra_args.first() {
        bail!("unrecognized argument `{additional}`");
    }

    if let Some(pkg) = package {
        if pkg != "kernel-bin" {
            bail!("unsupported package `{pkg}`; expected `kernel-bin`");
        }
    }

    let feature_list: Vec<String> = match features {
        Some(list) if !list.is_empty() => list,
        _ => vec!["boot".to_string()],
    };

    let target_triple = target.unwrap_or("x86_64-unknown-none");
    let profile = if release { "release" } else { "debug" };

    let metadata = MetadataCommand::new()
        .exec()
        .context("failed to load cargo metadata")?;
    let target_dir = PathBuf::from(metadata.target_directory);
    let workspace_root = PathBuf::from(metadata.workspace_root);

    generate_command_binaries(&workspace_root)?;
    regenerate_rootfs_image(&workspace_root)?;

    build_kernel_binary(release, target_triple, &feature_list)?;
    let kernel_path = target_dir
        .join(target_triple)
        .join(profile)
        .join("kernel-bin");

    if !kernel_path.exists() {
        bail!(
            "kernel binary not found at {} even after build",
            kernel_path.display()
        );
    }

    let out_path = output.map(Path::to_path_buf).unwrap_or_else(|| {
        target_dir
            .join(target_triple)
            .join(profile)
            .join("bootimage-bios.img")
    });
    if let Some(parent) = out_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }

    let mut builder = DiskImageBuilder::new(kernel_path.clone());
    let ramdisk_path = workspace_root.join("assets").join("rootfs.ext4");
    if !ramdisk_path.exists() {
        bail!(
            "ramdisk image not found at {}. Run `mke2fs -t ext4 -O ^has_journal,^metadata_csum,^64bit,^flex_bg -d assets/rootfs -b 1024 -m 0 assets/rootfs.ext4 1024` to regenerate.",
            ramdisk_path.display()
        );
    }
    builder.set_ramdisk(ramdisk_path);
    let config = BootConfig::default();
    builder.set_boot_config(&config);
    builder
        .create_bios_image(&out_path)
        .with_context(|| format!("failed to create BIOS image at {}", out_path.display()))?;

    println!("boot image written to {}", out_path.display());
    Ok(())
}

fn rebuild_rootfs() -> Result<()> {
    let metadata = MetadataCommand::new()
        .exec()
        .context("failed to load cargo metadata")?;
    let workspace_root = PathBuf::from(metadata.workspace_root);
    generate_command_binaries(&workspace_root)?;
    regenerate_rootfs_image(&workspace_root)?;
    Ok(())
}

const BCM_COMMANDS: &[(&str, u8)] = &[
    ("help", 0),
    ("history", 1),
    ("ls", 2),
    ("pwd", 3),
    ("cd", 4),
    ("cat", 5),
    ("echo", 6),
    ("touch", 7),
    ("mkdir", 8),
    ("rmdir", 9),
    ("rm", 10),
    ("cp", 11),
    ("mv", 12),
    ("reboot", 13),
    ("shutdown", 14),
    ("sh", 15),
];

fn generate_command_binaries(workspace_root: &Path) -> Result<()> {
    let bin_dir = workspace_root.join("assets").join("rootfs").join("bin");

    fs::create_dir_all(&bin_dir)
        .with_context(|| format!("failed to ensure {} exists", bin_dir.display()))?;

    for &(name, id) in BCM_COMMANDS {
        let data = build_command_module(id, name)?;

        let path = bin_dir.join(name);
        fs::write(&path, data)
            .with_context(|| format!("failed to write BCM stub for {}", path.display()))?;
    }

    Ok(())
}

fn build_command_module(command_id: u8, command_name: &str) -> Result<Vec<u8>> {
    let name_bytes = command_name.as_bytes();
    if name_bytes.is_empty() {
        bail!("command name must not be empty");
    }

    let mut elf = Vec::new();
    elf.resize(64, 0);

    // e_ident
    elf[0..4].copy_from_slice(&[0x7F, b'E', b'L', b'F']);
    elf[4] = 2; // 64-bit
    elf[5] = 1; // little-endian
    elf[6] = 1; // version
    elf[7] = 0; // System V

    // e_type (relocatable)
    write_u16(&mut elf, 16, 1);
    // e_machine (x86-64)
    write_u16(&mut elf, 18, 0x3E);
    // e_version
    write_u32(&mut elf, 20, 1);
    // e_entry stays 0
    // e_phoff stays 0 because we have no program headers

    // We'll fill e_shoff later once sizes are known.
    // e_flags remains 0.
    write_u16(&mut elf, 52, 64); // e_ehsize
    write_u16(&mut elf, 54, 0); // e_phentsize
    write_u16(&mut elf, 56, 0); // e_phnum
    write_u16(&mut elf, 58, 64); // e_shentsize
    write_u16(&mut elf, 60, 3); // e_shnum (null + .note.bcm + .shstrtab)
    write_u16(&mut elf, 62, 2); // e_shstrndx

    // Build note payload: namesz=0, descsz=12 (magic, version, command id), type=0x4D434221
    let desc = build_note_descriptor(command_id)?;
    let namesz = 0u32;
    let descsz =
        u32::try_from(desc.len()).context("descriptor length exceeds u32::MAX for ELF note")?;

    let mut note = Vec::new();
    note.extend_from_slice(&namesz.to_le_bytes());
    note.extend_from_slice(&descsz.to_le_bytes());
    note.extend_from_slice(&0x4D43_4221u32.to_le_bytes()); // 'BCB!' magic type
                                                           // No name payload because namesz == 0
    note.extend_from_slice(&desc);
    while note.len() % 4 != 0 {
        note.push(0);
    }

    let note_offset = elf.len();
    elf.extend_from_slice(&note);

    let shstrtab_offset = align_up(elf.len(), 4);
    if elf.len() != shstrtab_offset {
        elf.resize(shstrtab_offset, 0);
    }

    let mut shstrtab = Vec::new();
    shstrtab.push(0); // null entry
    let note_name_offset = shstrtab.len();
    shstrtab.extend_from_slice(b".note.bcm\0");
    let shstrtab_name_offset = shstrtab.len();
    shstrtab.extend_from_slice(b".shstrtab\0");

    let shstrtab_len = shstrtab.len();
    elf.extend_from_slice(&shstrtab);

    let section_headers_offset = align_up(elf.len(), 8);
    if elf.len() != section_headers_offset {
        elf.resize(section_headers_offset, 0);
    }

    let shoff = section_headers_offset as u64;
    write_u64(&mut elf, 40, shoff);

    // Reserve space for 3 section headers (null + note + shstrtab)
    elf.resize(section_headers_offset + 3 * 64, 0);

    // Section header 0 is already zeroed.

    // Section header 1: .note.bcm
    write_u32(
        &mut elf,
        section_headers_offset + 64,
        note_name_offset as u32,
    );
    write_u32(&mut elf, section_headers_offset + 64 + 4, 7); // SHT_NOTE
                                                             // sh_flags (offset +8) already zero.
    write_u64(
        &mut elf,
        section_headers_offset + 64 + 24,
        note_offset as u64,
    );
    write_u64(
        &mut elf,
        section_headers_offset + 64 + 32,
        note.len() as u64,
    );
    write_u64(&mut elf, section_headers_offset + 64 + 48, 4); // sh_addralign

    // Section header 2: .shstrtab
    let shstrtab_header_offset = section_headers_offset + 128;
    write_u32(
        &mut elf,
        shstrtab_header_offset,
        shstrtab_name_offset as u32,
    );
    write_u32(&mut elf, shstrtab_header_offset + 4, 3); // SHT_STRTAB
    write_u64(
        &mut elf,
        shstrtab_header_offset + 24,
        shstrtab_offset as u64,
    );
    write_u64(&mut elf, shstrtab_header_offset + 32, shstrtab_len as u64);
    write_u64(&mut elf, shstrtab_header_offset + 48, 1); // sh_addralign

    Ok(elf)
}

fn regenerate_rootfs_image(workspace_root: &Path) -> Result<()> {
    let assets_dir = workspace_root.join("assets");
    let rootfs_dir = assets_dir.join("rootfs");
    let ramdisk_path = assets_dir.join("rootfs.ext4");

    if !rootfs_dir.exists() {
        bail!(
            "root filesystem directory missing: {}",
            rootfs_dir.display()
        );
    }

    let mut cmd = ProcessCommand::new("mke2fs");
    cmd.arg("-t")
        .arg("ext4")
        .arg("-O")
        .arg("^has_journal,^metadata_csum,^64bit,^flex_bg")
        .arg("-d")
        .arg(&rootfs_dir)
        .arg("-b")
        .arg("1024")
        .arg("-m")
        .arg("0")
        .arg("-F")
        .arg(&ramdisk_path)
        .arg("1024");

    let status = cmd
        .status()
        .with_context(|| "failed to spawn mke2fs for rootfs regeneration")?;
    if !status.success() {
        bail!(
            "mke2fs exited with status {} while regenerating {}",
            status,
            ramdisk_path.display()
        );
    }

    if !ramdisk_path.exists() {
        bail!(
            "mke2fs completed but {} was not created",
            ramdisk_path.display()
        );
    }

    Ok(())
}

#[cfg(feature = "bootimage")]
fn build_kernel_binary(release: bool, target_triple: &str, features: &[String]) -> Result<()> {
    let mut cmd = ProcessCommand::new("rustup");
    cmd.arg("run").arg("nightly").arg("cargo");
    cmd.arg("-Zbuild-std=core,alloc,compiler_builtins");
    cmd.arg("-Zbuild-std-features=compiler-builtins-mem");
    cmd.arg("build");
    cmd.arg("-p").arg("kernel-bin");
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }
    cmd.arg("--target").arg(target_triple);
    if release {
        cmd.arg("--release");
    }
    cmd.env("RUSTUP_TOOLCHAIN", "nightly");
    let status = cmd.status().context("failed to invoke cargo build")?;
    if !status.success() {
        bail!("kernel build failed (status {status})");
    }
    Ok(())
}

fn build_note_descriptor(command_id: u8) -> Result<Vec<u8>> {
    let mut descriptor = Vec::with_capacity(12);
    descriptor.extend_from_slice(&0x214D_4342u32.to_le_bytes()); // '!BCM' magic
    descriptor.extend_from_slice(&1u32.to_le_bytes()); // version
    descriptor.extend_from_slice(&(command_id as u32).to_le_bytes());
    Ok(descriptor)
}

fn write_u16(buffer: &mut [u8], offset: usize, value: u16) {
    let bytes = value.to_le_bytes();
    buffer[offset..offset + 2].copy_from_slice(&bytes);
}

fn write_u32(buffer: &mut [u8], offset: usize, value: u32) {
    let bytes = value.to_le_bytes();
    buffer[offset..offset + 4].copy_from_slice(&bytes);
}

fn write_u64(buffer: &mut [u8], offset: usize, value: u64) {
    let bytes = value.to_le_bytes();
    buffer[offset..offset + 8].copy_from_slice(&bytes);
}

fn align_up(value: usize, align: usize) -> usize {
    if align == 0 {
        return value;
    }
    (value + align - 1) & !(align - 1)
}
