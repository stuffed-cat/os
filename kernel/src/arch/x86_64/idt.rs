//! Interrupt Descriptor Table initialization and handlers.

use log::{error, info, trace};
use spin::Once;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

#[cfg(feature = "hardware")]
use core::arch::naked_asm;
#[cfg(feature = "hardware")]
use core::mem::transmute;
#[cfg(feature = "hardware")]
use core::ptr;
#[cfg(feature = "hardware")]
use core::sync::atomic::{AtomicBool, Ordering};
#[cfg(feature = "hardware")]
use x86_64::registers::control::{Cr3, Cr3Flags};
#[cfg(feature = "hardware")]
use x86_64::registers::rflags::RFlags;
#[cfg(feature = "hardware")]
use x86_64::structures::idt::InterruptStackFrameValue;
#[cfg(feature = "hardware")]
use x86_64::structures::paging::{PageTable, PageTableFlags, PhysFrame};
#[cfg(feature = "hardware")]
use x86_64::PrivilegeLevel;
#[cfg(feature = "hardware")]
use x86_64::VirtAddr;

use super::{
    gdt,
    interrupts::{self, InterruptIndex},
    keyboard,
};
#[cfg(feature = "hardware")]
use crate::arch::x86_64::serial;
use crate::scheduler::Scheduler;
use crate::shell::{self, UserFaultKind};
#[cfg(feature = "hardware")]
use crate::{
    error::KernelError,
    process::{KernelContext, ProcessTable},
    scheduler::{SchedulingClass, TimerTickOutcome},
    syscall::SyscallDispatcher,
    user::{TrapFrame, UserContext},
};

#[cfg(feature = "hardware")]
static LOGGED_USER_ENTRY: AtomicBool = AtomicBool::new(false);

#[cfg(feature = "hardware")]
fn dump_user_page_entry(addr: u64) {
    use x86_64::registers::control::Cr3;

    let Some(table) = ProcessTable::global() else {
        return;
    };
    let Some(manager) = table.memory_manager() else {
        return;
    };

    let phys_offset = manager.physical_memory_offset();

    unsafe {
        let (root, _) = Cr3::read();
        let mut frame = root;
        let indices = [
            ((addr >> 39) & 0x1ff) as usize,
            ((addr >> 30) & 0x1ff) as usize,
            ((addr >> 21) & 0x1ff) as usize,
            ((addr >> 12) & 0x1ff) as usize,
        ];

        for (level, &index) in indices.iter().enumerate() {
            let table_ptr =
                (phys_offset + frame.start_address().as_u64()).as_u64() as *const PageTable;
            let table_ref = &*table_ptr;
            let entry = &table_ref[index];
            
            // Read raw entry value to check NX bit (bit 63)
            let raw_entry = core::ptr::read_volatile(&table_ref[index] as *const _ as *const u64);
            let nx_bit = (raw_entry >> 63) & 1;
            
            serial::write_fmt(format_args!(
                "pte L{} idx={} flags={:?} addr={:#x} raw={:#x} NX={}\r\n",
                4 - level,
                index,
                entry.flags(),
                entry.addr().as_u64(),
                raw_entry,
                nx_bit
            ));

            if !entry.flags().contains(PageTableFlags::PRESENT) {
                break;
            }

            if level == 3 {
                break;
            }

            let Ok(next_frame) = PhysFrame::from_start_address(entry.addr()) else {
                break;
            };
            frame = next_frame;
        }
    }
}

static IDT: Once<InterruptDescriptorTable> = Once::new();

const SYSCALL_VECTOR: u8 = 0x80;

/// Check if the exception occurred in user mode
fn is_user_mode_exception(stack_frame: &InterruptStackFrame) -> bool {
    // Check the code segment privilege level (bits 0-1)
    // 3 = user mode (ring 3), 0 = kernel mode (ring 0)
    (stack_frame.code_segment.0 & 3) == 3
}

#[cfg(feature = "hardware")]
#[repr(C)]
struct SavedRegisters {
    rax: u64,
    rbx: u64,
    rcx: u64,
    rdx: u64,
    rsi: u64,
    rdi: u64,
    r8: u64,
    r9: u64,
    r10: u64,
    r11: u64,
    r12: u64,
    r13: u64,
    r14: u64,
    r15: u64,
    rbp: u64,
}

#[cfg(feature = "hardware")]
impl SavedRegisters {
    fn snapshot(&self, frame: &InterruptStackFrameValue) -> TrapFrame {
        let mut trap = TrapFrame::default();
        trap.rax = self.rax;
        trap.rbx = self.rbx;
        trap.rcx = self.rcx;
        trap.rdx = self.rdx;
        trap.rsi = self.rsi;
        trap.rdi = self.rdi;
        trap.r8 = self.r8;
        trap.r9 = self.r9;
        trap.r10 = self.r10;
        trap.r11 = self.r11;
        trap.r12 = self.r12;
        trap.r13 = self.r13;
        trap.r14 = self.r14;
        trap.r15 = self.r15;
        trap.rbp = self.rbp;
        trap.rsp = frame.stack_pointer.as_u64();
        trap.rip = frame.instruction_pointer.as_u64();
        trap.rflags = frame.cpu_flags.bits();
        trap
    }

    fn apply_user_context(&mut self, context: &UserContext) {
        let frame = context.frame();
        self.rax = frame.rax;
        self.rbx = frame.rbx;
        self.rcx = frame.rcx;
        self.rdx = frame.rdx;
        self.rsi = frame.rsi;
        self.rdi = frame.rdi;
        self.r8 = frame.r8;
        self.r9 = frame.r9;
        self.r10 = frame.r10;
        self.r11 = frame.r11;
        self.r12 = frame.r12;
        self.r13 = frame.r13;
        self.r14 = frame.r14;
        self.r15 = frame.r15;
        self.rbp = frame.rbp;
    }
}

#[cfg(feature = "hardware")]
fn write_user_frame(frame: &mut InterruptStackFrameValue, context: &UserContext) {
    let trap = context.frame();
    frame.instruction_pointer = VirtAddr::new(trap.rip);
    frame.stack_pointer = VirtAddr::new(trap.rsp);
    frame.cpu_flags = RFlags::from_bits_truncate(trap.rflags);
    frame.code_segment = gdt::user_code_selector();
    frame.stack_segment = gdt::user_data_selector();
}

#[cfg(feature = "hardware")]
fn snapshot_kernel_context(
    regs: &SavedRegisters,
    frame: &InterruptStackFrameValue,
) -> KernelContext {
    KernelContext {
        rax: regs.rax,
        rbx: regs.rbx,
        rcx: regs.rcx,
        rdx: regs.rdx,
        rsi: regs.rsi,
        rdi: regs.rdi,
        r8: regs.r8,
        r9: regs.r9,
        r10: regs.r10,
        r11: regs.r11,
        r12: regs.r12,
        r13: regs.r13,
        r14: regs.r14,
        r15: regs.r15,
        rbp: regs.rbp,
        rsp: frame.stack_pointer.as_u64(),
        rip: frame.instruction_pointer.as_u64(),
        rflags: frame.cpu_flags.bits(),
    }
}

#[cfg(feature = "hardware")]
fn apply_kernel_context(
    regs: &mut SavedRegisters,
    frame: &mut InterruptStackFrameValue,
    context: &KernelContext,
) {
    regs.rax = context.rax;
    regs.rbx = context.rbx;
    regs.rcx = context.rcx;
    regs.rdx = context.rdx;
    regs.rsi = context.rsi;
    regs.rdi = context.rdi;
    regs.r8 = context.r8;
    regs.r9 = context.r9;
    regs.r10 = context.r10;
    regs.r11 = context.r11;
    regs.r12 = context.r12;
    regs.r13 = context.r13;
    regs.r14 = context.r14;
    regs.r15 = context.r15;
    regs.rbp = context.rbp;
    frame.instruction_pointer = VirtAddr::new(context.rip);
    frame.stack_pointer = VirtAddr::new(context.rsp);
    frame.cpu_flags = RFlags::from_bits_truncate(context.rflags);
}

#[cfg(feature = "hardware")]
fn switch_address_space(root: PhysFrame) {
    unsafe {
        let (current, _) = Cr3::read();
        if current != root {
            Cr3::write(root, Cr3Flags::empty());
        } else {
            // Force TLB flush even if same CR3 (important for first user entry)
            x86_64::instructions::tlb::flush_all();
        }
    }
}

/// Prepares the IDT entries but does not load it.
pub fn init() {
    IDT.call_once(|| {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault
                .set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::double_fault_ist_index());
        }
        #[cfg(any(feature = "hardware", feature = "boot"))]
        unsafe {
            let handler: extern "C" fn() -> ! = timer_interrupt_trampoline;
            let converted: extern "x86-interrupt" fn(InterruptStackFrame) = transmute(handler);
            idt[InterruptIndex::Timer.as_u8()].set_handler_fn(converted);
        }
        #[cfg(not(any(feature = "hardware", feature = "boot")))]
        idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        #[cfg(any(feature = "hardware", feature = "boot"))]
        unsafe {
            let handler: extern "C" fn() -> ! = invalid_opcode_trampoline;
            let converted: extern "x86-interrupt" fn(InterruptStackFrame) = transmute(handler);
            idt.invalid_opcode.set_handler_fn(converted);
        }
        #[cfg(not(any(feature = "hardware", feature = "boot")))]
        idt.invalid_opcode.set_handler_fn(invalid_opcode_handler);
        #[cfg(any(feature = "hardware", feature = "boot"))]
        unsafe {
            let handler: extern "C" fn() -> ! = syscall_interrupt_trampoline;
            let converted: extern "x86-interrupt" fn(InterruptStackFrame) = transmute(handler);
            idt[SYSCALL_VECTOR]
                .set_handler_fn(converted)
                .set_privilege_level(PrivilegeLevel::Ring3);
        }
        idt
    });
}

/// Loads the IDT into the CPU.
pub fn load() {
    if let Some(idt) = IDT.get() {
        idt.load();
    }
}

extern "x86-interrupt" fn breakpoint_handler(stack_frame: InterruptStackFrame) {
    if is_user_mode_exception(&stack_frame) {
        trace!(
            "User mode BREAKPOINT at {:#x}",
            stack_frame.instruction_pointer.as_u64()
        );
    } else {
        trace!("Kernel mode BREAKPOINT: {:?}", stack_frame);
    }
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _code: u64) -> ! {
    if is_user_mode_exception(&stack_frame) {
        error!(
            "User mode DOUBLE FAULT\nRIP: {:#x}",
            stack_frame.instruction_pointer.as_u64()
        );
        crate::shell::mark_current_process_failed();

        // This is still fatal, but at least we tried to log it
        panic!("DOUBLE FAULT (USER): {:?}", stack_frame);
    }

    panic!("DOUBLE FAULT (KERNEL): {:?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    if is_user_mode_exception(&stack_frame) {
        // User mode page fault - handle gracefully
        let fault_address = Cr2::read().map(|addr| addr.as_u64()).unwrap_or(0);

        shell::record_user_fault(
            UserFaultKind::PageFault,
            stack_frame.instruction_pointer.as_u64(),
            fault_address,
            error_code.bits() as u64,
        );

        #[cfg(feature = "hardware")]
        serial::write_fmt(format_args!("\r\n=== USER PAGE FAULT ===\r\n"));

        #[cfg(feature = "hardware")]
        serial::write_fmt(format_args!(
            "RIP: {:#x}\r\nFault Address: {:#x}\r\nError Code: {:?}\r\n",
            stack_frame.instruction_pointer.as_u64(),
            fault_address,
            error_code
        ));

        #[cfg(feature = "hardware")]
        serial::write_fmt(format_args!(
            "Present: {}, Write: {}, User: {}, InstFetch: {}\r\n",
            !error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION),
            error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE),
            error_code.contains(PageFaultErrorCode::USER_MODE),
            error_code.contains(PageFaultErrorCode::INSTRUCTION_FETCH)
        ));
        
        #[cfg(feature = "hardware")]
        serial::write_fmt(format_args!(
            "CS: {:#x} (CPL={}), SS: {:#x}\r\n",
            stack_frame.code_segment.0,
            stack_frame.code_segment.0 & 3,
            stack_frame.stack_segment.0
        ));

        #[cfg(feature = "hardware")]
        {
            serial::write_str("Fault address page table:\r\n");
            dump_user_page_entry(fault_address);
            serial::write_str("RIP page table:\r\n");
            dump_user_page_entry(stack_frame.instruction_pointer.as_u64());
            
            // Try to read the instruction bytes at RIP via physical address
            let Some(table) = ProcessTable::global() else { return; };
            let Some(manager) = table.memory_manager() else { return; };
            let phys_offset = manager.physical_memory_offset();
            let rip_phys_addr = 0x4e3000u64 + (stack_frame.instruction_pointer.as_u64() & 0xfff);
            let inst_ptr = (phys_offset + rip_phys_addr).as_u64() as *const u8;
            unsafe {
                serial::write_str("Instruction bytes at RIP (via physical): ");
                for i in 0..16 {
                    serial::write_fmt(format_args!("{:02x} ", *inst_ptr.add(i)));
                }
                serial::write_str("\r\n");
            }
        }

        #[cfg(feature = "hardware")]
        serial::write_fmt(format_args!("=== END PAGE FAULT INFO ===\r\n"));

        error!(
            "User mode PAGE FAULT\nRIP: {:#x}\nFaulted Address: {:#x}\nError Code: {:?}",
            stack_frame.instruction_pointer.as_u64(),
            fault_address,
            error_code
        );

        crate::shell::mark_current_process_failed();

        // Enable interrupts and enter debug halt loop
        x86_64::instructions::interrupts::enable();
        loop {
            x86_64::instructions::hlt();
        }
    }

    // Kernel mode page fault - this is fatal
    let cr2_val = Cr2::read().unwrap_or(VirtAddr::new(0));
    let rip = stack_frame.instruction_pointer.as_u64();
    let rsp = stack_frame.stack_pointer.as_u64();
    panic!(
        "PAGE FAULT (KERNEL): RIP={:#x} RSP={:#x}\nAccessed Address: {:#x}\nError Code: {:?}",
        rip,
        rsp,
        cr2_val.as_u64(),
        error_code
    );
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    if is_user_mode_exception(&stack_frame) {
        // User mode exception - handle gracefully
        shell::record_user_fault(
            UserFaultKind::GeneralProtection,
            stack_frame.instruction_pointer.as_u64(),
            0,
            error_code,
        );

        #[cfg(feature = "hardware")]
        {
            let (rbp, rsp): (u64, u64);
            unsafe {
                core::arch::asm!(
                    "mov {}, rbp",
                    "mov {}, rsp",
                    out(reg) rbp,
                    out(reg) rsp,
                );
            }
            serial::write_fmt(format_args!(
                "\r\n=== USER GP FAULT ===\r\nRIP: {:#x}\r\nRSP: {:#x} (mod 16 = {})\r\nRBP: {:#x} (mod 16 = {})\r\nError Code: {:#x}\r\n",
                stack_frame.instruction_pointer.as_u64(),
                rsp, rsp & 0xF,
                rbp, rbp & 0xF,
                error_code
            ));
        }

        error!(
            "User mode GENERAL PROTECTION FAULT\nRIP: {:#x}\nError Code: {:#x}",
            stack_frame.instruction_pointer.as_u64(),
            error_code
        );

        // Signal to shell to skip this process
        crate::shell::mark_current_process_failed();

        // Halt and wait for timer interrupt to schedule next process
        x86_64::instructions::interrupts::enable();
        loop {
            x86_64::instructions::hlt();
        }
    }

    // Kernel mode exception - this is fatal
    panic!(
        "GENERAL PROTECTION FAULT (KERNEL): {:?}\nError Code: {:#x}",
        stack_frame, error_code
    );
}

#[cfg(not(feature = "hardware"))]
extern "x86-interrupt" fn invalid_opcode_handler(stack_frame: InterruptStackFrame) {
    if is_user_mode_exception(&stack_frame) {
        // User mode exception
        shell::record_user_fault(
            UserFaultKind::InvalidOpcode,
            stack_frame.instruction_pointer.as_u64(),
            0,
            0,
        );

        #[cfg(feature = "hardware")]
        serial::write_fmt(format_args!(
            "\r\n=== USER INVALID OPCODE ===\r\nRIP: {:#x}\r\n",
            stack_frame.instruction_pointer.as_u64()
        ));

        error!(
            "User mode INVALID OPCODE\nRIP: {:#x}",
            stack_frame.instruction_pointer.as_u64()
        );
        crate::shell::mark_current_process_failed();

        // Halt and wait for timer interrupt to schedule next process
        x86_64::instructions::interrupts::enable();
        loop {
            x86_64::instructions::hlt();
        }
    }

    panic!("INVALID OPCODE (KERNEL): {:?}", stack_frame);
}

#[cfg(feature = "hardware")]
unsafe extern "C" fn timer_interrupt_handler(
    regs: *mut SavedRegisters,
    frame: *mut InterruptStackFrameValue,
) {
    let regs = &mut *regs;
    let frame = &mut *frame;

    // Timer interrupt - schedule next runnable thread
    static TIMER_COUNT: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);
    let count = TIMER_COUNT.fetch_add(1, Ordering::Relaxed);
    if count < 5 || count % 100 == 0 {
        log::trace!("timer interrupt #{}", count);
    }

    if let Some(table) = ProcessTable::global() {
        if let Some(root) = table.kernel_root_frame() {
            switch_address_space(root);
        }
    }

    if let Some(scheduler) = Scheduler::global() {
        let TimerTickOutcome { preempted, next } = scheduler.evaluate_timer_tick();

        if let Some(entry) = preempted {
            if let Some(table) = ProcessTable::global() {
                match entry.class {
                    SchedulingClass::User => {
                        let trap = regs.snapshot(frame);
                        let context = UserContext::from_trap_frame(trap);
                        table.store_thread_context(entry.pid, entry.tid, context);
                    }
                    SchedulingClass::Kernel => {
                        let context = snapshot_kernel_context(regs, frame);
                        table.store_kernel_context(entry.pid, entry.tid, context);
                    }
                }
            }
        }

        // Try to get the next thread to schedule (either from evaluate_timer_tick or pick_next)
        let next_entry = next.or_else(|| scheduler.pick_next());

        if count < 10 {
            match &next_entry {
                Some(e) => log::info!(
                    "timer #{}: got next entry pid={} tid={}",
                    count,
                    e.pid.as_u64(),
                    e.tid.as_u64()
                ),
                None => log::info!("timer #{}: no next entry", count),
            }
        }

        if let Some(entry) = next_entry {
            match entry.class {
                SchedulingClass::User => {
                    if let Some(table) = ProcessTable::global() {
                        if let Some(proc) = table.lookup(entry.pid) {
                            if let Some((context, root)) = proc.take_thread_runtime(entry.tid) {
                                static ENTRY_COUNT: core::sync::atomic::AtomicU64 =
                                    core::sync::atomic::AtomicU64::new(0);
                                static LAST_RIP: core::sync::atomic::AtomicU64 =
                                    core::sync::atomic::AtomicU64::new(0);
                                let count = ENTRY_COUNT.fetch_add(1, Ordering::Relaxed);
                                let trap = context.frame();
                                let last_rip = LAST_RIP.swap(trap.rip, Ordering::Relaxed);

                                if count < 10
                                    || count % 50 == 0
                                    || (count < 100 && last_rip == trap.rip)
                                {
                                    let status = if last_rip == trap.rip { "STUCK" } else { "ok" };
                                    log::info!(
                                        "timer: resuming user #{} pid={} tid={} rip={:#x} [{}]",
                                        count,
                                        entry.pid.as_u64(),
                                        entry.tid.as_u64(),
                                        trap.rip,
                                        status
                                    );
                                }
                                regs.apply_user_context(&context);
                                write_user_frame(frame, &context);
                                switch_address_space(root);
                            } else if LOGGED_USER_ENTRY
                                .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
                                .is_ok()
                            {
                                info!(
                                    "timer: missing user context for pid={} tid={} (no runtime state)",
                                    entry.pid.as_u64(),
                                    entry.tid.as_u64()
                                );
                            }
                        } else if LOGGED_USER_ENTRY
                            .compare_exchange(false, true, Ordering::SeqCst, Ordering::Relaxed)
                            .is_ok()
                        {
                            info!(
                                "timer: missing process for pid={} during scheduling",
                                entry.pid.as_u64()
                            );
                        }
                    }
                }
                SchedulingClass::Kernel => {
                    if let Some(table) = ProcessTable::global() {
                        if let Some(context) = table.take_kernel_context(entry.pid, entry.tid) {
                            apply_kernel_context(regs, frame, &context);
                        }
                    }
                }
            }
        }
    }

    interrupts::notify_end_of_interrupt(InterruptIndex::Timer);
}

#[cfg(feature = "hardware")]
unsafe extern "C" fn syscall_interrupt_handler(
    regs: *mut SavedRegisters,
    frame: *mut InterruptStackFrameValue,
) {
    let regs = &mut *regs;
    let frame = &mut *frame;

    if let Some(table) = ProcessTable::global() {
        if let Some(root) = table.kernel_root_frame() {
            switch_address_space(root);
        }
    }

    let Some(scheduler) = Scheduler::global() else {
        return;
    };

    let Some(entry) = scheduler.current_thread() else {
        return;
    };

    if entry.class != SchedulingClass::User {
        return;
    }

    let Some(table) = ProcessTable::global() else {
        return;
    };

    let Some(proc) = table.lookup(entry.pid) else {
        return;
    };

    let mut context = UserContext::from_trap_frame(regs.snapshot(frame));

    let result = SyscallDispatcher::global()
        .ok_or_else(|| KernelError::Unimplemented("syscall dispatcher not registered"))
        .and_then(|dispatcher| dispatcher.handle_trap(entry.pid, context.frame_mut()));

    if let Err(err) = result {
        context
            .frame_mut()
            .set_return_value(encode_kernel_error(err));
    }

    regs.apply_user_context(&context);
    write_user_frame(frame, &context);

    if let Some(root) = proc.page_table_root() {
        switch_address_space(root);
    }
}

#[cfg(feature = "hardware")]
unsafe extern "C" fn invalid_opcode_handler(
    regs: *mut SavedRegisters,
    frame: *mut InterruptStackFrameValue,
) {
    let regs = &mut *regs;
    let frame = &mut *frame;

    let fault_ip = frame.instruction_pointer.as_u64();
    let opcode = unsafe {
        [
            ptr::read(fault_ip as *const u8),
            ptr::read(fault_ip.wrapping_add(1) as *const u8),
        ]
    };

    if opcode != [0x0f, 0x05] {
        panic!(
            "INVALID OPCODE at {fault_ip:#x}: {:02x} {:02x}",
            opcode[0], opcode[1]
        );
    }

    if let Some(table) = ProcessTable::global() {
        if let Some(root) = table.kernel_root_frame() {
            switch_address_space(root);
        }
    }

    let Some(scheduler) = Scheduler::global() else {
        return;
    };

    let Some(entry) = scheduler.current_thread() else {
        return;
    };

    if entry.class != SchedulingClass::User {
        return;
    }

    let Some(table) = ProcessTable::global() else {
        return;
    };

    let Some(proc) = table.lookup(entry.pid) else {
        return;
    };

    let mut context = UserContext::from_trap_frame(regs.snapshot(frame));
    let return_ip = context.frame().rip.wrapping_add(2);
    let flags = context.frame().rflags;

    {
        let frame_mut = context.frame_mut();
        frame_mut.rip = return_ip;
        frame_mut.rcx = return_ip;
        frame_mut.r11 = flags;
    }

    let result = SyscallDispatcher::global()
        .ok_or_else(|| KernelError::Unimplemented("syscall dispatcher not registered"))
        .and_then(|dispatcher| dispatcher.handle_trap(entry.pid, context.frame_mut()));

    if let Err(err) = result {
        context
            .frame_mut()
            .set_return_value(encode_kernel_error(err));
    }

    regs.apply_user_context(&context);
    write_user_frame(frame, &context);

    if let Some(root) = proc.page_table_root() {
        switch_address_space(root);
    }
}

#[cfg(not(feature = "hardware"))]
extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    if let Some(scheduler) = Scheduler::global() {
        scheduler.handle_timer_tick();
    }
    interrupts::notify_end_of_interrupt(InterruptIndex::Timer);
}

#[cfg(feature = "hardware")]
#[unsafe(naked)]
extern "C" fn timer_interrupt_trampoline() -> ! {
    naked_asm!(
        "push rbp",
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbx",
        "push rax",
        "mov rdi, rsp",
        "lea rsi, [rsp + {frame_offset}]",
        "sub rsp, 8",
        "call {handler}",
        "add rsp, 8",
        "pop rax",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "pop rbp",
        "iretq",
        frame_offset = const 15 * 8,
        handler = sym timer_interrupt_handler
    );
}

#[cfg(feature = "hardware")]
#[unsafe(naked)]
extern "C" fn syscall_interrupt_trampoline() -> ! {
    naked_asm!(
        "push rbp",
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbx",
        "push rax",
        "mov rdi, rsp",
        "lea rsi, [rsp + {frame_offset}]",
        "sub rsp, 8",
        "call {handler}",
        "add rsp, 8",
        "pop rax",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "pop rbp",
        "iretq",
        frame_offset = const 15 * 8,
        handler = sym syscall_interrupt_handler
    );
}

#[cfg(feature = "hardware")]
#[unsafe(naked)]
extern "C" fn invalid_opcode_trampoline() -> ! {
    naked_asm!(
        "push rbp",
        "push r15",
        "push r14",
        "push r13",
        "push r12",
        "push r11",
        "push r10",
        "push r9",
        "push r8",
        "push rdi",
        "push rsi",
        "push rdx",
        "push rcx",
        "push rbx",
        "push rax",
        "mov rdi, rsp",
        "lea rsi, [rsp + {frame_offset}]",
        "sub rsp, 8",
        "call {handler}",
        "add rsp, 8",
        "pop rax",
        "pop rbx",
        "pop rcx",
        "pop rdx",
        "pop rsi",
        "pop rdi",
        "pop r8",
        "pop r9",
        "pop r10",
        "pop r11",
        "pop r12",
        "pop r13",
        "pop r14",
        "pop r15",
        "pop rbp",
        "iretq",
        frame_offset = const 15 * 8,
        handler = sym invalid_opcode_handler
    );
}

#[cfg(feature = "hardware")]
fn encode_kernel_error(err: KernelError) -> u64 {
    error!("syscall error: {}", err);
    u64::MAX
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    #[cfg(feature = "hardware")]
    if let Some(table) = ProcessTable::global() {
        if let Some(root) = table.kernel_root_frame() {
            switch_address_space(root);
        }
    }

    let scancode = keyboard::read_scancode();
    shell::enqueue_scancode(scancode);
    interrupts::notify_end_of_interrupt(InterruptIndex::Keyboard);
}
