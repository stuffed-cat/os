//! Interrupt Descriptor Table initialization and handlers.

use log::{error, trace};
use spin::Once;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

#[cfg(feature = "hardware")]
use core::arch::naked_asm;
#[cfg(feature = "hardware")]
use core::mem::transmute;
#[cfg(feature = "hardware")]
use x86_64::registers::control::{Cr3, Cr3Flags};
#[cfg(feature = "hardware")]
use x86_64::registers::rflags::RFlags;
#[cfg(feature = "hardware")]
use x86_64::structures::idt::InterruptStackFrameValue;
#[cfg(feature = "hardware")]
use x86_64::structures::paging::PhysFrame;
#[cfg(feature = "hardware")]
use x86_64::PrivilegeLevel;
#[cfg(feature = "hardware")]
use x86_64::VirtAddr;

use super::{
    gdt,
    interrupts::{self, InterruptIndex},
    keyboard,
};
use crate::scheduler::Scheduler;
use crate::shell;
#[cfg(feature = "hardware")]
use crate::{
    error::KernelError,
    process::{KernelContext, ProcessTable},
    scheduler::{SchedulingClass, TimerTickOutcome},
    syscall::SyscallDispatcher,
    user::{TrapFrame, UserContext},
};

static IDT: Once<InterruptDescriptorTable> = Once::new();

const SYSCALL_VECTOR: u8 = 0x80;

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
        #[cfg(feature = "hardware")]
        unsafe {
            let handler: extern "C" fn() -> ! = timer_interrupt_trampoline;
            let converted: extern "x86-interrupt" fn(InterruptStackFrame) = transmute(handler);
            idt[InterruptIndex::Timer.as_u8()].set_handler_fn(converted);
        }
        #[cfg(not(feature = "hardware"))]
        idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
        #[cfg(feature = "hardware")]
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
    trace!("BREAKPOINT: {:?}", stack_frame);
}

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _code: u64) -> ! {
    panic!("DOUBLE FAULT: {:?}", stack_frame);
}

extern "x86-interrupt" fn page_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: PageFaultErrorCode,
) {
    panic!(
        "PAGE FAULT: {:?}\nAccessed Address: {:?}\nError Code: {:?}",
        stack_frame,
        Cr2::read(),
        error_code
    );
}

extern "x86-interrupt" fn general_protection_fault_handler(
    stack_frame: InterruptStackFrame,
    error_code: u64,
) {
    panic!(
        "GENERAL PROTECTION FAULT: {:?}\nError Code: {:#x}",
        stack_frame, error_code
    );
}

#[cfg(feature = "hardware")]
unsafe extern "C" fn timer_interrupt_handler(
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

        if let Some(entry) = next {
            match entry.class {
                SchedulingClass::User => {
                    if let Some(table) = ProcessTable::global() {
                        if let Some((context, root)) =
                            table.take_thread_context(entry.pid, entry.tid)
                        {
                            regs.apply_user_context(&context);
                            write_user_frame(frame, &context);
                            switch_address_space(root);
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
