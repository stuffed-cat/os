use std::{
    fs,
    path::{Path, PathBuf},
    process::Command as ProcessCommand,
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
}

fn main() -> Result<()> {
    let args = Args::parse();
    match args.command {
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
    }
    Ok(())
}

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

    build_kernel_binary(release, target_triple, &feature_list)?;

    let metadata = MetadataCommand::new()
        .exec()
        .context("failed to load cargo metadata")?;
    let target_dir = PathBuf::from(metadata.target_directory);
    let workspace_root = PathBuf::from(metadata.workspace_root);
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
    let ramdisk_path = workspace_root.join("assets").join("rootfs.ext2");
    if !ramdisk_path.exists() {
        bail!(
            "ramdisk image not found at {}. Run `mke2fs -t ext2 -d assets/rootfs -b 1024 -m 0 assets/rootfs.ext2 1024` to regenerate.",
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
