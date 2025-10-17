//! Serial port support for early boot logging.

use core::fmt::{self, Write};

use log::{LevelFilter, Log, Metadata, Record};
use spin::{Mutex, Once};
use uart_16550::SerialPort;

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
