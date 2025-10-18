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

const ANSI_BUFFER_CAPACITY: usize = 16;

#[derive(Clone, Copy)]
enum AnsiState {
    Idle,
    Esc,
    Csi,
}

#[derive(Clone, Copy)]
struct RgbColor {
    r: u8,
    g: u8,
    b: u8,
}

impl RgbColor {
    const fn new(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }
}

const DEFAULT_FG: RgbColor = RgbColor::new(0xEE, 0xEE, 0xEE);
const DEFAULT_BG: RgbColor = RgbColor::new(0x00, 0x00, 0x00);

const ANSI_BASE_COLORS: [RgbColor; 8] = [
    RgbColor::new(0x00, 0x00, 0x00), // black
    RgbColor::new(0xAA, 0x00, 0x00), // red
    RgbColor::new(0x00, 0xAA, 0x00), // green
    RgbColor::new(0xAA, 0xAA, 0x00), // yellow
    RgbColor::new(0x00, 0x00, 0xAA), // blue
    RgbColor::new(0xAA, 0x00, 0xAA), // magenta
    RgbColor::new(0x00, 0xAA, 0xAA), // cyan
    RgbColor::new(0xAA, 0xAA, 0xAA), // white
];

const ANSI_BRIGHT_COLORS: [RgbColor; 8] = [
    RgbColor::new(0x55, 0x55, 0x55), // bright black (gray)
    RgbColor::new(0xFF, 0x55, 0x55), // bright red
    RgbColor::new(0x55, 0xFF, 0x55), // bright green
    RgbColor::new(0xFF, 0xFF, 0x55), // bright yellow
    RgbColor::new(0x55, 0x55, 0xFF), // bright blue
    RgbColor::new(0xFF, 0x55, 0xFF), // bright magenta
    RgbColor::new(0x55, 0xFF, 0xFF), // bright cyan
    RgbColor::new(0xFF, 0xFF, 0xFF), // bright white
];

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
    fg_color: RgbColor,
    bg_color: RgbColor,
    bold: bool,
    ansi_state: AnsiState,
    ansi_buffer: [char; ANSI_BUFFER_CAPACITY],
    ansi_len: usize,
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
            fg_color: DEFAULT_FG,
            bg_color: DEFAULT_BG,
            bold: false,
            ansi_state: AnsiState::Idle,
            ansi_buffer: ['\0'; ANSI_BUFFER_CAPACITY],
            ansi_len: 0,
        };
        writer.clear_screen();
        writer
    }

    fn write_string(&mut self, s: &str) {
        for ch in s.chars() {
            self.process_char(ch);
        }
    }

    fn process_char(&mut self, ch: char) {
        match self.ansi_state {
            AnsiState::Idle => {
                if ch == '\x1b' {
                    self.ansi_state = AnsiState::Esc;
                } else {
                    self.write_visible_char(ch);
                }
            }
            AnsiState::Esc => {
                if ch == '[' {
                    self.ansi_state = AnsiState::Csi;
                    self.ansi_len = 0;
                } else {
                    self.ansi_state = AnsiState::Idle;
                    self.write_visible_char('\x1b');
                    self.write_visible_char(ch);
                }
            }
            AnsiState::Csi => {
                if ch.is_ascii_digit() || ch == ';' {
                    if self.ansi_len < ANSI_BUFFER_CAPACITY {
                        self.ansi_buffer[self.ansi_len] = ch;
                        self.ansi_len += 1;
                    }
                } else {
                    self.apply_csi(ch);
                    self.ansi_state = AnsiState::Idle;
                    self.ansi_len = 0;
                }
            }
        }
    }

    fn write_visible_char(&mut self, ch: char) {
        match ch {
            '\n' => self.new_line(),
            '\r' => self.cursor_x = 0,
            '\t' => {
                for _ in 0..TAB_WIDTH {
                    self.write_visible_char(' ');
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

    fn apply_csi(&mut self, final_char: char) {
        if final_char != 'm' {
            return;
        }

        let mut params = [0u16; 8];
        let mut count = 0usize;
        let mut value = 0u16;
        let mut has_value = false;

        if self.ansi_len == 0 {
            params[0] = 0;
            count = 1;
        } else {
            for &ch in &self.ansi_buffer[..self.ansi_len] {
                if ch.is_ascii_digit() {
                    value = value
                        .saturating_mul(10)
                        .saturating_add((ch as u8 - b'0') as u16);
                    has_value = true;
                } else if ch == ';' {
                    if count < params.len() {
                        params[count] = if has_value { value } else { 0 };
                        count += 1;
                    }
                    value = 0;
                    has_value = false;
                }
            }
            if count < params.len() {
                params[count] = if has_value { value } else { 0 };
                count += 1;
            }
        }

        for &param in &params[..count] {
            self.apply_sgr_param(param);
        }
    }

    fn apply_sgr_param(&mut self, param: u16) {
        match param {
            0 => self.reset_colors(),
            1 => self.bold = true,
            22 => self.bold = false,
            30..=37 => {
                let index = (param - 30) as usize;
                let bright = self.bold;
                self.fg_color = if bright {
                    ANSI_BRIGHT_COLORS[index]
                } else {
                    ANSI_BASE_COLORS[index]
                };
            }
            90..=97 => {
                let index = (param - 90) as usize;
                self.fg_color = ANSI_BRIGHT_COLORS[index];
            }
            40..=47 => {
                let index = (param - 40) as usize;
                self.bg_color = ANSI_BASE_COLORS[index];
            }
            100..=107 => {
                let index = (param - 100) as usize;
                self.bg_color = ANSI_BRIGHT_COLORS[index];
            }
            _ => {}
        }
    }

    fn reset_colors(&mut self) {
        self.bold = false;
        self.fg_color = DEFAULT_FG;
        self.bg_color = DEFAULT_BG;
    }

    fn draw_glyph(&mut self, glyph: &[u8; CHAR_HEIGHT]) {
        let x = self.cursor_x * CHAR_WIDTH;
        let y = self.cursor_y * CHAR_HEIGHT;
        let width = self.info.width as usize;
        let height = self.info.height as usize;

        if x + CHAR_WIDTH > width || y + CHAR_HEIGHT > height {
            return;
        }

        let fg = self.fg_color;
        let bg = self.bg_color;
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
                        if pixel_on { fg } else { bg },
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
        let bg = self.bg_color;
        self.with_buffer(|info, buffer| {
            let stride_pixels = info.stride as usize;
            let bytes_per_pixel = info.bytes_per_pixel as usize;
            let row_bytes = stride_pixels * bytes_per_pixel;
            let scroll_bytes = row_bytes * CHAR_HEIGHT;
            let len = buffer.len();
            if scroll_bytes >= len {
                Self::fill_with_color(info, buffer, bg);
                return;
            }
            buffer.copy_within(scroll_bytes.., 0);
            Self::fill_with_color(info, &mut buffer[len - scroll_bytes..], bg);
        });
        if rows > 0 {
            self.cursor_y = rows - 1;
        }
    }

    fn clear_screen(&mut self) {
        let bg = self.bg_color;
        self.with_buffer(|info, buffer| Self::fill_with_color(info, buffer, bg));
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

    fn fill_with_color(info: &FrameBufferInfo, slice: &mut [u8], color: RgbColor) {
        let bytes_per_pixel = info.bytes_per_pixel as usize;
        if bytes_per_pixel == 0 {
            return;
        }
        for chunk in slice.chunks_mut(bytes_per_pixel) {
            Self::store_color(&info.pixel_format, chunk, color);
        }
    }

    fn store_color(pixel_format: &PixelFormat, target: &mut [u8], color: RgbColor) {
        match pixel_format {
            PixelFormat::Rgb | PixelFormat::Bgr => {
                if target.len() < 3 {
                    return;
                }
                let (r, g, b) = (color.r, color.g, color.b);
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
                target[0] =
                    ((u16::from(color.r) * 30 + u16::from(color.g) * 59 + u16::from(color.b) * 11)
                        / 100) as u8;
            }
            _ => {
                let gray =
                    ((u16::from(color.r) * 30 + u16::from(color.g) * 59 + u16::from(color.b) * 11)
                        / 100) as u8;
                for byte in target.iter_mut() {
                    *byte = gray;
                }
            }
        }
    }
}
