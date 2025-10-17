//! Interrupt Descriptor Table initialization and handlers.

use log::trace;
use spin::Once;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use super::{
    gdt,
    interrupts::{self, InterruptIndex},
};

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

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    interrupts::notify_end_of_interrupt(InterruptIndex::Timer);
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    interrupts::notify_end_of_interrupt(InterruptIndex::Keyboard);
}
