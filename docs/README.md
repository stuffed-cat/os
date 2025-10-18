# 文档总览

欢迎来到 `os` 项目的文档中心。本目录对分布在仓库各处的设计说明、开发指南以及运行说明进行了整理，便于快速上手或查找细节。

## 文档结构

- [`architecture.md`](architecture.md)：系统架构、关键模块与混合内核设计要点。
- [`development.md`](development.md)：开发环境准备、常用命令与测试策略。
- [`modules.md`](modules.md)：内核与用户态各模块的职责、关键类型以及交互关系。
- [`elf.md`](elf.md)：ELF64 加载流程、段校验逻辑、TLS 模板与栈权限处理。
- [`filesystem.md`](filesystem.md)：ext2/3/4 解析、overlay 与写入日志设计细节。
- [`qemu.md`](qemu.md)：在本地构建可启动镜像并通过 QEMU 运行的完整步骤。
- [`troubleshooting.md`](troubleshooting.md)：常见问题与解决方案，尤其是裸机构建过程中可能遇到的坑。

## 快速开始

1. 阅读 [`development.md`](development.md) 按照步骤准备依赖并运行测试。
2. 如需了解整体设计或进行二次开发，可参考 [`architecture.md`](architecture.md)。
3. 想体验裸机启动流程，则继续查看 [`qemu.md`](qemu.md)。

欢迎根据需要扩展此目录，维持文档与最新代码的同步。