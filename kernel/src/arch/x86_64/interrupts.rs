//! Interrupt controller integration for PIC-based systems.

use log::trace;
#[cfg(not(test))]
use spin::Mutex;
use x86_64::instructions::interrupts;
#[cfg(not(test))]
use x86_64::instructions::port::Port;

/// Offset for the first PIC.
pub const PIC_1_OFFSET: u8 = 32;
/// Offset for the second PIC in the cascade.
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

#[cfg(not(test))]
static PICS: Mutex<Pics> = Mutex::new(unsafe { Pics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

/// Enumeration of hardware interrupt vectors used by the kernel.
#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    /// Programmable interval timer interrupt.
    Timer = PIC_1_OFFSET,
    /// Keyboard controller interrupt.
    Keyboard,
}

impl InterruptIndex {
    /// Returns the interrupt vector number.
    pub fn as_u8(self) -> u8 {
        self as u8
    }

    /// Returns the vector as usize for table indexing.
    pub fn as_usize(self) -> usize {
        usize::from(self.as_u8())
    }
}

/// Trait abstracting primitive interrupt controller operations.
pub trait InterruptController {
    /// Initializes the controller.
    fn init(&self);
    /// Enables maskable interrupts.
    fn enable(&self);
    /// Disables maskable interrupts.
    fn disable(&self);
    /// Acknowledges the completion of the given interrupt vector.
    fn ack(&self, vector: u8);
}

/// Implementation for the chained PIC setup.
pub struct Pic8259Controller;

impl Pic8259Controller {
    /// Creates a new controller instance.
    pub const fn new() -> Self {
        Self
    }
}

impl InterruptController for Pic8259Controller {
    fn init(&self) {
        trace!("Initializing chained PIC");
        #[cfg(not(test))]
        unsafe {
            PICS.lock().initialize();
        }
    }

    fn enable(&self) {
        interrupts::enable();
    }

    fn disable(&self) {
        interrupts::disable();
    }

    fn ack(&self, vector: u8) {
        notify_end_of_interrupt_raw(vector);
    }
}

/// Notifies the PIC that an interrupt has been processed.
pub fn notify_end_of_interrupt(index: InterruptIndex) {
    notify_end_of_interrupt_raw(index.as_u8());
}

fn notify_end_of_interrupt_raw(vector: u8) {
    #[cfg(test)]
    let _ = vector;
    #[cfg(not(test))]
    unsafe {
        PICS.lock().notify_end_of_interrupt(vector);
    }
}

#[cfg(not(test))]
struct Pic {
    offset: u8,
    command: Port<u8>,
    data: Port<u8>,
}

#[cfg(not(test))]
impl Pic {
    const unsafe fn new(offset: u8, command_port: u16, data_port: u16) -> Self {
        Self {
            offset,
            command: Port::new(command_port),
            data: Port::new(data_port),
        }
    }
}

#[cfg(not(test))]
struct Pics {
    primary: Pic,
    secondary: Pic,
}

#[cfg(not(test))]
impl Pics {
    const unsafe fn new(primary_offset: u8, secondary_offset: u8) -> Self {
        Self {
            primary: Pic::new(primary_offset, 0x20, 0x21),
            secondary: Pic::new(secondary_offset, 0xA0, 0xA1),
        }
    }

    unsafe fn initialize(&mut self) {
        trace!(
            "Initializing chained PIC with offsets {} and {}",
            self.primary.offset, self.secondary.offset
        );

        let mut wait_port: Port<u8> = Port::new(0x80);

        let primary_mask = self.primary.data.read();
        let secondary_mask = self.secondary.data.read();

        self.primary.command.write(0x11);
        wait_port.write(0);
        self.secondary.command.write(0x11);
        wait_port.write(0);

        self.primary.data.write(self.primary.offset);
        wait_port.write(0);
        self.secondary.data.write(self.secondary.offset);
        wait_port.write(0);

        self.primary.data.write(4);
        wait_port.write(0);
        self.secondary.data.write(2);
        wait_port.write(0);

        self.primary.data.write(0x01);
        wait_port.write(0);
        self.secondary.data.write(0x01);
        wait_port.write(0);

        self.primary.data.write(primary_mask);
        self.secondary.data.write(secondary_mask);
    }

    unsafe fn notify_end_of_interrupt(&mut self, vector: u8) {
        if vector >= PIC_2_OFFSET && vector < PIC_2_OFFSET + 8 {
            self.secondary.command.write(0x20);
        }
        self.primary.command.write(0x20);
    }
}
