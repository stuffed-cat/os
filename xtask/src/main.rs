use std::{
    fs,
    path::{Path, PathBuf},
};

use anyhow::{bail, Context, Result};
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
    /// Build a bootable disk image using the vendored bootloader.
    Bootimage {
        /// Build the release profile instead of debug.
        #[arg(long)]
        release: bool,
        /// Override the target triple used for the kernel binary.
        #[arg(long)]
        target: Option<String>,
        /// Output path for the generated disk image.
        #[arg(long)]
        output: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
        Command::Bootimage {
            release,
            target,
            output,
        } => build_bootimage(release, target.as_deref(), output.as_deref())?,
    }
    Ok(())
}

fn build_bootimage(release: bool, target: Option<&str>, output: Option<&Path>) -> Result<()> {
    let target_triple = target.unwrap_or("x86_64-unknown-none");
    let profile = if release { "release" } else { "debug" };

    let metadata = MetadataCommand::new()
        .exec()
        .context("failed to load cargo metadata")?;
    let target_dir = PathBuf::from(metadata.target_directory);
    let kernel_path = target_dir
        .join(target_triple)
        .join(profile)
        .join("kernel-bin");

    if !kernel_path.exists() {
        bail!(
            "kernel binary not found at {} (build it with `cargo +nightly build -p kernel-bin --features boot -Z build-std=core,alloc,compiler_builtins --target {target_triple}`)",
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
    let config = BootConfig::default();
    builder.set_boot_config(&config);
    builder
        .create_bios_image(&out_path)
        .with_context(|| format!("failed to create BIOS image at {}", out_path.display()))?;

    println!("boot image written to {}", out_path.display());
    Ok(())
}
