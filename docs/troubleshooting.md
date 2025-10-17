# 常见问题排查

本文收集在构建与运行过程中遇到的高频问题，便于快速定位与解决。如遇到新的问题，欢迎补充。

## 构建相关

### `serde_core` 缺少 `Result` 或 `Option`

- **现象**：启用 `kernel-bin` 的 `boot` 特性后，编译阶段出现大量 `cannot find type Result`、`cannot find tuple variant Ok` 等报错。
- **原因**：未使用 nightly toolchain 或缺少 `llvm-tools-preview` 组件，导致 `bootimage` 无法正确链接 `core`/`alloc`。
- **解决步骤**：
  ```bash
  rustup component add llvm-tools-preview --toolchain nightly
  rustup target add x86_64-unknown-none --toolchain nightly
  cargo +nightly bootimage -p kernel-bin --features boot
  ```

### `bootimage` 命令找不到

- `cargo bootimage` 依赖额外的工具包，确保执行：
  ```bash
  cargo +nightly install bootimage
  ```
  安装完成后重新打开终端，或确认 `~/.cargo/bin` 已加入 `PATH`。

### `linker cc not found`

- 运行 `bootimage` 时出现该错误，通常是系统缺少基础编译工具链。
- 在 Debian/Ubuntu 上可安装：
  ```bash
  sudo apt-get install build-essential
  ```

## 运行相关

### QEMU 无输出

- 启动命令中确保包含 `-serial stdio`，内核日志通过串口输出。
- 若仍无输出，可尝试移除 `-display none` 检查图形界面是否有报错。

### 内核立即停止或循环

- 目前内核在引导完成后进入 `hlt` 循环等待中断，属于早期阶段预期行为。

## 文档同步

如果遇到文档未覆盖的问题，请在更新代码的同时补充此文件，确保后续贡献者能够快速定位相同问题。