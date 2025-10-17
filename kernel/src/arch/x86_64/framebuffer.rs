#![cfg(feature = "hardware")]
#![allow(dead_code)]

//! Minimal text console renderer that mirrors serial output onto the bootloader framebuffer.

use core::sync::atomic::{AtomicBool, Ordering};

use bootloader_api::info::{FrameBuffer, FrameBufferInfo, PixelFormat};
use font8x8::UnicodeFonts;
use spin::Mutex;

const CHAR_WIDTH: usize = 8;
const CHAR_HEIGHT: usize = 8;
const TAB_WIDTH: usize = 4;
const EMPTY_GLYPH: [u8; CHAR_HEIGHT] = [0; CHAR_HEIGHT];

static FRAMEBUFFER: Mutex<Option<FrameBufferWriter>> = Mutex::new(None);
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// Installs the framebuffer writer using the bootloader-provided framebuffer.
pub fn init(framebuffer: &'static mut FrameBuffer) {
    let mut guard = FRAMEBUFFER.lock();
    if guard.is_none() {
        let writer = FrameBufferWriter::new(framebuffer);
        *guard = Some(writer);
        INITIALIZED.store(true, Ordering::Release);
    }
}

/// Appends a UTF-8 string to the framebuffer console.
pub fn write_str(s: &str) {
    if !INITIALIZED.load(Ordering::Acquire) {
        return;
    }
    if let Some(writer) = FRAMEBUFFER.lock().as_mut() {
        writer.write_string(s);
    }
}

struct FrameBufferWriter {
    framebuffer: *mut FrameBuffer,
    info: FrameBufferInfo,
    cursor_x: usize,
    cursor_y: usize,
}

unsafe impl Send for FrameBufferWriter {}

impl FrameBufferWriter {
    fn new(framebuffer: &'static mut FrameBuffer) -> Self {
        let info = framebuffer.info();
        let mut writer = Self {
            framebuffer,
            info,
            cursor_x: 0,
            cursor_y: 0,
        };
        writer.clear_screen();
        writer
    }

    fn write_string(&mut self, s: &str) {
        for ch in s.chars() {
            self.write_char(ch);
        }
    }

    fn write_char(&mut self, ch: char) {
        match ch {
            '\n' => self.new_line(),
            '\r' => self.cursor_x = 0,
            '\t' => {
                for _ in 0..TAB_WIDTH {
                    self.write_char(' ');
                }
            }
            '\u{08}' => {
                if self.cursor_x > 0 {
                    self.cursor_x -= 1;
                }
            }
            _ => {
                if self.max_columns() == 0 || self.max_rows() == 0 {
                    return;
                }
                if self.cursor_y >= self.max_rows() {
                    self.scroll();
                }

                let column_limit = self.max_columns();
                if self.cursor_x >= column_limit {
                    self.new_line();
                }

                if let Some(glyph) = font8x8::BASIC_FONTS.get(ch) {
                    self.draw_glyph(&glyph);
                } else {
                    self.draw_glyph(&EMPTY_GLYPH);
                }

                self.cursor_x += 1;
                if self.cursor_x >= column_limit {
                    self.new_line();
                }
            }
        }
    }

    fn draw_glyph(&mut self, glyph: &[u8; CHAR_HEIGHT]) {
        let x = self.cursor_x * CHAR_WIDTH;
        let y = self.cursor_y * CHAR_HEIGHT;
        let width = self.info.width as usize;
        let height = self.info.height as usize;

        if x + CHAR_WIDTH > width || y + CHAR_HEIGHT > height {
            return;
        }

        self.with_buffer(|info, buffer| {
            let stride_pixels = info.stride as usize;
            let bytes_per_pixel = info.bytes_per_pixel as usize;

            for (row, glyph_row) in glyph.iter().enumerate().take(CHAR_HEIGHT) {
                for col in 0..CHAR_WIDTH {
                    let pixel_on = (*glyph_row >> col) & 1 != 0;
                    let target_x = x + col;
                    let target_y = y + row;
                    let offset = (target_y * stride_pixels + target_x) * bytes_per_pixel;
                    Self::store_color(
                        &info.pixel_format,
                        &mut buffer[offset..offset + bytes_per_pixel],
                        if pixel_on { Color::FOREGROUND } else { Color::BACKGROUND },
                    );
                }
            }
        });
    }

    fn new_line(&mut self) {
        self.cursor_x = 0;
        self.cursor_y += 1;
        if self.cursor_y >= self.max_rows() {
            self.scroll();
        }
    }

    fn scroll(&mut self) {
        let rows = self.max_rows();
        if rows == 0 {
            return;
        }
        self.with_buffer(|info, buffer| {
            let stride_pixels = info.stride as usize;
            let bytes_per_pixel = info.bytes_per_pixel as usize;
            let row_bytes = stride_pixels * bytes_per_pixel;
            let scroll_bytes = row_bytes * CHAR_HEIGHT;
            let len = buffer.len();
            if scroll_bytes >= len {
                buffer.fill(0);
                return;
            }
            buffer.copy_within(scroll_bytes.., 0);
            for byte in &mut buffer[len - scroll_bytes..] {
                *byte = 0;
            }
        });
        if rows > 0 {
            self.cursor_y = rows - 1;
        }
    }

    fn clear_screen(&mut self) {
        self.with_buffer(|_, buffer| buffer.fill(0));
        self.cursor_x = 0;
        self.cursor_y = 0;
    }

    fn max_columns(&self) -> usize {
        let width = self.info.width as usize;
        width / CHAR_WIDTH
    }

    fn max_rows(&self) -> usize {
        let height = self.info.height as usize;
        height / CHAR_HEIGHT
    }

    fn with_buffer<F>(&mut self, mut f: F)
    where
        F: FnMut(&FrameBufferInfo, &mut [u8]),
    {
        let framebuffer = unsafe { &mut *self.framebuffer };
        let buffer = framebuffer.buffer_mut();
        f(&self.info, buffer);
    }

    fn store_color(pixel_format: &PixelFormat, target: &mut [u8], color: Color) {
        match pixel_format {
            PixelFormat::Rgb | PixelFormat::Bgr => {
                if target.len() < 3 {
                    return;
                }
                let (r, g, b) = color.rgb();
                if matches!(pixel_format, PixelFormat::Rgb) {
                    target[0] = r;
                    target[1] = g;
                    target[2] = b;
                } else {
                    target[0] = b;
                    target[1] = g;
                    target[2] = r;
                }
            }
            PixelFormat::U8 => {
                target[0] = color.grayscale();
            }
            _ => {
                for byte in target.iter_mut() {
                    *byte = color.grayscale();
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Color {
    FOREGROUND,
    BACKGROUND,
}

impl Color {
    fn rgb(self) -> (u8, u8, u8) {
        match self {
            Color::FOREGROUND => (0xEE, 0xEE, 0xEE),
            Color::BACKGROUND => (0x00, 0x00, 0x00),
        }
    }

    fn grayscale(self) -> u8 {
        match self {
            Color::FOREGROUND => 0xEE,
            Color::BACKGROUND => 0x00,
        }
    }

}
