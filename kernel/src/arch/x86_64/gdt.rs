//! Global Descriptor Table setup for privileged execution.

use spin::Once;
use x86_64::instructions::segmentation::{Segment, CS, DS, ES, SS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{Descriptor, GlobalDescriptorTable, SegmentSelector};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

const DOUBLE_FAULT_IST_INDEX: usize = 0;

static TSS: Once<TaskStateSegment> = Once::new();
static GDT: Once<(GlobalDescriptorTable, Selectors)> = Once::new();

struct Selectors {
    code_selector: SegmentSelector,
    data_selector: SegmentSelector,
    user_code_selector: SegmentSelector,
    user_data_selector: SegmentSelector,
    tss_selector: SegmentSelector,
}

/// Initializes the GDT and TSS needed for interrupt handling.
pub fn init() {
    TSS.call_once(|| {
        let mut tss = TaskStateSegment::new();
        static mut DOUBLE_FAULT_STACK: [u8; 4096] = [0; 4096];
        let stack_start = VirtAddr::from_ptr(core::ptr::addr_of!(DOUBLE_FAULT_STACK));
        let stack_end = stack_start + 4096;
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX] = stack_end;
        tss
    });

    GDT.call_once(|| {
        let mut gdt = GlobalDescriptorTable::new();
        let code_selector = gdt.append(Descriptor::kernel_code_segment());
        let data_selector = gdt.append(Descriptor::kernel_data_segment());
        let user_code_selector = gdt.append(Descriptor::user_code_segment());
        let user_data_selector = gdt.append(Descriptor::user_data_segment());
        let tss_selector = gdt.append(Descriptor::tss_segment(TSS.wait()));
        (
            gdt,
            Selectors {
                code_selector,
                data_selector,
                user_code_selector,
                user_data_selector,
                tss_selector,
            },
        )
    });

    let gdt_data = GDT.wait();
    gdt_data.0.load();
    unsafe {
        CS::set_reg(gdt_data.1.code_selector);
        DS::set_reg(gdt_data.1.data_selector);
        ES::set_reg(gdt_data.1.data_selector);
        SS::set_reg(gdt_data.1.data_selector);
        load_tss(gdt_data.1.tss_selector);
    }
}

/// Provides access to the TSS IST index for other modules.
pub fn double_fault_ist_index() -> u16 {
    DOUBLE_FAULT_IST_INDEX as u16
}

fn selectors() -> &'static Selectors {
    &GDT.wait().1
}

/// Returns the segment selector for user-mode code.
pub fn user_code_selector() -> SegmentSelector {
    selectors().user_code_selector
}

/// Returns the segment selector for user-mode data segments.
pub fn user_data_selector() -> SegmentSelector {
    selectors().user_data_selector
}
