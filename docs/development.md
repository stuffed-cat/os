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