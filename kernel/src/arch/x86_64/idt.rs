//! Interrupt Descriptor Table initialization and handlers.

use log::trace;
use spin::Once;
use x86_64::registers::control::Cr2;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};

use super::{
    gdt,
    interrupts::{self, InterruptIndex},
    keyboard,
};
use crate::shell;

static IDT: Once<InterruptDescriptorTable> = Once::new();

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
        idt[InterruptIndex::Timer.as_u8()].set_handler_fn(timer_interrupt_handler);
        idt[InterruptIndex::Keyboard.as_u8()].set_handler_fn(keyboard_interrupt_handler);
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt.general_protection_fault
            .set_handler_fn(general_protection_fault_handler);
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
        stack_frame,
        error_code
    );
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    interrupts::notify_end_of_interrupt(InterruptIndex::Timer);
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    let scancode = keyboard::read_scancode();
    shell::enqueue_scancode(scancode);
    interrupts::notify_end_of_interrupt(InterruptIndex::Keyboard);
}
