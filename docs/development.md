# 开发指南

本文档帮助贡献者或未来维护者快速完成环境配置、常用命令与质量保障步骤。

## 环境准备

1. **基础工具**
   - Rust 1.78+ 稳定版（建议安装 rustup 方便切换 toolchain）。
   - `cargo` 与 `rustfmt` 自带于 rustup。
   - 可选：`just`、`cargo-nextest` 等生产力工具。

2. **仓库初始化**
   ```bash
   git clone https://github.com/stuffed-cat/os.git
   cd os
   rustup override set stable
   ```

3. **常用组件**
   - 日常开发（带 `std`）依赖 stable toolchain。
   - 构建裸机镜像（启用 `kernel-bin` 的 `boot` 特性）需要安装额外 target、可能需要 nightly，详见 [`qemu.md`](qemu.md)。

## 工作流建议

1. **编辑器 / IDE**：推荐 VS Code + Rust Analyzer 或 CLion。
2. **代码格式化**：提交前运行 `cargo fmt`。
3. **静态检查**：执行 `cargo clippy --all-targets --all-features`（如启用 nightly，请添加 `-Z unstable-options`）。
4. **单元与集成测试**：
   ```bash
   cargo test
   ```
   该命令会同时运行 `kernel` crate 的单测、集成测试以及 `userland` 的测试。
5. **编写新模块**：请补充相应的文档（如本目录）并在合适位置添加测试用例。

### 重新生成 RAM DISK（ext4 可写根文件系统）

裸机镜像会自动将 `assets/rootfs.ext4` 作为 ramdisk 装载进内核，内核使用写入 overlay 提供默认可写的视图。若需要修改根文件系统内容，可直接编辑 `assets/rootfs/` 目录，然后使用以下命令重新生成兼容的 ext4 镜像：

```bash
mke2fs -t ext4 -O ^has_journal,^metadata_csum -d assets/rootfs -b 1024 -m 0 assets/rootfs.ext4 16384
```

> 提示：上述命令依赖 `mke2fs`（e2fsprogs）工具。镜像大小为 16 MiB；如需更多空间，可调整最后的块数量参数。`-O` 参数用于禁用当前内核尚未实现的 ext4 特性（journal、metadata checksum）。从现在起，镜像可启用 ext4 的 `64bit` 与 `flex_bg` 扩展，无需额外参数即可兼容。

#### 用户态 Shell 与命令集合

当前内核在 `ShellSubsystem::init` 中直接 `exec` `/bin/sh`，由用户态 shell 负责解析与调度命令。`xtask bootimage` 会自动：

1. 使用 `cargo build -p userland --release --bin sh` 等目标产出一组 ELF（二进制位于 `target/release/`）。
2. 将它们复制到 `assets/rootfs/bin/`，包括 `sh`、`ls`、`cat`、`cp` 等工具。
3. 重新打包 `assets/rootfs.ext4` 并写入镜像。

因此 `/bin` 中的命令已经是真正的用户态程序，它们通过 `userland::syscall` 模块编码 POSIX 请求，与内核的 `PosixLayer` 交互。若要添加新的命令，可在 `xtask/src/main.rs` 中：

1. 将命令名加入 `commands` 数组，确保编译器会构建对应的 `userland` 二进制。
2. 如需兼容旧版 BCM 框架，可在 `BCM_COMMANDS` 常量里注册占位符。
3. 运行 `cargo run -p xtask --features bootimage -- rootfs`（或 `bootimage` 子命令）重新构建根文件系统。

也可以手动把编译好的 ELF 拷贝到 `assets/rootfs/bin/` 后再次运行 `bootimage`，工具会自动重新生成 ext4 镜像。

用户态 shell 依赖一组环境变量（`HOME`、`PWD`、`USER`、`SHELL` 等）由内核在 `exec` 前设置，可根据需要在 `Process::set_env` 调用处添加更多默认配置。

### 构建裸机镜像

使用工作区内置的 `xtask` 可生成 BIOS 镜像（调用 vendored `bootloader`）。由于 bootloader 依赖 nightly toolchain，需要先安装 nightly（`rustup toolchain install nightly`）。生成镜像时请显式启用 `bootimage` 特性：

```bash
cargo run -p xtask --features bootimage -- bootimage
```

生成的镜像位于 `target/x86_64-unknown-none/debug/bootimage-bios.img`，可配合 `qemu-system-x86_64` 启动验证。

若直接执行 `cargo xtask bootimage`（未启用特性），工具会给出友好的提示信息并指导如何重新运行。

若仅需重新生成根文件系统中的裸机命令与 `assets/rootfs.ext4`，运行 `cargo xtask rootfs` 即可，无需启用额外特性。

## 代码布局速查

```
├── kernel/         # 核心内核库
│   ├── src/
│   │   ├── arch/   # x86-64 架构专属代码
│   │   ├── hal.rs  # 硬件抽象层
│   │   ├── memory.rs
│   │   ├── ...
│   │   └── tests.rs
│   └── Cargo.toml
├── kernel-bin/     # 裸机入口，与 bootloader 集成
├── userland/       # 用户态原型
├── docs/           # 本目录
└── README.md       # 项目概览
```

## Git 指南

- 遵循“单一职责”提交，附带清晰 commit message。
- 若引入新依赖，请补充在 `Cargo.toml` 与文档中说明用途。
- 变更公共 API 时，请在 `CHANGELOG`（若未来添加）或文档中注明破坏性变更。

## 持续改进

- 若文档与代码不一致，请优先修改文档。
- 建议为新功能编写集成测试，确保混合内核各子系统协同正常。
- 欢迎在 Issues / Discussions 中维护 roadmap 或提出改进建议。

保持代码和文档的同步，有助于让项目更易维护与扩展。