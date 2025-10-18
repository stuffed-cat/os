//! Serial port support for early boot logging.

#[cfg(feature = "hardware")]
mod imp {
    use core::fmt::{self, Arguments, Write};

    use log::{LevelFilter, Log, Metadata, Record};
    use spin::{Mutex, Once};
    use uart_16550::SerialPort;

    use crate::arch::x86_64::framebuffer;

    /// I/O port base for the primary serial port.
    const SERIAL_IO_PORT: u16 = 0x3F8;

    static SERIAL: Mutex<Option<SerialPort>> = Mutex::new(None);
    static LOGGER: SerialLogger = SerialLogger;
    static LOGGER_ONCE: Once<()> = Once::new();

    /// Initializes the UART-backed logger.
    pub fn init() {
        LOGGER_ONCE.call_once(|| {
            let mut serial = unsafe { SerialPort::new(SERIAL_IO_PORT) };
            serial.init();
            *SERIAL.lock() = Some(serial);
            let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Trace));
        });
    }

    struct SerialLogger;

    impl Log for SerialLogger {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &Record<'_>) {
            if !self.enabled(record.metadata()) {
                return;
            }
            with_serial(|serial| {
                let _ = writeln!(
                    SerialWriter(serial),
                    "[{}] {}",
                    record.level(),
                    record.args()
                );
            });
        }

        fn flush(&self) {
            with_serial(|serial| {
                let _ = SerialWriter(serial).flush();
            });
        }
    }

    struct SerialWriter<'a>(&'a mut SerialPort);

    impl<'a> Write for SerialWriter<'a> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for byte in s.bytes() {
                self.0.send(byte);
            }
            framebuffer::write_str(s);
            Ok(())
        }
    }

    impl<'a> SerialWriter<'a> {
        fn flush(&mut self) -> fmt::Result {
            // UART output is instantaneous for this simplistic model.
            Ok(())
        }
    }

    /// Writes directly to the serial port for panic handlers.
    pub fn write_str(s: &str) {
        with_serial(|serial| {
            let _ = SerialWriter(serial).write_str(s);
        });
    }

    /// Writes raw bytes to the serial port without UTF-8 assumptions.
    pub fn write_bytes(bytes: &[u8]) {
        with_serial(|serial| {
            for byte in bytes {
                serial.send(*byte);
            }
        });
    }

    /// Writes formatted data to the serial port.
    pub fn write_fmt(args: Arguments<'_>) {
        with_serial(|serial| {
            let _ = SerialWriter(serial).write_fmt(args);
        });
    }

    fn with_serial<F>(mut f: F)
    where
        F: FnMut(&mut SerialPort),
    {
        let mut guard = SERIAL.lock();
        if guard.is_none() {
            let mut serial = unsafe { SerialPort::new(SERIAL_IO_PORT) };
            serial.init();
            *guard = Some(serial);
        }
        if let Some(serial) = guard.as_mut() {
            f(serial);
        }
    }
}

#[cfg(not(feature = "hardware"))]
mod imp {
    use alloc::vec::Vec;
    use core::fmt::{self, Arguments, Write};

    use log::{LevelFilter, Log, Metadata, Record};
    use spin::Mutex;

    struct BufferWriter<'a>(&'a mut Vec<u8>);

    impl<'a> Write for BufferWriter<'a> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            self.0.extend_from_slice(s.as_bytes());
            Ok(())
        }
    }

    static BUFFER: Mutex<Vec<u8>> = Mutex::new(Vec::new());
    static LOGGER: StubLogger = StubLogger;
    static LOGGER_INIT: Mutex<bool> = Mutex::new(false);

    /// Initializes the stubbed serial logger for host-based tests.
    pub fn init() {
        let mut init = LOGGER_INIT.lock();
        if !*init {
            let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(LevelFilter::Trace));
            *init = true;
        }
    }

    /// Captures UTF-8 output in the backing buffer instead of touching hardware.
    pub fn write_str(s: &str) {
        BUFFER.lock().extend_from_slice(s.as_bytes());
    }

    /// Captures raw byte output in the backing buffer.
    pub fn write_bytes(bytes: &[u8]) {
        BUFFER.lock().extend_from_slice(bytes);
    }

    /// Formats arbitrary arguments into the buffer without performing I/O.
    pub fn write_fmt(args: Arguments<'_>) {
        let mut guard = BUFFER.lock();
        let mut writer = BufferWriter(&mut *guard);
        let _ = fmt::write(&mut writer, args);
    }

    pub struct StubLogger;

    impl Log for StubLogger {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn log(&self, record: &Record<'_>) {
            let mut guard = BUFFER.lock();
            let mut writer = BufferWriter(&mut *guard);
            let _ = writeln!(writer, "[{}] {}", record.level(), record.args());
        }

        fn flush(&self) {}
    }

    /// Retrieves the accumulated buffer for test inspection.
    /// Drains the buffered output for inspection in tests.
    pub fn drain() -> Vec<u8> {
        BUFFER.lock().drain(..).collect()
    }
}

pub use imp::init;
pub use imp::write_bytes;
pub use imp::write_fmt;
pub use imp::write_str;

#[cfg(not(feature = "hardware"))]
pub use imp::drain;
