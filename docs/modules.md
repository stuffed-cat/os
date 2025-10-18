# 模块全览

本文件按照仓库内的主要 Rust crate 与模块结构进行讲解，帮助快速定位核心逻辑并理解它们之间的协作关系。内容覆盖 `kernel`、`userland`、`libc-lite` 以及辅助工具 `xtask`，同时列出关键类型、重要函数与协作流程。

## Kernel crate

`kernel` 提供混合内核的主体实现，模块之间兼顾微内核式的能力管控与单体式的快速路径。

### `core`
- **职责**：定义 `Subsystem` trait、`KernelBuilder`、`KernelState` 等核心框架，负责初始化顺序与生命周期管理。
- **关键类型**：`KernelContext`（向子系统暴露受限环境）、`SubsystemId`、`SubsystemHandle`。
- **交互**：启动阶段逐个调用子系统的 `init`，运行期在 `tick` 循环中驱动它们协同工作。

### `arch`
- **职责**：x86-64 架构相关的启动流程、GDT/IDT/中断控制器初始化。
- **关键类型**：`ArchBootstrap`、`InterruptDescriptorTable` 包装、`Pic8259Controller`。
- **交互**：为 `hal`、`scheduler` 等模块提供硬件级钩子，如中断屏蔽、APIC/HPET 定时源。

### `hal`
- **职责**：硬件抽象层，将底层寄存器操作封装为易于测试与移植的接口。
- **关键类型**：`HardwareAbstractionLayer`、`InterruptController`、`Timer`。
- **交互**：向 `scheduler` 发布时钟 tick，供 `memory` 管理页表与帧分配，向 `services` 提供统一硬件访问入口。

### `memory`
- **职责**：分页表构建、帧分配器、虚拟内存能力（`Capability`）管理。
- **关键类型**：`MemoryManager`、`PageTableHandle`、`CapabilitySpace`。
- **交互**：`process` 在加载 ELF 时通过 `MemoryManager` 申请映射；`scheduler` 通过能力机制切换页表；`ipc` 可基于能力实现共享内存。

### `process`
- **职责**：进程表（`ProcessTable`）、线程状态、文件描述符表、`exec`/`fork` 生命周期管理。
- **关键类型**：`Process`、`ThreadState`、`UserContext`。
- **交互**：与 `scheduler` 协商线程调度，与 `fs` 进行文件读写，与 `elf` 协作加载可执行文件，并经 `syscall` 接口暴露给用户态。

### `scheduler`
- **职责**：基于时间片的抢占式调度器，支持内核/用户线程优先级队列与定时器中断。
- **关键类型**：`Scheduler`、`RunQueueEntry`、`ThreadStatus`。
- **交互**：通过 `hal` 获取时钟中断，调度 `process` 提供的线程，向 `ipc` 通知线程阻塞/唤醒事件。

### `ipc`
- **职责**：能力化的消息邮箱，支持跨进程通道绑定与访问控制。
- **关键类型**：`Mailbox`、`Channel`, `Message`。
- **交互**：`services` 可注册系统服务并通过邮箱暴露接口；`syscall` 将 POSIX 式 IPC 调用映射到邮箱传输。

### `services`
- **职责**：运行时服务注册与发现，提供以类型为索引的服务容器。
- **关键类型**：`ServiceRegistry`、`ServiceHandle`。
- **交互**：各子系统启动时向 registry 注册；`process` 和 `ipc` 通过 service 查找内核功能。

### `fs`
- **职责**：POSIX 风格的文件系统接口与 ramfs/overlay 实现，含权限/UID/GID 检查。
- **关键类型**：`DirEntry`、`FsError`、`Credentials`。
- **交互**：供 `process` 处理 `open/read/write`，`shell` 访问文件，`syscall` 将用户态请求路由进来。

### `posix`
- **职责**：POSIX 兼容层，把 Linux syscall 编号/errno 对应到内核内部实现。
- **关键类型**：`PosixLayer`、`Errno`、路径规范化函数。
- **交互**：`syscall` 调用 `PosixLayer::dispatch` 处理 `read/open/fork/exec` 等操作。

### `syscall`
- **职责**：系统调用调度，按 x86-64 SysV ABI 从 `TrapFrame` 解析参数并转发给 POSIX 层。
- **关键类型**：`SyscallDispatcher`、`SyscallId`。
- **交互**：被 `user` 提供的陷入机制触发，返回值写回 `TrapFrame`，并在错误时转换为 `KernelError`。

### `elf`
- **职责**：64 位 ELF 加载器（详见本文稍后章节），产出 `ExecutableImage` 和段信息供 `process`/`user` 使用。
- **关键类型**：`ExecutableImage`（包含 `segments`、`stack_flags`、`tls_template`）、`ExecutableSegment`（记录 `mem_size`/`file_size`/`align`）、`TlsTemplate`。
- **校验要点**：确认魔数/架构/版本与 OS ABI（支持 SysV 与 Linux）、校准 program header 大小、验证段对齐与 canonical 地址、保证入口地址落在可加载段中、解析 `PT_GNU_STACK`/`PT_TLS` 附加元信息。
- **交互**：为 `process::exec` 提供镜像元数据；`user::AddressSpace` 利用 `stack_flags` 调整栈权限并在未来消费 `tls_template` 构建 TLS 区域。

### `user`
- **职责**：用户态上下文、地址空间、栈布局与陷入帧管理。
- **关键类型**：`AddressSpace`、`SegmentMapping`、`TrapFrame`、`UserContext`、`Stack`（包含栈权限 `MemoryFlags`）。
- **交互**：`process::set_program_image` 调用 `AddressSpace::from_executable` 构造映射，并根据 ELF 的 `stack_flags` 与自定义 `StackConfig` 合并栈权限；`syscall` 在陷入/返回时读写 `TrapFrame`。

### `shell`
- **职责**：内核内置的简易 shell 调度 `userland` 的命令以及 bare shell 的内建指令。
- **关键类型**：`ShellSubsystem`、`KernelShellFs`、`KernelShellSystem`。
- **交互**：监听串口输入（通过 `arch::x86_64::serial`），调用 `userland::BareShell`，并利用 `kernel::fs` 实现文件访问。

## userland crate

`userland` 提供 POSIX 风格的原型用户态，包括标准 shell、syscall 演示工具和 bare-metal 环境命令。

### `bare_shell`
- **职责**：在无操作系统环境下工作的简化 shell，支持 `ls`, `cd`, `cp`, `mv`, `touch`, `rm`, `mkdir`, `rmdir` 等命令。
- **结构**：主要类型 `BareShell<Io, Fs, Sys>`，`commands` 子模块拆分每个文件操作实现。
- **交互**：通过 `ShellFs` trait 与内核/模拟文件系统通信，通过 `ShellSystem` trait 执行 `reboot`、`shutdown` 等控制操作。

### `shell`
- **职责**：std 环境下的交互式 shell，解析命令、历史、管道等，最终通过 `syscall` 与内核通信。

### `command_util`
- **职责**：为 std shell 的各个二进制工具提供公共解析/输出函数。

### `syscall`
- **职责**：封装与内核的消息协议，提供 `Runtime`、`SyscallRequest` 等类型，便于用户态程序复用。

### `src/bin` 目录
- **职责**：实现 `ls`, `cat`, `touch`, `mv` 等命令行工具，依赖 `command_util` 与 `syscall`。
- **交互**：在标准环境中编译运行，用于驱动内核 syscall 实现的验证。

## libc-lite crate

- **职责**：提供最小化的 C 标准库替身，专注于 syscalls 包装、字符串与内存工具。
- **关键元素**：`build.rs` 负责生成绑定；`src/lib.rs` 暴露裸函数供运行时链接。
- **交互**：未来将与真实的 ELF 用户程序结合，替换当前用户态二进制的依赖。

## xtask 工具

- **职责**：构建、打包与镜像管理的脚本化入口，包含 `bootimage`, `qemu` 等子命令。
- **关键逻辑**：解析 CLI 参数、调用 Cargo 构建、准备根文件系统（ext4），并可使用 `bootloader` crate 打包成可启动镜像。

## 交互总览

```text
+-------------+      syscalls       +-----------------+
|  userland   | ------------------> |      kernel     |
|  shell/bin  | <------------------ |  fs / process   |
+-------------+                     +-----------------+
        |                                    |
        | xtask build/rootfs                 |
        v                                    v
     assets/ -------------------------> boot image (GRUB)
```

- 构建流程：`xtask bootimage` -> 编译用户态与内核 -> 生成 ext4 根文件系统 -> 打包成可启动镜像。
- 运行流程：串口或控制台输入由 `kernel::shell` 转发给 `userland::BareShell`，系统调用再回落到 `kernel::syscall` 与 `kernel::posix`。
1
本文件将随模块演进持续更新，欢迎在新增子系统或重大重构后补充说明。
