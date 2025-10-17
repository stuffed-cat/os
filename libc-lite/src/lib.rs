#![cfg_attr(all(not(test), feature = "no_std"), no_std)]

//! `libc-lite` now forwards to the upstream picolibc implementations for core
//! string/memory primitives, ensuring compatibility with standard C
//! expectations while the rest of the libc surface is brought online.

mod ffi {
    use core::ffi::c_void;

    extern "C" {
        pub fn memcpy(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
        pub fn memmove(dest: *mut c_void, src: *const c_void, n: usize) -> *mut c_void;
        pub fn memcmp(lhs: *const c_void, rhs: *const c_void, n: usize) -> i32;
        pub fn memset(dest: *mut c_void, value: i32, n: usize) -> *mut c_void;
        pub fn strlen(ptr: *const u8) -> usize;
        pub fn strcpy(dest: *mut u8, src: *const u8) -> *mut u8;
        pub fn strncpy(dest: *mut u8, src: *const u8, n: usize) -> *mut u8;
        pub fn strcmp(lhs: *const u8, rhs: *const u8) -> i32;
    }
}

pub use ffi::{memcmp, memcpy, memmove, memset, strcmp, strcpy, strlen, strncpy};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn memset_zeroes_buffer() {
        let mut buf = [1u8; 8];
        unsafe {
            memset(buf.as_mut_ptr().cast(), 0, buf.len());
        }
        assert_eq!(&buf, &[0; 8]);
    }

    #[test]
    fn memcpy_copies_bytes() {
        let src = [1u8, 2, 3, 4];
        let mut dst = [0u8; 4];
        unsafe {
            memcpy(dst.as_mut_ptr().cast(), src.as_ptr().cast(), src.len());
        }
        assert_eq!(dst, src);
    }

    #[test]
    fn memmove_handles_overlap() {
        let mut buf = *b"abcdef";
        unsafe {
            memmove(buf[1..].as_mut_ptr().cast(), buf.as_ptr().cast(), 5);
        }
        assert_eq!(&buf, b"aabcde");
    }

    #[test]
    fn strlen_counts_bytes() {
        let data = b"hello\0";
        let len = unsafe { strlen(data.as_ptr()) };
        assert_eq!(len, 5);
    }

    #[test]
    fn strcmp_orders_strings() {
        let a = b"abc\0";
        let b = b"abd\0";
        assert!(unsafe { strcmp(a.as_ptr(), b.as_ptr()) } < 0);
    }
}

#[cfg(all(not(test), feature = "no_std"))]
#[panic_handler]
fn panic(_: &core::panic::PanicInfo<'_>) -> ! {
    loop {}
}
