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

### 重新生成 RAM DISK（ext2 根文件系统）

裸机镜像会自动将 `assets/rootfs.ext2` 作为 `ramdisk` 装载进内核。若需要修改根文件系统内容，可直接编辑 `assets/rootfs/` 目录，然后使用以下命令重新生成 ext2 镜像：

```bash
mke2fs -t ext2 -d assets/rootfs -b 1024 -m 0 assets/rootfs.ext2 1024
```

> 提示：上述命令依赖 `mke2fs`（e2fsprogs）工具。镜像大小为 1 MiB；如需更多空间，可调整最后的块数量参数。

#### Bare shell 内置命令

重新生成 `rootfs.ext2` 后，`/bin` 目录会生成一组 `#!bare-script` 脚本，它们是对裸机 shell 内置命令的独立封装（每个脚本的第一行必须是 `#!bare-script`，后续行可调用 `builtin <name>` 来触发内置实现）。这让命令逻辑存储在镜像中，未来接入真正的 `exec` 时只需替换对应脚本或程序即可。当前版本的 shell 在内核态解释以下命令：

| 命令 | 功能 | 备注 |
|------|------|------|
| `help` | 显示帮助 | |
| `history` | 查看历史命令 | |
| `ls [PATH]` | 列出目录内容 | 支持相对/绝对路径，可使用 `-a`/`--all`、`--color[=WHEN]` |
| `pwd` | 显示当前工作目录 | |
| `cd [PATH]` | 切换工作目录 | 多参数报错 |
| `cat FILE...` | 打印文件内容 | 仅支持 ext2 中的常规文件 |
| `echo ARGS...` | 原样回显参数 | |
| `touch`/`mkdir`/`rmdir`/`rm`/`cp`/`mv` | 预留命令 | 当前根文件系统为只读，这些命令会提示只读限制 |
| `reboot` | 请求重启 | 在 `hardware` 构建目标上调用 ACPI/键盘复位，其他配置打印提示 |
| `shutdown` | 请求关机 | 同上 |

> 注意：脚本解释器位于 `userland/src/bare_shell.rs`，会读取 `/bin/<command>` 的脚本并解析其中的 `builtin` 调用。彩色输出基于 ANSI 转义序列，如需关闭可执行 `ls --color=never`。

### 构建裸机镜像

使用工作区内置的 `xtask` 可生成 BIOS 镜像（调用 vendored `bootloader`）。由于 bootloader 依赖 nightly toolchain，请在执行命令时显式指定：

```bash
RUSTUP_TOOLCHAIN=nightly cargo xtask bootimage
```

生成的镜像位于 `target/x86_64-unknown-none/debug/bootimage-bios.img`，可配合 `qemu-system-x86_64` 启动验证。

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