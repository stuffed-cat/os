//! User-mode context switching helpers for x86-64.

use crate::user::UserContext;
use x86_64::structures::paging::PhysFrame;

/// Transfers control to user space using the provided register snapshot and CR3 root.
///
/// # Safety
///
/// The caller must ensure interrupts are masked appropriately and that the supplied
/// context references a valid user-mode stack and instruction pointer.
#[cfg(feature = "hardware")]
pub unsafe fn enter_user_mode(context: &UserContext, page_table: PhysFrame) -> ! {
    use x86_64::registers::control::{Cr3, Cr3Flags};

    let frame = context.frame().clone();
    let (root, _) = Cr3::read();
    if root != page_table {
        Cr3::write(page_table, Cr3Flags::empty());
    }

    // Selector indices are provided by the GDT setup.
    let user_code = super::gdt::user_code_selector().0 as u64;
    let user_data = super::gdt::user_data_selector().0 as u64;

    core::arch::asm!(
        // Load general-purpose registers from the trap frame snapshot.
        "mov rax, {rax}",
        "mov rbx, {rbx}",
        "mov rcx, {rcx}",
        "mov rdx, {rdx}",
        "mov rsi, {rsi}",
        "mov rdi, {rdi}",
        "mov r8,  {r8}",
        "mov r9,  {r9}",
        "mov r10, {r10}",
        "mov r11, {r11}",
        "mov r12, {r12}",
        "mov r13, {r13}",
        "mov r14, {r14}",
        "mov r15, {r15}",
        "mov rbp, {rbp}",
        // Prepare data segments for user mode access.
        "mov ds, {data}",
        "mov es, {data}",
        "mov fs, {data}",
        "mov gs, {data}",
        // Push user mode stack, flags, and code selector for IRETQ.
        "push {data64}",
        "push {rsp}",
        "push {rflags}",
        "push {code}",
        "push {rip}",
        "iretq",
        rax = in(reg) frame.rax,
        rbx = in(reg) frame.rbx,
        rcx = in(reg) frame.rcx,
        rdx = in(reg) frame.rdx,
        rsi = in(reg) frame.rsi,
        rdi = in(reg) frame.rdi,
        r8 = in(reg) frame.r8,
        r9 = in(reg) frame.r9,
        r10 = in(reg) frame.r10,
        r11 = in(reg) frame.r11,
        r12 = in(reg) frame.r12,
        r13 = in(reg) frame.r13,
        r14 = in(reg) frame.r14,
        r15 = in(reg) frame.r15,
        rbp = in(reg) frame.rbp,
        rsp = in(reg) frame.rsp,
        rip = in(reg) frame.rip,
        rflags = in(reg) frame.rflags,
        code = in(reg) user_code,
        data = in("dx") (user_data as u16),
        data64 = in(reg) user_data,
        options(noreturn)
    );
    core::hint::unreachable_unchecked();
}

/// Host builds rely on a stubbed implementation because no privilege transition is available.
#[cfg(not(feature = "hardware"))]
pub unsafe fn enter_user_mode(_context: &UserContext, _page_table: PhysFrame) -> ! {
    panic!("user-mode entry is not available in std builds");
}
