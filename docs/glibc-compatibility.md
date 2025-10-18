# glibc 接口兼容性评估

> 更新时间：2025-10-19

本记录旨在梳理当前内核对 Linux/glibc 运行时所需接口的支持情况，并给出补齐路线。分析基于 `kernel/src/syscall.rs`, `kernel/src/posix.rs`, `kernel/src/process.rs`, `kernel/src/memory.rs` 以及用户态 `libc-lite`/`userland` 现状。

## 现有系统调用面

内核目前公开的 syscall ID 映射如下：

| 编号 | SyscallId | 对应 POSIX / Linux 调用 | 实现位置 |
| ---- | --------- | ----------------------- | -------- |
| 0    | `Read`    | `read`                  | `PosixLayer::read` |
| 1    | `Write`   | `write`                 | `PosixLayer::write` |
| 2    | `Open`    | `open`（文件句柄映射）  | `PosixLayer::open` |
| 3    | `Close`   | `close`                 | `PosixLayer::close` |
| 22   | `Pipe`    | `pipe`                  | `PosixLayer::pipe` |
| 32   | `Dup`     | `dup`                   | `PosixLayer::dup` |
| 33   | `Dup2`    | `dup2`                  | `PosixLayer::dup2` |
| 35   | `Sleep`   | `nanosleep` 占位       | `PosixLayer::sleep` |
| 39   | `GetPid`  | `getpid`                | `PosixLayer::dispatch` |
| 57   | `Fork`    | `fork`（写时复制 TODO） | `ProcessTable::fork` |
| 59   | `Exec`    | `execve`                | `PosixLayer::exec` |
| 60   | `Exit`    | `exit` / `exit_group`   | `PosixLayer::exit` |
| 61   | `WaitPid` | `wait4` 变体           | `PosixLayer::waitpid` |
| 79   | `GetCwd`  | `getcwd`                | `PosixLayer::getcwd` |
| 80   | `Chdir`   | `chdir`                 | `PosixLayer::chdir` |

除此之外，其余编号统统落入 `Unknown` 分支并返回 `ENOSYS`。

> **观察**：整个系统调用面仍停留在“微型 shell + 基础 I/O”的水平，尚未覆盖 glibc 初始化所依赖的内存管理、信号、线程、时间、文件属性等接口。

## glibc 启动的最小接口集合

基于 glibc 对 Linux x86_64 ABI 的要求，静态/动态链接程序启动通常至少涉及以下调用：

1. **进程 / 线程管理**：`exit_group`, `clone`, `set_tid_address`, `set_robust_list`, `gettid`, `tgkill`
2. **内存管理**：`brk`, `mmap`, `munmap`, `mprotect`, `mremap`
3. **TLS / 寄存器**：`arch_prctl`, `set_thread_area`
4. **文件描述符语义**：`openat`, `close`, `read`, `write`, `lseek`, `pread64/pwrite64`, `fstat`, `statx` 或 `newfstatat`, `fcntl`, `ioctl`
5. **目录 / 链接**：`getdents64`, `renameat2`, `unlinkat`, `mkdirat`
6. **时间 & 时钟**：`clock_gettime`, `clock_getres`, `nanosleep`
7. **信号**：`rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn`
8. **同步原语**：`futex`
9. **随机数**：`getrandom`
10. **misc**：`uname`, `prlimit64`, `socketpair`（若要支持网络）等

此外 glibc 会根据构建选项尝试 `sysinfo`, `sched_getaffinity`, `getpid`, `getppid`, `umask` 等。

## 差距矩阵

| 类别 | 关键需求 | 当前状态 | 差距 & 建议 |
| ---- | -------- | -------- | ----------- |
| 进程控制 | `exit_group` | `Exit` 等价，但需要统一退出所有线程 | 将 `Exit` 语义改为 `exit_group`，对单线程进程保持兼容 |
| 线程/clone | `clone`, `set_tid_address`, `set_robust_list`, `gettid` | 未实现 | 引入线程表管理：复用 `Process::next_tid`，在 `ProcessTable` 中记录 TCB，补齐 `SyscallId` |- `clone` 可先实现共享地址空间/文件描述符的单线程模拟，后续扩展真正的线程 |
| 内存管理 | `mmap`, `munmap`, `mprotect`, `brk` | `memory` 模块仅支持静态段映射 | 需要面向用户态的分页 API：<br/>- 扩展 `MemoryManager`，实现匿名页分配/回收<br/>- 在 `Process` 或 `AddressSpace` 中记录 VMA<br/>- 实现简单的 `brk` 堆指针 |
| TLS/寄存器 | `arch_prctl` (for FS/GS base) | ELF loader 已解析 TLS，但缺少系统调用 | 在 `TrapFrame` 暴露 fs/gs 基址控制；利用 `x86_64::registers::model_specific` 写 MSR |
| 文件属性 | `fstat`, `statx`, `lseek`, `pread/pwrite`, `fcntl` | 只支持 read/write/close | - `Fs` 层已有 metadata API，可重写成 `fstat`
- `lseek` -> 操作 `Process::fd_offsets`
- `fcntl` 至少实现 `F_GETFD/F_SETFD`
- `pread/pwrite` 可封装 `read_file_with_credentials` 绕过偏移维护 |
| 目录操作 | `openat`, `mkdirat`, `unlinkat`, `renameat`, `getdents64` | 完全缺失 | - 重用 `fs` overlay/基础实现，补齐 path 解析（处理相对路径 + fd）
- `getdents64` 可直接从 `list_dir_with_credentials` 中转换 | 
| 时间 | `clock_gettime`, `clock_getres`, `gettimeofday` | 只有毫秒睡眠 (`Sleep`) | 提供基于 `hal::time::monotonic()` 或 PIT 的时钟源；必要时使用 TSC 比率 |
| 信号 | `rt_sigaction`, `rt_sigprocmask`, `rt_sigreturn` | 完全缺失 | 需要信号描述表、挂起位图、用户栈帧保存；可分阶段实现：先 stub 返回 `-ENOSYS` 并在 glibc 初始化时进行降级 |
| 同步 | `futex` | 缺失 | 首阶段实现用户态等待/唤醒表，复用调度器休眠队列 |
| 随机 | `getrandom` | 缺失 | 可从 RDRAND/RDSEED（x86_64）或 ChaCha PRNG 提供 |
| 其它 | `uname`, `sysinfo`, `prlimit64` | 缺失 | 简单结构可直接填充常量 |

## 建议的推进顺序

1. **内存管理基线**：先实现 `brk` + 匿名 `mmap/munmap`，配合 `mprotect` 的最简读写控制。无此功能 glibc 无法初始化堆与 TLS。
2. **时间/时钟与 `clock_gettime`**：glibc 初始化会调用 `__clock_gettime`，尽早补齐。
3. **文件属性增强**：实现 `fstat`, `lseek`, `fcntl(F_GETFL/F_SETFL)`, `getdents64`，保证基础文件操作可用。
4. **线程原语**：`clone`（带 `CLONE_SETTLS`）、`set_tid_address`、`futex`。即便短期内不开放真正多线程，也需提供最小语义避免 glibc 早期崩溃。
5. **信号框架**：定义 `Signal` 结构、安装/恢复上下文。
6. **随机与 uname 等附属功能**。

每一步都需要用户态测试：
- 使用 `picolibc`/`libc-lite` 引入 glibc 运行时单元测试。
- 构建最小 glibc 程序（`int main(){return 0;}`），确认 `_start -> __libc_start_main` 过程完成。

## 代码接入点

| 区域 | 作用 | 备注 |
| ---- | ---- | ---- |
| `kernel/src/syscall.rs` | 扩展 syscall 枚举与解码 | 添加新 ID 与 trap 参数顺序 |
| `kernel/src/posix.rs` | 大部分 POSIX wrapper | 可新增多个 `fn` 并维护文件描述符状态 |
| `kernel/src/process.rs` | 进程/线程表，fd offset 管理 | 需要扩展以支持 TID、信号、VMA |
| `kernel/src/memory.rs` | 页分配与地址空间 | 现有实现偏向内核，需引入用户态 API |
| `kernel/src/scheduler.rs` | 睡眠与唤醒 | 为 `futex`、信号提供阻塞/唤醒原语 |
| `kernel/src/hal.rs` / `arch/x86_64` | 时间、中断、寄存器 | `clock_gettime`、`arch_prctl` 实现位置 |

## 后续工作追踪

1. 在 `docs/architecture.md` 增加 glibc 兼容部分，随着实现推进更新状态矩阵。
2. 为关键 syscall 添加内核单元测试与用户态集成测试（例如通过 `tests/integration.rs` 执行小型 glibc 二进制）。
3. 规划一个“兼容性 gating” CI：编译并运行 `glibc` 核心测试或 musl 的 smoke tests。

---

如需扩展，请先在 issue/设计文档中写明 ABI 约定及与现有 `userland` 的兼容策略，避免 syscall 编号冲突。