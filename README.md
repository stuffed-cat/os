# os – Hybrid Kernel Architecture (x86-64, POSIX-oriented)

This repository documents and prototypes a Rust-based hybrid kernel for x86-64 that aims to blend microkernel isolation with monolithic fast-path services while offering a POSIX-friendly, Unix-like surface to userland. The current code focuses on architectural scaffolding, subsystem contracts, and developer documentation.

## 🎯 Project Goals

- **Hybrid Kernel**: Combine the simplicity and performance of monolithic kernels with the modular safety of microkernels.
- **POSIX / Unix-like Semantics**: Provide familiar abstractions and syscall semantics to run standard userland environments.
- **Rust-first Implementation**: Leverage Rust's safety guarantees for both kernel and user space components.
- **Incremental Bring-up**: Start with architectural scaffolding, grow toward a bootable system.

## 🧱 Architecture Overview

The system is split into two Rust crates inside a Cargo workspace:

| Crate | Purpose |
|-------|---------|
| `kernel` | Core hybrid kernel primitives, subsystems, hardware abstraction, and a POSIX compatibility layer. |
| `userland` | Prototype POSIX-style binary that exercises syscall semantics in a hosted environment. |

### Kernel Structure

The kernel crate (`kernel/src`) is organized into cohesive modules:

- `core`: Kernel state machine, subsystem trait, and builder that wires everything together.
- `error`: Typed error handling for kernel/subsystem interactions.
- `arch`: x86-64 architecture-specific bootstrapping, trap handling, and interrupt controller stubs.
- `hal`: Hardware abstraction layer that wraps architectural primitives in higher-level services.
- `memory`: Virtual memory manager blending capability-based isolation with fast monolithic mapping paths.
- `process`: Process table, capability tracking, and process control blocks.
- `scheduler`: Hybrid scheduler with class-based queues for kernel vs. user threads.
- `ipc`: Message-passing fabric (mailboxes) to support microkernel-style service isolation.
- `services`: Type-driven registry for kernel services discoverable at runtime.
- `posix`: POSIX compatibility shim exposing errno values, signals, and path normalization.
- `syscall`: Syscall dispatcher that bridges userland entrypoints to POSIX operations.

```
+-------------------------+          +-----------------------------+
|         userland        |  syscalls|            kernel            |
|  POSIX apps, libc, CLI  | <------> | dispatcher -> POSIX layer    |
+-------------------------+          |   |            |             |
                                     |   v            v             |
                                     | scheduler   process table    |
                                     |   |             |            |
                                     |   +--> memory, IPC, services |
                                     +-----------------------------+
```

#### Hybrid Design Principles

1. **Subsystem Contracts**: Every major component implements the `Subsystem` trait. During boot the kernel iterates through a list of subsystems, initializing them with a minimal, immutable `KernelContext`.
2. **Microkernel Isolation**: Services such as IPC routers, virtual memory managers, and device drivers can be registered via `ServiceRegistry` and accessed through capability tokens. IPC messages are routed through mailboxes (`ipc::Mailbox`) which sit at the microkernel-like boundary.
3. **Monolithic Fast Paths**: Hot-path components like the scheduler and virtual memory manager share direct memory structures (`scheduler::Scheduler`, `memory::VirtualMemoryManager`) to minimize context switches while still enforcing capability limitations.
4. **POSIX Compatibility**: The `posix::PosixLayer` adapts syscall identifiers to Unix-like semantics (errno codes, signals, path normalization). This shim enables traditional userland stacks to run atop the hybrid core.

### Userland Prototype

The `userland` crate provides a simple binary that exercises stdout writes in a hosted environment. Eventually this crate will be replaced by real ELF binaries compiled with a lightweight libc targeting the kernel's syscall ABI.

## 🔄 Control Flow

1. **Bootstrap**: `KernelBuilder` constructs the kernel, registering services and subsystems. `arch::x86_64::ArchBootstrap` performs hardware checks.
2. **Subsystem Init**: Each subsystem's `init` method is called in the `KernelState::Bootstrap` stage to perform internal setup.
3. **Run Loop**: Transition to `KernelState::Running`, execute subsystem `tick` hooks. The scheduler would switch between kernel/user threads, honoring IPC events.
4. **Syscalls**: Traps from userland are decoded by `syscall::SyscallDispatcher`, routed to the POSIX layer, and serviced using registered kernel facilities.

## 🧪 Testing Strategy

- `kernel` crate includes unit tests ensuring subsystems initialize and tick in order.
- Future work: Integration tests simulating IPC exchanges, scheduling rotations, and POSIX syscall semantics.

Run the current test suite:

```bash
cargo test
```

## 🛣️ Roadmap

- [ ] Bootable x86-64 image (GRUB/multiboot) with minimal `no_std` runtime.
- [ ] Memory management unit (paging structures, frame allocator).
- [ ] Preemptive scheduler integrated with hardware timer interrupts.
- [ ] IPC capability security policies and userland channel binding.
- [ ] Filesystem service (initially ramfs) exposing POSIX file descriptors.
- [ ] Expand syscall surface (`fork`, `exec`, `open`, `read`, `wait`, etc.).

## 🔐 Licensing

Licensed under the MIT license. See [`LICENSE`](LICENSE) for details.
