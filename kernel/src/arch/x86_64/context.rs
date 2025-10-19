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

    let frame_ptr = &frame as *const crate::user::TrapFrame;

    core::arch::asm!(
        "mov rax, qword ptr [{frame} + {rax_off}]",
        "mov rbx, qword ptr [{frame} + {rbx_off}]",
        "mov rcx, qword ptr [{frame} + {rcx_off}]",
        "mov rdx, qword ptr [{frame} + {rdx_off}]",
        "mov rsi, qword ptr [{frame} + {rsi_off}]",
        "mov rdi, qword ptr [{frame} + {rdi_off}]",
        "mov r8,  qword ptr [{frame} + {r8_off}]",
        "mov r9,  qword ptr [{frame} + {r9_off}]",
        "mov r10, qword ptr [{frame} + {r10_off}]",
        "mov r11, qword ptr [{frame} + {r11_off}]",
        "mov r12, qword ptr [{frame} + {r12_off}]",
        "mov r13, qword ptr [{frame} + {r13_off}]",
        "mov r14, qword ptr [{frame} + {r14_off}]",
        "mov r15, qword ptr [{frame} + {r15_off}]",
        "mov rbp, qword ptr [{frame} + {rbp_off}]",
        "mov ds, {data:x}",
        "mov es, {data:x}",
        "mov fs, {data:x}",
        "mov gs, {data:x}",
        "push {data64}",
        "push qword ptr [{frame} + {rsp_off}]",
        "push qword ptr [{frame} + {rflags_off}]",
        "push {code}",
        "push qword ptr [{frame} + {rip_off}]",
        "iretq",
        frame = in(reg) frame_ptr,
        data = in(reg) (user_data as u16),
        data64 = in(reg) user_data,
        code = in(reg) user_code,
        rax_off = const core::mem::offset_of!(crate::user::TrapFrame, rax),
        rbx_off = const core::mem::offset_of!(crate::user::TrapFrame, rbx),
        rcx_off = const core::mem::offset_of!(crate::user::TrapFrame, rcx),
        rdx_off = const core::mem::offset_of!(crate::user::TrapFrame, rdx),
        rsi_off = const core::mem::offset_of!(crate::user::TrapFrame, rsi),
        rdi_off = const core::mem::offset_of!(crate::user::TrapFrame, rdi),
        r8_off = const core::mem::offset_of!(crate::user::TrapFrame, r8),
        r9_off = const core::mem::offset_of!(crate::user::TrapFrame, r9),
        r10_off = const core::mem::offset_of!(crate::user::TrapFrame, r10),
        r11_off = const core::mem::offset_of!(crate::user::TrapFrame, r11),
        r12_off = const core::mem::offset_of!(crate::user::TrapFrame, r12),
        r13_off = const core::mem::offset_of!(crate::user::TrapFrame, r13),
        r14_off = const core::mem::offset_of!(crate::user::TrapFrame, r14),
        r15_off = const core::mem::offset_of!(crate::user::TrapFrame, r15),
        rbp_off = const core::mem::offset_of!(crate::user::TrapFrame, rbp),
        rsp_off = const core::mem::offset_of!(crate::user::TrapFrame, rsp),
        rip_off = const core::mem::offset_of!(crate::user::TrapFrame, rip),
        rflags_off = const core::mem::offset_of!(crate::user::TrapFrame, rflags),
        options(noreturn)
    );
}

/// Host builds rely on a stubbed implementation because no privilege transition is available.
#[cfg(not(feature = "hardware"))]
pub unsafe fn enter_user_mode(_context: &UserContext, _page_table: PhysFrame) -> ! {
    panic!("user-mode entry is not available in std builds");
}
