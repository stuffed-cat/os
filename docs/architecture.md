# 系统架构与设计理念

本文档从宏观视角介绍 `NexaOS` 项目当前实现的混合内核架构，涵盖核心 crate、子系统职责划分以及 POSIX 兼容层的行为约定。

## 总览

仓库采用 Rust 2021 版 Cargo workspace，主要包含三个 crate：

| crate        | 角色说明 |
|--------------|-----------|
| `kernel`     | 混合内核主体，提供硬件抽象层、进程/调度/IPC 等核心子系统以及 POSIX 兼容接口。 |
| `kernel-bin` | 基于 `bootloader` crate 的裸机入口，负责在真实或虚拟硬件上初始化 `kernel`。 |
| `userland`   | 产出 `/bin/sh` 及一组基础命令的 ELF，可直接在用户态通过 POSIX syscall 运行。 |

整体控制流：

1. `kernel-bin` 在引导阶段收集启动信息（内存 map、页表偏移等），构造 `HalConfig` 并引导 `kernel::Hal`。
2. `KernelBuilder` 注册所有子系统，进入 `KernelState::Bootstrap` 完成初始化。
3. `shell::ShellSubsystem` 在内核态完成串口输入缓冲，并在 `init` 阶段 `exec` `/bin/sh`，由调度器接管该用户进程的运行。
4. 进入 `KernelState::Running` 后，调度器与 IPC 子系统协作提供基础 OS 服务，用户态通过 POSIX 层暴露的 syscall ABI 与内核交互。

```
+------------------+      syscalls      +--------------------------------+
|     userland     | <----------------> |    kernel (POSIX shim + HAL)   |
| (POSIX 程序原型) |                    | sched / process / memory / IPC |
+------------------+                    +--------------------------------+
```

## 子系统与模块

`kernel/src` 目录按职责拆分：

- `core`: `Kernel`, `KernelBuilder` 及 `Subsystem` trait；负责生命周期管理与依赖注入。
- `arch`: x86-64 专有逻辑，包含 GDT/IDT、PIC 中断控制器、串口日志等。
- `hal`: 在架构层与内核服务之间建立抽象，负责内存映射、启停中断等。
- `memory`: 引导期帧分配器 (`BootFrameAllocator`) 与基于 `OffsetPageTable` 的分页管理。
- `process`: 进程控制块、父子关系、文件描述符表等。
- `scheduler`: 简单双队列调度器，区分内核线程与用户线程优先级，支持在用户线程上下文中切换 CR3。
- `ipc`: 邮箱式消息传递设施，是迈向微内核化隔离的基础。
- `posix`: Syscall ID 映射、错误码 (`errno`) 以及路径/程序句柄注册，并桥接串口输入为 `tty:stdin` 的阻塞读取。
- `shell`: 负责键盘扫描码译码、串口回显、内核态输入缓冲，并在初始化时创建 `/bin/sh` 用户进程。
- `services`: 轻量级服务注册中心，支持运行时按类型查找子系统实例。

### 混合内核要点

1. **微内核式隔离**：IPC、内存能力等组件通过 `Capability` 和 `VmRegion` 结构描述访问权限，结合 `process::rebuild_user_page_tables`，fork 后每个子进程都会重建独立页表。
2. **单体内核快速路径**：调度器、内存映射等热点仍共享内核态数据结构，保证性能和实现复杂度在早期阶段可控。
3. **POSIX 兼容 + 动态装载**：`process::exec` 支持解析 ELF `PT_INTERP`，在内核态先行加载解释器镜像并构造合并地址空间，随后由用户态动态链接器接管控制权。
4. **用户态 Shell**：`shell` 子系统只负责输入缓冲与首进程启动，内核不再自带命令解释器，所有会话逻辑在用户态 `/bin/sh` 中实现。

## 引导与硬件抽象

`ArchBootstrap` 完成：

- 串口初始化（`serial::init`）用于早期日志。
- GDT/TSS 配置，确保双重故障等异常有专用栈。
- IDT/8259 PIC 配置：目前实现了简化版 PIC 初始化与 EOI 逻辑，满足 PIT/键盘等基础中断需求。

`Hal::bootstrap`：

1. 根据 `HalConfig` 中的 `frame_ranges` 构建 `BootFrameAllocator`。
2. 使用 `OffsetPageTable` 将内核堆 (`HEAP_START` 起的虚拟地址范围) 映射到物理帧。
3. 公开 `enable_interrupts/disable_interrupts` 等接口给上层。

## 测试与未来规划（技术路线）

- `kernel/tests/integration.rs` 已覆盖 IPC 往返、调度器轮转、POSIX syscall 流程；`posix_fork_exec_open_read_flow` 用例会验证解释器加载路径。
- `cargo run -p xtask --features bootimage -- bootimage` 会同时编译用户态 ELF、重打包根文件系统，并验证内核在 `hardware` 配置下可通过 bootloader 构建。

**技术路线更新**：

1. **完善 I/O 多路复用**：在当前 `tty:stdin` 基础上扩展 `poll`/`select`，并为管道、伪终端暴露统一等待接口。
2. **进程生命周期管理**：实现 `waitpid`、信号传递与作业控制，使用户态 shell 能正确回收子进程。
3. **文件系统增强**：引入写入日志回放、目录缓存与 `stat` 系列 syscall，减轻 ext4 访问压力。
4. **内存安全边界**：针对用户态参数添加更细粒度的边界检查，并实现 mmap/munmap 以支撑动态链接器的内存需求。
5. **硬件适配**：迁移至 APIC/HPET 计时源并封装中断屏蔽，为多核调度做准备。

本页仅概览关键结构及最新演进，建议结合源码与注释进一步阅读。