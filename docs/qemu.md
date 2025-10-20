# 在 QEMU 中运行内核

本指南介绍如何从源码构建裸机镜像，并通过 QEMU 启动 `kernel`。请注意，该流程仍处于实验阶段，部分步骤依赖 nightly toolchain 和 `bootloader` 提供的工具链支持。

## 前置条件

1. **Rust toolchain**
   ```bash
   rustup toolchain install nightly
   rustup component add llvm-tools-preview --toolchain nightly
   rustup target add x86_64-unknown-none --toolchain nightly
   ```

2. **（可选）bootimage 工具**：若希望直接使用 `cargo bootimage`，可单独安装。
   ```bash
   cargo +nightly install bootimage
   ```

3. **QEMU**（以 Ubuntu/Debian 为例）
   ```bash
   sudo apt-get install qemu-system-x86
   ```

## 构建步骤

1. **切换到项目根目录**
   ```bash
   cd /path/to/os
   ```

2. **编译并生成镜像**
   ```bash
   cargo run -p xtask --features bootimage -- bootimage
   ```

   - `xtask` 会调用仓库中 vendored 的 `bootloader` 生成 BIOS 镜像，输出路径为 `target/x86_64-unknown-none/debug/bootimage-bios.img`。
   - 若出现 “failed to get llvm tools” 或 “the option `Z` is only accepted on the nightly compiler” 等报错，请确认已按前置条件安装 nightly toolchain 及 `llvm-tools-preview` 组件。
   - 如需使用原生 `cargo bootimage` 工作流，可参考 `bootloader` 官方文档；此处推荐的 `xtask` 已封装正确的参数与路径。

## 使用 QEMU 启动

```bash
qemu-system-x86_64 \
   -drive format=raw,file=target/x86_64-unknown-none/debug/bootimage-bios.img \
  -serial stdio
```

- `-serial stdio` 将串口输出重定向到当前终端，便于查看 `kernel` 打印的日志。
- `-display none` 关闭图形输出，集中在文本模式；如需图形界面可移除此参数。

若一切正常，QEMU 终端会显示引导日志与串口输出。当前内核仍处于早期阶段。

## 调试技巧

- **开启 GDB 远程调试**：在 QEMU 命令中加入 `-s -S`，QEMU 会在启动时挂起并监听 1234 端口，可使用 `gdb`/`lldb` 连接。
- **查看引导日志**：`kernel` 使用串口输出日志，确保 `-serial stdio` 或指向文件以保存信息。
- **构建 release 版本**：
   ```bash
   cargo run -p xtask --features bootimage -- bootimage --release
   ```
- **磁盘镜像路径**：若修改 target 目录或使用自定义目标，可在 `cargo run -p xtask --features bootimage -- bootimage --help` 查看更多参数。

## 常见问题

| 问题 | 可能原因 | 解决方案 |
|------|----------|----------|
| `serde_core` 缺少 `Result`/`Option` 等符号 | 未使用 nightly 或缺失 `llvm-tools-preview` | 重新执行前置安装命令，确保 `cargo +nightly` 在使用 |
| `bootimage` 命令不存在 | 未安装或 PATH 未更新 | 重新运行 `cargo +nightly install bootimage` 并重新打开终端 |
| QEMU 启动后黑屏无输出 | 未启用串口 / 内核未打印 | 加上 `-serial stdio` 参数，或检查 `kernel` 日志初始化 |
| `failed to create FAT filesystem` 报错 | 旧版镜像生成流程受 FAT16 容量限制 | 当前版本已自动切换 FAT32；若仍失败，请确认磁盘镜像未超过 2 GiB |

目前 `kernel` 裸机模式功能有限，将在 roadmap 中持续扩展。欢迎在体验过程中记录问题并更新 [`docs/troubleshooting.md`](troubleshooting.md)。